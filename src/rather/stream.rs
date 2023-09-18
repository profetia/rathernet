//! # Rather Streams
//! Rather streams are used to send and receive data on an ather. The data is encoded in the form of
//! audio signals in the method of phase shift keying (PSK). The stream is composed of a header
//! (8 symbols), a length field (7 symbols with 1 parity symbol), a body (n symbols with
//! maximum 128 symbols) and a checksum field (8 symbols). The header is used to identify the
//! start of a stream. The length field is used to indicate the length of the body. The checksum
//! field is used to verify the integrity of the stream. The body is the actual data to be sent.

// TODO: implement length field and checksum field

use super::{Frame, Header, Symbol};
use crate::raudio::AudioOutputStream;
use bitvec::slice::BitSlice;
use cpal::SupportedStreamConfig;
use std::time::Duration;

pub struct AtherStreamConfig {
    pub frequency: u32,
    pub bit_rate: u32,
    pub header: Header,
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
            header: Header::new(sample_rate, duration),
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
    fn encode(&self, bits: &BitSlice) -> Frame {
        let mut body = vec![];
        for bit in bits {
            if *bit {
                body.push(self.config.symbols.1.clone());
            } else {
                body.push(self.config.symbols.0.clone());
            }
        }
        Frame::new(
            self.config.stream_config.clone(),
            self.config.header.clone(),
            body,
        )
    }

    pub async fn write(&self, bits: &BitSlice) {
        let frame = self.encode(bits);
        self.stream.write(frame).await;
    }

    pub async fn write_timeout(&self, bits: &BitSlice, timeout: Duration) {
        let frame = self.encode(bits);
        self.stream.write_timeout(frame, timeout).await;
    }
}
