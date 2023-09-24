//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form
//! of audio signals in the method of phase shift keying (PSK). The stream is composed of a header,
//! consisting of a preamble (PREAMBLE_LEN symbols) and a length field (LENGTH_FIELD_LEN symbols)
//! covering LENGTH_LEN bits protected by convolutional code ([171, 133], 1/2), a payload
//! (PAYLOAD_FIELD_LENGTH symbols), covering PAYLOAD_LEN bits protected by the same convolutional
//! code. Decoding the payload will result in a bit stream, which is expected to be PAYLOAD_LEN
//! bits long. An ununified frame indicates the end of a packet.

use super::{
    conv::ConvCode,
    signal::{self, BandPass},
    Preamble, Symbol, Warmup,
};
use crate::raudio::{
    AudioInputStream, AudioOutputStream, AudioSamples, AudioTrack, ContinuousStream,
};
use bitvec::prelude::*;
use cpal::SupportedStreamConfig;
use std::{
    mem,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{self, Poll, Waker},
    time::Duration,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::{Stream, StreamExt};

const WARMUP_LEN: usize = 8;
const PREAMBLE_LEN: usize = 32; // 8 | 16 | 32
const CORR_THRESHOLD: f32 = 0.15;
const CONV_GENERATORS: &[usize] = &[171, 133];
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

const PAYLOAD_LEN: usize = 64; // 64 | 128
const PAYLOAD_FIELD_LEN: usize = (PAYLOAD_LEN + CONV_ORDER) * CONV_FACTOR;
const LENGTH_LEN: usize = PAYLOAD_FIELD_LEN.ilog2() as usize + 1;
const LENGTH_FIELD_LEN: usize = (LENGTH_LEN + CONV_ORDER) * CONV_FACTOR;

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
            warmup: Warmup::new(WARMUP_LEN, sample_rate, duration),
            preamble: Preamble::new(PREAMBLE_LEN, sample_rate, duration),
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
        let mut frames = vec![self.config.warmup.0.clone()];
        frames.extend(encode_packet(&self.config, bits));
        let track = AudioTrack::new(self.config.stream_config.clone(), frames.concat().into());
        self.stream.write(track).await;
    }

    pub async fn write_timeout(&self, bits: &BitSlice, timeout: Duration) {
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
}

fn encode_packet(config: &AtherStreamConfig, bits: &BitSlice) -> Vec<AudioSamples<f32>> {
    let conv = ConvCode::new(CONV_GENERATORS);
    let mut frames = vec![];
    for chunk in bits.chunks(PAYLOAD_LEN) {
        let payload = conv.encode(chunk).encode(config.symbols.clone());
        let length = conv
            .encode(&payload.len().view_bits()[..LENGTH_LEN])
            .encode(config.symbols.clone())
            .to_owned();

        frames.push(
            [
                config.preamble.0.clone(),
                length.into_iter().collect(),
                payload.into_iter().collect(),
            ]
            .concat()
            .into(),
        );
    }
    if bits.len() % PAYLOAD_LEN == 0 {
        let payload: Vec<Symbol> = vec![];
        let length = conv
            .encode(&payload.len().view_bits()[..LENGTH_LEN])
            .encode(config.symbols.clone())
            .to_owned();

        frames.push(
            [
                config.preamble.0.clone(),
                length.into_iter().collect(),
                payload.into_iter().collect(),
            ]
            .concat()
            .into(),
        );
    }

    frames
}

trait AtherSymbolEncoding {
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol>;
}

impl AtherSymbolEncoding for usize {
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol> {
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
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol> {
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
                let mut buf = vec![];
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
                                    buf.clear();
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
    buf: &mut Vec<f32>,
) -> Option<BitVec> {
    let sample_rate = config.stream_config.sample_rate().0 as f32;
    let band_pass = (
        config.frequency as f32 - 1000.,
        config.frequency as f32 + 1000.,
    );
    let preamble_len = config.preamble.0.len();
    let symbol_len = config.symbols.0 .0.len();
    let conv = ConvCode::new(CONV_GENERATORS);

    println!("Start decode a frame");

    loop {
        if buf.len() >= preamble_len {
            let (index, value) = signal::synchronize(&config.preamble.0, buf);
            if value > CORR_THRESHOLD {
                if (index + preamble_len as isize) < (buf.len() as isize) {
                    // println!("Before preamble {:?}", buf[..index as usize].to_owned());
                    // println!(
                    //     "Preamble {:?}",
                    //     buf[index as usize..(index + preamble_len as isize) as usize].to_owned()
                    // );
                    *buf = buf.split_off((index + preamble_len as isize) as usize);
                    break;
                }
            } else {
                *buf = buf.split_off(buf.len() - preamble_len);
            }
        }
        match stream.next().await {
            Some(sample) => buf.extend(sample.iter()),
            None => return None,
        }
    }

    println!("Preamble found");

    let mut length = bitvec![];
    while length.len() < LENGTH_FIELD_LEN {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            if value > 0. {
                length.push(true);
                println!("[{}] symbol 1 - {:?}", length.len(), value);
            } else {
                length.push(false);
                println!("[{}] symbol 0 - {:?}", length.len(), value);
            }

            *buf = buf.split_off(symbol_len);
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    let (length, err) = conv.decode(&length);
    let length = length.decode();

    println!("Found length {} with {} errors", length, err);

    let mut payload = bitvec![];
    while payload.len() < length {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            if value > 0. {
                payload.push(true);
                println!("[{}] symbol 1 - {:?}", payload.len(), value);
            } else {
                payload.push(false);
                println!("[{}] symbol 0 - {:?}", payload.len(), value);
            }

            *buf = buf.split_off(symbol_len);
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    let (bits, err) = conv.decode(&payload);
    println!("Payload decoded with {} errors", err);

    Some(bits)
}

async fn decode_packet(
    config: &AtherStreamConfig,
    stream: &mut AudioInputStream<f32>,
    buf: &mut Vec<f32>,
) -> Option<BitVec> {
    let mut bits = bitvec![];
    loop {
        match decode_frame(config, stream, buf).await {
            Some(frame) => {
                let len = frame.len();
                bits.extend(frame);
                eprintln!("Recieved {}, New {}", bits.len(), len);
                if len != PAYLOAD_LEN {
                    break;
                }
            }
            None => return None,
        }
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

trait AtherBitDecoding {
    fn decode(&self) -> usize;
}

impl AtherBitDecoding for BitVec {
    fn decode(&self) -> usize {
        let mut result = 0usize;
        for (index, bit) in self.iter().enumerate() {
            if *bit {
                result |= 1 << index;
            }
        }
        result
    }
}

impl AtherBitDecoding for &BitSlice {
    fn decode(&self) -> usize {
        let mut result = 0usize;
        for (index, bit) in self.iter().enumerate() {
            if *bit {
                result |= 1 << index;
            }
        }
        result
    }
}
