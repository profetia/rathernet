//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form of
//! audio signals in the method of phase shift keying (PSK). The stream is composed of a header
//! (8 symbols), a length field (7 symbols with 1 parity symbol), a body (n symbols with
//! maximum 128 symbols) and a checksum field (8 symbols). The header is used to identify the
//! start of a stream. The length field is used to indicate the length of the body. The checksum
//! field is used to verify the integrity of the stream. The body is the actual data to be sent.

// TODO: implement length field and checksum field

use super::{frame::Header, Body, Frame, Preamble, Symbol};
use crate::raudio::AudioOutputStream;
use bitvec::prelude::*;
use cpal::SupportedStreamConfig;
use std::time::Duration;

pub struct AtherStreamConfig {
    pub frequency: u32,
    pub bit_rate: u32,
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
            preamble: Preamble::new(sample_rate, duration),
            symbols: Symbol::new(frequency, sample_rate, duration),
            stream_config,
        }
    }
}

pub struct AtherOutputStream {
    config: AtherStreamConfig,
    stream: AudioOutputStream<Frame>,
}

impl AtherOutputStream {
    pub fn new(config: AtherStreamConfig, stream: AudioOutputStream<Frame>) -> Self {
        Self { config, stream }
    }
}

impl AtherOutputStream {
    fn encode(&self, bits: &BitSlice) -> Vec<Frame> {
        let mut frames = vec![];
        for chunk in bits.chunks(128) {
            let payload = chunk.encode(self.config.symbols.clone());
            let length = (bits.len() as u8).encode(self.config.symbols.clone());

            frames.push(Frame::new(
                self.config.stream_config.clone(),
                Header::new(self.config.preamble.clone(), length),
                Body::new(payload),
            ));
        }

        frames
    }

    pub async fn write(&self, bits: &BitSlice) {
        let frames = self.encode(bits);
        for frame in frames {
            self.stream.write(frame).await;
        }
    }

    pub async fn write_timeout(&self, bits: &BitSlice, timeout: Duration) {
        let frames = self.encode(bits);
        tokio::select! {
            _ = async {
                for frame in frames {
                    self.stream.write(frame).await;
                }
            } => {}
            _ = tokio::time::sleep(timeout) => {}
        };
    }
}

trait AtherEncoding {
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol>;
}

impl AtherEncoding for u8 {
    fn encode(&self, symbols: (Symbol, Symbol)) -> Vec<Symbol> {
        self.view_bits::<Lsb0>()
            .into_iter()
            .take(7)
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
