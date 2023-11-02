//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form
//! of audio signals in the method of phase shift keying (PSK). A frame in the stream consists as
//! follows:
//! - Preamble (PREAMBLE_SYMBOL_LEN symbols): A sequence of symbols to indicate the start of a
//!  frame.
//! - Header (HEADER_BITS_LEN bits): The header of the frame, containing the encoded result of the
//!  meta data and the parity.
//!     - Meta (META_BITS_LEN bits): A sequence of bits to indicate the metadata of the frame, encoded
//!      with convolutional code.
//!         - Flag (META_FLAG_BITS_LEN bits): A bit to indicate if the frame is the last frame of the
//!          packet. 1 for the last frame, 0 for the rest.
//!         - Type (META_TYPE_BITS_LEN bits): A bit to indicate the type of the frame. 1 for data, 0
//!          for parity.    
//!         - Reserved (META_RESERVED_BITS_LEN bits): Reserved for future use.
//!     - Parity (PARITY_BITS_LEN bits): The parity of the body.
//! - Body (BODY_BITS_LEN bits): The body of the frame, containing the encoded result of the
//!  length and payload.
//!     - Length (LENGTH_BITS_LEN bits): The length of the payload.
//!     - Payload (PAYLOAD_BITS_LEN bits): The payload of the frame.

