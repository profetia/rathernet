//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form
//! of audio signals in the method of phase shift keying (PSK). The stream is composed of a header,
//! consisting of a preamble (PREAMBLE_LEN symbols) and a body (BODY_LEN symbols). The body is
//! composed of a length field (LENGTH_LEN symbols) and a payload (PAYLOAD_LEN symbols). The body
//! is encoded using a convolutional code with a constraint length of CONV_ORDER and generators of
//! CONV_GENERATORS. The payload is padded with zeros to make it a multiple of PAYLOAD_LEN symbols,
//! and its length is encoded in the length field.

use super::{
    signal::{self, BandPass},
    Preamble, Symbol, Warmup,
};
use crate::raudio::{
    AudioInputStream, AudioOutputStream, AudioSamples, AudioTrack, ContinuousStream,
};
use bitvec::prelude::*;
use cpal::SupportedStreamConfig;
use raptor_code::{SourceBlockDecoder, SourceBlockEncoder};
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
const PREAMBLE_LEN: usize = 64; // 8 | 16 | 32 | 64

const CORR_THRESHOLD: f32 = 0.15;

const PAYLOAD_LEN: usize = 8;
const PAYLOAD_BITS: usize = PAYLOAD_LEN * 8;
const LENGTH_LEN: usize = 1;
const BODY_LEN: usize = LENGTH_LEN + PAYLOAD_LEN * 2 - 1;
const BODY_BITS: usize = BODY_LEN * 9;

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
    let mut frames = vec![];
    for chunk in bits.chunks(PAYLOAD_BITS) {
        let mut source = vec![];
        source.push(chunk.len() as u8);

        let mut payload = chunk.to_owned();
        if payload.len() < PAYLOAD_BITS {
            payload.resize(PAYLOAD_BITS, false);
        }
        for chunk in payload.chunks(8) {
            source.push(chunk.decode() as u8);
        }

        let mut encoder = SourceBlockEncoder::new(&source, PAYLOAD_LEN + LENGTH_LEN);

        let mut body = bitvec![];
        body.reserve(BODY_BITS);
        for esi in 0..BODY_LEN as u32 {
            let byte = encoder.fountain(esi)[0];
            let bits = byte.view_bits::<Lsb0>();
            let parity = bits[0..8].iter().fold(0, |acc, bit| acc ^ *bit as u8);
            body.push(parity != 0);
            body.extend(&bits[0..8]);
        }

        frames.push(
            [
                config.preamble.0.clone(),
                body.encode(config.symbols.clone()).into_iter().collect(),
            ]
            .concat()
            .into(),
        );
    }
    if bits.len() % PAYLOAD_BITS == 0 {
        let mut source = vec![];
        source.push(0_u8);

        let payload = bitvec![0; PAYLOAD_BITS];
        for chunk in payload.chunks(8) {
            source.push(chunk.decode() as u8);
        }

        let mut encoder = SourceBlockEncoder::new(&source, PAYLOAD_LEN + LENGTH_LEN);

        let mut body = bitvec![];
        body.reserve(BODY_BITS);
        for esi in 0..BODY_LEN as u32 {
            let byte = encoder.fountain(esi)[0];
            let bits = byte.view_bits::<Lsb0>();
            let parity = bits[0..8].iter().fold(0, |acc, bit| acc ^ *bit as u8);
            body.push(parity != 0);
            body.extend(&bits[0..8]);
        }

        frames.push(
            [
                config.preamble.0.clone(),
                body.encode(config.symbols.clone()).into_iter().collect(),
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

    println!("Start decode a frame");

    loop {
        if buf.len() >= preamble_len {
            let (index, value) = signal::synchronize(&config.preamble.0, buf);
            if value > CORR_THRESHOLD {
                if (index + preamble_len as isize) < (buf.len() as isize) {
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

    let mut body = bitvec![];
    body.reserve(BODY_BITS);
    while body.len() < BODY_BITS {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            if value > 0. {
                body.push(true);
                println!("[{}] symbol 1 - {:?}", body.len(), value);
            } else {
                body.push(false);
                println!("[{}] symbol 0 - {:?}", body.len(), value);
            }

            *buf = buf.split_off(symbol_len);
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    let mut decoder = SourceBlockDecoder::new(PAYLOAD_LEN + LENGTH_LEN);
    for (esi, chunk) in body
        .chunks(9)
        .filter(|chunk| {
            let parity = chunk[0] as u8;
            let recieved = chunk[1..].iter().fold(0, |acc, bit| acc ^ *bit as u8);
            println!("Parity: {}, got {}", parity, recieved);
            parity == recieved
        })
        .enumerate()
    {
        decoder.push_encoding_symbol(&[(&chunk[1..]).decode() as u8], esi as u32);
        if decoder.fully_specified() {
            break;
        }
    }

    let body = decoder.decode(PAYLOAD_LEN + LENGTH_LEN).unwrap();
    let length = if body[0] <= PAYLOAD_BITS as u8 {
        body[0] as usize
    } else {
        PAYLOAD_BITS
    };
    println!("Found length {}", length);

    let payload = body[1..]
        .iter()
        .flat_map(|byte| byte.view_bits::<Lsb0>())
        .collect::<BitVec>()[..length]
        .to_owned();

    println!("Found payload {}", &payload[..]);

    Some(payload)
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
                println!("Recieved {}, New {}", bits.len(), len);
                eprintln!("Recieved {}, New {}", bits.len(), len);
                if len != PAYLOAD_BITS {
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
