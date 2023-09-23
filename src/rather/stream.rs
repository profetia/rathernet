//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form
//! of audio signals in the method of phase shift keying (PSK). The stream is composed of a header,
//! consisting of a preamble (32 symbols) and a length field (15 symbols) covering 11 bits protected
//! by Hamming(15, 11), a body (n symbols with maximum 2047 symbols), which is the encoded by
//! convolutional codes ((5, 7), 1/2). Decoding the body will result in a bit stream, which is
//! expected to be 64 bits long. An ununified frame indicates the end of a packet.

// TODO: implement the parity of length field and checksum field

use super::{
    signal::{self, BandPass},
    Preamble, Symbol, Warmup,
};
use crate::raudio::{
    AudioInputStream, AudioOutputStream, AudioSamples, AudioTrack, ContinuousStream,
};
use bitvec::prelude::*;
use cpal::SupportedStreamConfig;
use std::{
    fs, mem,
    pin::Pin,
    slice::Chunks,
    sync::{Arc, Mutex},
    task::{self, Poll, Waker},
    time::Duration,
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::{Stream, StreamExt};

const WARMUP_LEN: usize = 8;
const PREAMBLE_LEN: usize = 32; // 8 | 16
const LENGTH_LEN: usize = 7; // 5 | 6
const PAYLOAD_LEN: usize = (1 << LENGTH_LEN) - 1;
const CORR_THRESHOLD: f32 = 0.15;

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
        // fs::write("ref.json", format!("{:?}", frames.concat())).unwrap();
        let track = AudioTrack::new(self.config.stream_config.clone(), frames.concat().into());
        // track.write_to_file("ref.wav").unwrap();
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
    for chunk in bits.chunks(PAYLOAD_LEN) {
        let payload = chunk.encode(config.symbols.clone());
        let length = chunk.len().encode(config.symbols.clone())[..LENGTH_LEN].to_owned();

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
        let length = 0usize.encode(config.symbols.clone())[..LENGTH_LEN].to_owned();

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

trait AtherEncoding {
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol>;
}

impl AtherEncoding for usize {
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

impl AtherEncoding for BitSlice {
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

async fn decode_packet(
    // async fn decode_frame(
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

    println!("Start decode");

    loop {
        println!(
            "Looping on the preamble {}, expect {}",
            buf.len(),
            preamble_len
        );
        if buf.len() >= preamble_len {
            let (index, value) = signal::synchronize(&config.preamble.0, buf);
            println!("Got sync index {}", index);
            if value > CORR_THRESHOLD {
                println!("Got index {} with {}", index, value);
                if (index + preamble_len as isize) < (buf.len() as isize) {
                    // println!("Before preamble {:?}", buf[..index as usize].to_owned());
                    // println!(
                    //     "Preamble {:?}",
                    //     buf[index as usize..(index + preamble_len as isize) as usize].to_owned()
                    // );
                    *buf = buf.split_off((index + preamble_len as isize) as usize);
                    break;
                } else {
                    println!("Failed to find a start, len {}", buf.len());
                }
            } else {
                println!(
                    "Failed to conform the threshold, got {}, len {}",
                    value,
                    buf.len()
                );
                *buf = buf.split_off(buf.len() - preamble_len);
            }
        }

        println!("Wait for more data");
        match stream.next().await {
            Some(sample) => buf.extend(sample.iter()),
            None => return None,
        }
        println!("Done");
    }

    println!("Preamble found");
    // println!("Remaining data {:?}", buf);

    let (mut length, mut index) = (0usize, 0usize);
    while index < LENGTH_LEN {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            if value > 0. {
                length += 1 << index;
                // println!("[{}] symbol 1 - {:?}", index, value);
                println!("[{}] symbol 1 - {:?}", index, symbol);
            } else {
                // println!("[{}] symbol 0 - {:?}", index, value);
                println!("[{}] symbol 0 - {:?}", index, symbol);
            }

            *buf = buf.split_off(symbol_len);
            index += 1;
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    println!("Found length {}", length);

    let (mut bits, mut index) = (bitvec![], 0usize);
    while index < length {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(sample_rate, band_pass);
            let value = signal::dot_product(&config.symbols.1 .0, &symbol);
            if value > 0. {
                bits.push(true);
                // println!("[{}] symbol 1 - {:?}", index, value);
                println!("[{}] symbol 1 - {:?}", index, symbol);
            } else {
                bits.push(false);
                // println!("[{}] symbol 0 - {:?}", index, value);
                println!("[{}] symbol 0 - {:?}", index, symbol);
            }

            *buf = buf.split_off(symbol_len);
            index += 1;
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    Some(bits)
}

// async fn decode_packet(
//     config: &AtherStreamConfig,
//     stream: &Arc<sync::Mutex<AudioInputStream<f32>>>,
//     buf: &mut Vec<f32>,
// ) -> Option<BitVec> {
//     let mut bits = bitvec![];
//     loop {
//         match decode_frame(config, stream, buf).await {
//             Some(frame) => {
//                 if frame.is_empty() {
//                     break;
//                 } else {
//                     bits.extend(frame);
//                 }
//             }
//             None => return None,
//         }
//     }
//     Some(bits)
// }

impl ContinuousStream for AtherInputStream {
    fn resume(&self) {
        self.sender.send(AtherInputTaskCmd::Resume).unwrap();
    }

    fn suspend(&self) {
        self.sender.send(AtherInputTaskCmd::Suspended).unwrap();
    }
}