extern crate reed_solomon_erasure;
use super::{
    conv::ConvCode,
    encode::{DecodeToInt, EncodeFromBytes},
    signal::{self, BandPass, Deinterleave, InterpolateBack, InterpolateFront},
    Preamble, Symbol, Warmup,
};
use crate::{
    rather::encode::DecodeToBytes,
    raudio::{AudioInputStream, AudioOutputStream, AudioSamples, AudioTrack, ContinuousStream},
};
use bitvec::prelude::*;
use cpal::SupportedStreamConfig;
use crc::{Crc, CRC_16_IBM_SDLC};
use num::Complex;
use reed_solomon_erasure::galois_8::ReedSolomon;
use std::{
    mem,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{self, Poll, Waker},
    time::Duration,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::{Stream, StreamExt};

const WARMUP_SYMBOL_LEN: usize = 8;
const PREAMBLE_SYMBOL_LEN: usize = 32; // 8 | 16 | 32 | 64
const PREAMBLE_CORR_THRESHOLD: f32 = 0.15;

const CONV_GENERATORS: &[usize] = &[5, 7];
const CONV_FACTOR: usize = CONV_GENERATORS.len();
const CONV_ORDER: usize = {
    let mut max = 0usize;
    let mut index = 0;
    while index < CONV_GENERATORS.len() {
        if max < CONV_GENERATORS[index] {
            max = CONV_GENERATORS[index];
        }
        index += 1;
    }
    max.ilog2() as usize
};

const META_FLAG_BITS_LEN: usize = 1;
const META_TYPE_BITS_LEN: usize = 1;
const META_RESERVED_BITS_LEN: usize = 2;
const META_BITS_LEN: usize = META_FLAG_BITS_LEN + META_FLAG_BITS_LEN + META_RESERVED_BITS_LEN;

const PARITY_ALGORITHM: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
const PARITY_RATIO: f32 = 0.25;
const PARITY_BYTE_LEN: usize = 2;
const PARITY_BITS_LEN: usize = PARITY_BYTE_LEN * 8;

const LENGTH_BYTE_LEN: usize = 2;
const LENGTH_BITS_LEN: usize = LENGTH_BYTE_LEN * 8;
const PAYLOAD_BYTE_LEN: usize = 8;
const PAYLOAD_BITS_LEN: usize = PAYLOAD_BYTE_LEN * 8;

const META_ENCODED_BITS_LEN: usize = (META_BITS_LEN + CONV_ORDER) * CONV_FACTOR;
const HEADER_BITS_LEN: usize = META_ENCODED_BITS_LEN + PARITY_BITS_LEN;
const BODY_BITS_LEN: usize = LENGTH_BITS_LEN + PAYLOAD_BITS_LEN;
#[allow(dead_code)]
const PACKET_BITS_LEN: usize = HEADER_BITS_LEN + BODY_BITS_LEN;

#[derive(Debug, Clone)]
pub struct AtherStreamConfig {
    pub frequency: u32,
    pub bit_rate: u32,
    pub warmup: Warmup,
    pub preamble: Preamble,
    pub symbols: (Symbol, Symbol),
    pub stream_config: SupportedStreamConfig,
}

impl AtherStreamConfig {
    pub fn new(frequency: u32, bit_rate: u32, stream_config: SupportedStreamConfig) -> Self {
        let duration = 1.0 / bit_rate as f32;
        let sample_rate = stream_config.sample_rate().0;

        Self {
            frequency,
            bit_rate,
            warmup: Warmup::new(WARMUP_SYMBOL_LEN, sample_rate, duration),
            preamble: Preamble::new(PREAMBLE_SYMBOL_LEN, sample_rate, duration),
            symbols: Symbol::new(frequency, sample_rate, duration),
            stream_config,
        }
    }
}

pub struct AtherOutputStream {
    config: AtherStreamConfig,
    stream: AudioOutputStream<AudioTrack<f32>>,
}

impl AtherOutputStream {
    pub fn new(config: AtherStreamConfig, stream: AudioOutputStream<AudioTrack<f32>>) -> Self {
        Self { config, stream }
    }
}

impl AtherOutputStream {
    pub async fn write(&self, bits: &BitSlice) {
        self.write_prelude().await;
        let mut frames = vec![self.config.warmup.0.clone()];
        frames.extend(encode_packet(&self.config, bits));
        let track = AudioTrack::new(self.config.stream_config.clone(), frames.concat().into());
        self.stream.write(track).await;
    }

    pub async fn write_timeout(&self, bits: &BitSlice, timeout: Duration) {
        self.write_prelude().await;
        let mut frames = vec![self.config.warmup.0.clone()];
        frames.extend(encode_packet(&self.config, bits));
        let track = AudioTrack::new(self.config.stream_config.clone(), frames.concat().into());
        tokio::select! {
            _ = async {
                self.stream.write(track).await;
            } => {}
            _ = tokio::time::sleep(timeout) => {}
        };
    }

    async fn write_prelude(&self) {
        let mut prelude = self.config.warmup.0.to_vec();
        prelude.extend(self.config.preamble.0.interpolate_back().iter());
        prelude.extend(self.config.symbols.0 .0.interpolate_back().iter());
        prelude.extend(self.config.symbols.0 .0.interpolate_front().iter());
        let track = AudioTrack::new(self.config.stream_config.clone(), prelude.into());
        self.stream.write(track).await;
    }
}

fn encode_packet(config: &AtherStreamConfig, bits: &BitSlice) -> Vec<AudioSamples<f32>> {
    let mut shards = bits
        .chunks(PAYLOAD_BITS_LEN)
        .map(|chunk| {
            let length = DecodeToBytes::decode(&chunk.len().view_bits()[..LENGTH_BITS_LEN]);
            let payload = DecodeToBytes::decode(chunk);

            let mut shard = vec![];
            shard.extend(length);
            shard.extend(payload);
            shard.resize(LENGTH_BYTE_LEN + PAYLOAD_BYTE_LEN, 0u8);

            shard
        })
        .collect::<Vec<_>>();
    let data_shards = shards.len();
    let parity_shards = (data_shards as f32 * PARITY_RATIO) as usize;
    shards.resize(
        data_shards + parity_shards,
        vec![0u8; LENGTH_BYTE_LEN + PAYLOAD_BYTE_LEN],
    );

    let reed = ReedSolomon::new(data_shards, parity_shards).unwrap();
    reed.encode(&mut shards).unwrap();

    let conv = ConvCode::new(CONV_GENERATORS);
    shards
        .into_iter()
        .enumerate()
        .map(|(index, shard)| {
            let mut meta = bitvec![];
            meta.push(index == data_shards + parity_shards - 1);
            meta.push(index < data_shards);
            meta.extend(bitvec![0; META_RESERVED_BITS_LEN]);
            meta = conv.encode(&meta);

            let parity = PARITY_ALGORITHM.checksum(&shard) as usize;
            let parity = &parity.view_bits::<Lsb0>()[..PARITY_BITS_LEN];

            let mut header = bitvec![];
            header.extend(meta);
            header.extend(parity);

            let body = shard.encode();

            let mut samples: Vec<f32> = vec![];
            samples.extend(config.preamble.0.interpolate_back().iter());

            let header = header
                .encode(&config.symbols)
                .into_iter()
                .map(|symbol| symbol.0)
                .collect::<Vec<_>>()
                .concat();
            samples.extend(header.interpolate_back().iter());

            let body = body
                .encode(&config.symbols)
                .into_iter()
                .map(|symbol| symbol.0)
                .collect::<Vec<_>>()
                .concat();

            samples.extend(itertools::interleave(
                body[..body.len() / 2].iter(),
                body[body.len() / 2..].iter(),
            ));

            samples.into()
        })
        .collect::<Vec<_>>()
}

trait AtherSymbolEncoding {
    fn encode(&self, symbols: &(Symbol, Symbol)) -> Vec<Symbol>;
}

impl AtherSymbolEncoding for usize {
    fn encode(&self, symbols: &(Symbol, Symbol)) -> Vec<Symbol> {
        self.view_bits::<Lsb0>()
            .into_iter()
            .map(|bit| {
                if *bit {
                    symbols.1.clone()
                } else {
                    symbols.0.clone()
                }
            })
            .collect::<Vec<Symbol>>()
    }
}

impl AtherSymbolEncoding for BitSlice {
    fn encode(&self, symbols: &(Symbol, Symbol)) -> Vec<Symbol> {
        let mut samples = vec![];
        for bit in self {
            if *bit {
                samples.push(symbols.1.clone());
            } else {
                samples.push(symbols.0.clone());
            }
        }
        samples
    }
}

pub struct AtherInputStream {
    task: AtherInputTask,
    sender: UnboundedSender<AtherInputTaskCmd>,
}

impl AtherInputStream {
    pub fn new(config: AtherStreamConfig, mut stream: AudioInputStream<f32>) -> Self {
        let (sender, mut reciever) = mpsc::unbounded_channel();
        let task = Arc::new(Mutex::new(AtherInputTaskState::Pending));
        tokio::spawn({
            let task = task.clone();
            async move {
                let mut buf = (vec![], vec![]);
                while let Some(cmd) = reciever.recv().await {
                    match cmd {
                        AtherInputTaskCmd::Running => {
                            match decode_packet(&config, &mut stream, &mut buf).await {
                                Some(bits) => {
                                    let mut guard = task.lock().unwrap();
                                    match guard.take() {
                                        AtherInputTaskState::Running(waker) => {
                                            *guard = AtherInputTaskState::Completed(bits);
                                            waker.wake();
                                        }
                                        content => *guard = content,
                                    }
                                }
                                None => {
                                    buf.0.clear();
                                    buf.1.clear();
                                }
                            }
                        }
                        AtherInputTaskCmd::Suspended => {
                            stream.suspend();
                            let mut guard = task.lock().unwrap();
                            match guard.take() {
                                AtherInputTaskState::Running(waker) => {
                                    *guard = AtherInputTaskState::Suspended(None);
                                    waker.wake();
                                }
                                AtherInputTaskState::Completed(bits) => {
                                    *guard = AtherInputTaskState::Suspended(Some(bits));
                                }
                                content => *guard = content,
                            }
                        }
                        AtherInputTaskCmd::Resume => {
                            stream.resume();
                            let mut guard = task.lock().unwrap();
                            match guard.take() {
                                AtherInputTaskState::Suspended(bits) => {
                                    if let Some(bits) = bits {
                                        *guard = AtherInputTaskState::Completed(bits);
                                    } else {
                                        *guard = AtherInputTaskState::Pending;
                                    }
                                }
                                content => *guard = content,
                            }
                        }
                    }
                }
            }
        });
        Self { sender, task }
    }
}

enum AtherInputTaskCmd {
    Running,
    Suspended,
    Resume,
}

type AtherInputTask = Arc<Mutex<AtherInputTaskState>>;

enum AtherInputTaskState {
    Pending,
    Running(Waker),
    Completed(BitVec),
    Suspended(Option<BitVec>),
}

impl AtherInputTaskState {
    fn take(&mut self) -> AtherInputTaskState {
        mem::replace(self, AtherInputTaskState::Suspended(None))
    }
}

impl Stream for AtherInputStream {
    type Item = BitVec;

    fn poll_next(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        let mut guard = self.task.lock().unwrap();
        match guard.take() {
            AtherInputTaskState::Pending => {
                *guard = AtherInputTaskState::Running(cx.waker().clone());
                self.sender.send(AtherInputTaskCmd::Running).unwrap();
                Poll::Pending
            }
            AtherInputTaskState::Running(_) => {
                *guard = AtherInputTaskState::Running(cx.waker().clone());
                Poll::Pending
            }
            AtherInputTaskState::Completed(bits) => {
                *guard = AtherInputTaskState::Pending;
                Poll::Ready(Some(bits))
            }
            AtherInputTaskState::Suspended(bits) => {
                if let Some(bits) = bits {
                    *guard = AtherInputTaskState::Suspended(None);
                    Poll::Ready(Some(bits))
                } else {
                    Poll::Ready(None)
                }
            }
        }
    }
}

async fn decode_frame(
    config: &AtherStreamConfig,
    stream: &mut AudioInputStream<f32>,
    buf: &mut (Vec<f32>, Vec<f32>),
    metric: &AtherStreamMetric,
) -> Option<(BitVec, usize, Vec<u8>)> {
    let sample_rate = config.stream_config.sample_rate().0 as f32;
    let band_pass = (
        config.frequency as f32 - 1000.,
        config.frequency as f32 + 1000.,
    );
    let symbol_len = config.symbols.0 .0.len();
    let conv = ConvCode::new(CONV_GENERATORS);

    // println!("Decode frame...");
    decode_preamble(config, stream, buf).await?;

    let mut header = bitvec![];
    while header.len() < HEADER_BITS_LEN {
        if buf.0.len() >= symbol_len {
            let mut symbol = buf.0[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            header.push(value > 0.);
            buf.0 = buf.0.split_off(symbol_len);
            buf.1 = buf.1.split_off(symbol_len);
        } else {
            expand_buf(stream, buf).await?;
        }
    }

    // TODO: Add channel metric adjustment
    let mut body = (bitvec![], bitvec![]);
    while body.0.len() < BODY_BITS_LEN {
        if buf.0.len() >= symbol_len {
            let mut symbol = (
                buf.0[..symbol_len].to_owned(),
                buf.1[..symbol_len].to_owned(),
            );
            symbol.0.band_pass(sample_rate, band_pass);
            symbol.1.band_pass(sample_rate, band_pass);
            let symbol = decode_symbol(symbol, metric);

            let value = (
                signal::dot_product(&config.symbols.1 .0, &symbol.0),
                signal::dot_product(&config.symbols.1 .0, &symbol.1),
            );
            body.0.push(value.0 > 0.);
            body.1.push(value.1 > 0.);

            buf.0 = buf.0.split_off(symbol_len);
            buf.1 = buf.1.split_off(symbol_len);
        } else {
            expand_buf(stream, buf).await?;
        }
    }

    let meta = &header[..META_ENCODED_BITS_LEN];
    let (meta, _err) = conv.decode(meta);
    // println!("Receive meta: {}", meta);

    let parity = &header[META_ENCODED_BITS_LEN..];
    let parity = DecodeToInt::<usize>::decode(parity);
    // println!("Receive parity: {}", parity);

    let mut bits = bitvec![];
    bits.extend(&body.0);
    bits.extend(&body.1);
    let body = DecodeToBytes::decode(&bits);
    // println!("Receive body: {:?}", body);

    Some((meta, parity, body))
}

fn decode_symbol(symbol: (Vec<f32>, Vec<f32>), metric: &AtherStreamMetric) -> (Vec<f32>, Vec<f32>) {
    let full = symbol.0.len() * 2 - 1;
    let symbol_fft = (signal::rfft(&symbol.0, full), signal::rfft(&symbol.1, full));

    let factor = (
        (
            metric.1 .1 / (metric.0 .0 * metric.1 .1 - metric.1 .0 * metric.0 .1),
            -metric.0 .1 / (metric.0 .0 * metric.1 .1 - metric.1 .0 * metric.0 .1),
        ),
        (
            metric.1 .0 / (metric.0 .1 * metric.1 .0 - metric.1 .1 * metric.0 .0),
            -metric.0 .0 / (metric.0 .1 * metric.1 .0 - metric.1 .1 * metric.0 .0),
        ),
    );

    let symbol_fft = (
        symbol_fft
            .0
            .iter()
            .zip(symbol_fft.1.iter())
            .map(|item| factor.0 .0 * item.0 + factor.0 .1 * item.1)
            .collect::<Vec<_>>(),
        symbol_fft
            .0
            .iter()
            .zip(symbol_fft.1.iter())
            .map(|item| factor.1 .0 * item.0 + factor.1 .1 * item.1)
            .collect::<Vec<_>>(),
    );

    let symbol_ifft = (
        signal::irfft(&symbol_fft.0, full),
        signal::irfft(&symbol_fft.1, full),
    );

    (
        symbol_ifft
            .0
            .iter()
            .map(|&item| item / full as f32)
            .collect(),
        symbol_ifft
            .1
            .iter()
            .map(|&item| item / full as f32)
            .collect(),
    )
}

async fn expand_buf(
    stream: &mut AudioInputStream<f32>,
    buf: &mut (Vec<f32>, Vec<f32>),
) -> Option<()> {
    let sample = stream.next().await?;
    let sample = sample.deinterleave();
    buf.0.extend(sample.0.iter());
    buf.1.extend(sample.1.iter());
    Some(())
}

async fn decode_preamble(
    config: &AtherStreamConfig,
    stream: &mut AudioInputStream<f32>,
    buf: &mut (Vec<f32>, Vec<f32>),
) -> Option<()> {
    let preamble_len = config.preamble.0.len();
    loop {
        if buf.0.len() >= preamble_len {
            let (index, value) = signal::synchronize(&config.preamble.0, &buf.0);
            if value > PREAMBLE_CORR_THRESHOLD {
                if (index + preamble_len as isize) < (buf.0.len() as isize) {
                    buf.0 = buf.0.split_off((index + preamble_len as isize) as usize);
                    buf.1 = buf.1.split_off((index + preamble_len as isize) as usize);
                    break;
                }
            } else {
                buf.0 = buf.0.split_off(buf.0.len() - preamble_len);
                buf.1 = buf.1.split_off(buf.1.len() - preamble_len);
            }
        }
        expand_buf(stream, buf).await?;
    }
    Some(())
}

type AtherStreamMetric = ((Complex<f32>, Complex<f32>), (Complex<f32>, Complex<f32>));

async fn decode_prelude(
    config: &AtherStreamConfig,
    stream: &mut AudioInputStream<f32>,
    buf: &mut (Vec<f32>, Vec<f32>),
) -> Option<AtherStreamMetric> {
    let sample_rate = config.stream_config.sample_rate().0 as f32;
    let band_pass = (
        config.frequency as f32 - 1000.,
        config.frequency as f32 + 1000.,
    );
    let symbol_len = config.symbols.0 .0.len();

    decode_preamble(config, stream, buf).await?;

    let mut prelude = ((vec![], vec![]), (vec![], vec![]));
    loop {
        if buf.0.len() >= symbol_len {
            let mut symbol = (
                buf.0[..symbol_len].to_owned(),
                buf.1[..symbol_len].to_owned(),
            );
            symbol.0.band_pass(sample_rate, band_pass);
            symbol.1.band_pass(sample_rate, band_pass);
            prelude.0 = symbol;

            buf.0 = buf.0.split_off(symbol_len);
            buf.1 = buf.1.split_off(symbol_len);
            break;
        } else {
            expand_buf(stream, buf).await?;
        }
    }

    loop {
        if buf.0.len() >= symbol_len {
            let mut symbol = (
                buf.0[..symbol_len].to_owned(),
                buf.1[..symbol_len].to_owned(),
            );
            symbol.0.band_pass(sample_rate, band_pass);
            symbol.1.band_pass(sample_rate, band_pass);
            prelude.1 = symbol;

            buf.0 = buf.0.split_off(symbol_len);
            buf.1 = buf.1.split_off(symbol_len);
            break;
        } else {
            expand_buf(stream, buf).await?;
        }
    }

    Some((
        (
            signal::characterize(&prelude.0 .0, &config.symbols.0 .0),
            signal::characterize(&prelude.0 .1, &config.symbols.0 .0),
        ),
        (
            signal::characterize(&prelude.1 .0, &config.symbols.0 .0),
            signal::characterize(&prelude.1 .1, &config.symbols.0 .0),
        ),
    ))
}

async fn decode_packet(
    config: &AtherStreamConfig,
    stream: &mut AudioInputStream<f32>,
    buf: &mut (Vec<f32>, Vec<f32>),
) -> Option<BitVec> {
    let metric = decode_prelude(config, stream, buf).await?;

    let mut shards = vec![];
    let (mut data_shards, mut parity_shards) = (0, 0);
    loop {
        match decode_frame(config, stream, buf, &metric).await {
            Some((meta, parity, body)) => {
                let recieved = PARITY_ALGORITHM.checksum(&body) as usize;
                if recieved == parity {
                    shards.push(Some(body));
                } else {
                    shards.push(None);
                }

                let r#type = meta[META_FLAG_BITS_LEN + META_TYPE_BITS_LEN - 1];
                if r#type {
                    data_shards += 1;
                } else {
                    parity_shards += 1;
                }

                let flag = meta[META_FLAG_BITS_LEN - 1];
                eprintln!(
                    "Receive frame {} (flag {}, type {}, ok {})",
                    shards.len(),
                    flag,
                    r#type,
                    recieved == parity
                );
                if flag {
                    break;
                }
            }
            None => return None,
        }
    }

    eprintln!(
        "Reconstruct shards from {} data shards and {} parity shards",
        data_shards, parity_shards
    );
    let reed = ReedSolomon::new(data_shards, parity_shards).unwrap();
    reed.reconstruct(&mut shards).unwrap();

    let mut bits = bitvec![];
    for (index, shard) in shards.into_iter().flatten().take(data_shards).enumerate() {
        let mut length = DecodeToInt::<usize>::decode(&shard[..LENGTH_BYTE_LEN].encode());
        if length > PAYLOAD_BITS_LEN {
            length = PAYLOAD_BITS_LEN;
        }
        eprintln!("Decode frame {} (length {})", index, length);
        let payload = &shard[LENGTH_BYTE_LEN..];
        bits.extend(&payload.encode()[..length]);
    }

    Some(bits)
}

impl ContinuousStream for AtherInputStream {
    fn resume(&self) {
        self.sender.send(AtherInputTaskCmd::Resume).unwrap();
    }

    fn suspend(&self) {
        self.sender.send(AtherInputTaskCmd::Suspended).unwrap();
    }
}
