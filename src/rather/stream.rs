use super::{
    builtin::{
        LENGTH_BITS_LEN, PAYLOAD_BITS_LEN, PREAMBLE_CORR_THRESHOLD, PREAMBLE_SYMBOL_LEN,
        WARMUP_SYMBOL_LEN,
    },
    encode::DecodeToInt,
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

#[derive(Debug, Clone)]
pub struct AtherStreamConfig {
    pub bit_rate: u32,
    pub warmup: Warmup,
    pub preamble: Preamble,
    pub frequencies: Vec<u32>,
    pub symbols: Vec<(Symbol, Symbol)>,
    pub stream_config: SupportedStreamConfig,
}

impl AtherStreamConfig {
    pub fn new(
        frequency_range: (u32, u32),
        bit_rate: u32,
        stream_config: SupportedStreamConfig,
    ) -> Self {
        let duration = 1.0 / bit_rate as f32;
        let sample_rate = stream_config.sample_rate().0;

        let frequencies = (frequency_range.0..frequency_range.1)
            .step_by(bit_rate as usize)
            .collect::<Vec<u32>>();
        let symbols = frequencies
            .iter()
            .map(|&frequency| Symbol::new(frequency, sample_rate, duration))
            .collect::<Vec<(Symbol, Symbol)>>();

        Self {
            bit_rate,
            warmup: Warmup::new(WARMUP_SYMBOL_LEN, sample_rate, duration),
            preamble: Preamble::new(PREAMBLE_SYMBOL_LEN, sample_rate, duration),
            frequencies,
            symbols,
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
        let mut frame = vec![self.config.warmup.0.clone()];
        frame.push(encode_frame(&self.config, bits));
        let track = AudioTrack::new(self.config.stream_config.clone(), frame.concat().into());
        self.stream.write(track).await;
    }

    pub async fn write_timeout(&self, bits: &BitSlice, timeout: Duration) {
        let mut frame = vec![self.config.warmup.0.clone()];
        frame.push(encode_frame(&self.config, bits));
        let track = AudioTrack::new(self.config.stream_config.clone(), frame.concat().into());
        self.stream.write_timeout(track, timeout).await;
    }
}

fn encode_frame(config: &AtherStreamConfig, bits: &BitSlice) -> AudioSamples<f32> {
    assert!(bits.len() <= PAYLOAD_BITS_LEN);

    let default_symbols = &config.symbols[0];
    let symbol_len = default_symbols.0 .0.len();
    let ofdm_len = config.symbols.len();

    let mut samples = vec![];
    samples.push(config.preamble.0.clone());
    let length = bits.len().encode(default_symbols)[..LENGTH_BITS_LEN].to_owned();
    samples.extend(length.into_iter().map(|symbol| symbol.0));

    for chunk in bits.chunks(ofdm_len) {
        let mut sample = vec![0f32; symbol_len];
        for (bit, symbols) in chunk.iter().zip(config.symbols.iter()) {
            let symbol = if *bit { &symbols.1 } else { &symbols.0 };
            for (sample_item, symbol_item) in sample.iter_mut().zip(symbol.0.iter()) {
                *sample_item += symbol_item;
            }
        }
        samples.push(sample.into());
    }

    samples.concat().into()
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
                let mut buf = vec![];
                while let Some(cmd) = reciever.recv().await {
                    match cmd {
                        AtherInputTaskCmd::Running => {
                            match decode_frame(&config, &mut stream, &mut buf).await {
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

impl AtherInputStream {
    pub async fn read(&mut self) -> BitVec {
        let mut result = bitvec![];
        while let Some(data) = self.next().await {
            result.extend(data.iter())
        }
        result
    }

    pub async fn read_timeout(&mut self, timeout: Duration) -> BitVec {
        let mut result = bitvec![];
        tokio::select! {
            _ = async {
                while let Some(data) = self.next().await {
                    result.extend(data.iter());
                }
            } => {},
            _ = tokio::time::sleep(timeout) => {},
        };
        result
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
    let preamble_len = config.preamble.0.len();
    let default_frequency = &config.frequencies[0];
    let default_symbols = &config.symbols[0];
    let symbol_len = default_symbols.0 .0.len();

    // println!("Decode frame...");

    loop {
        if buf.len() >= preamble_len {
            let (index, value) = signal::synchronize(&config.preamble.0, buf);
            if value > PREAMBLE_CORR_THRESHOLD {
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

    let mut length = bitvec![];
    while length.len() < LENGTH_BITS_LEN {
        if buf.len() >= symbol_len {
            let mut symbol = buf[..symbol_len].to_owned();
            symbol.band_pass(
                sample_rate,
                (
                    (default_frequency - config.bit_rate / 2) as f32,
                    (default_frequency + config.bit_rate / 2) as f32,
                ),
            );
            let value = signal::dot_product(&default_symbols.1 .0, &symbol);
            length.push(value > 0.);
            *buf = buf.split_off(symbol_len);
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    let length = length.decode();

    let mut payload = bitvec![];
    while payload.len() < length {
        if buf.len() >= symbol_len {
            for (frequency, symbols) in config.frequencies.iter().zip(config.symbols.iter()) {
                let mut symbol = buf[..symbol_len].to_owned();
                symbol.band_pass(
                    sample_rate,
                    (
                        (frequency - config.bit_rate / 2) as f32,
                        (frequency + config.bit_rate / 2) as f32,
                    ),
                );
                let value = signal::dot_product(&symbols.1 .0, &symbol);
                payload.push(value > 0.);
            }

            *buf = buf.split_off(symbol_len);
        } else {
            match stream.next().await {
                Some(sample) => buf.extend(sample.iter()),
                None => return None,
            }
        }
    }

    Some(payload[..length].to_owned())
}

impl ContinuousStream for AtherInputStream {
    fn resume(&self) {
        self.sender.send(AtherInputTaskCmd::Resume).unwrap();
    }

    fn suspend(&self) {
        self.sender.send(AtherInputTaskCmd::Suspended).unwrap();
    }
}
