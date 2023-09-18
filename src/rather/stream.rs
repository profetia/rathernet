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
