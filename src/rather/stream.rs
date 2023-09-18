use super::{Frame, Header, Symbol};
use crate::raudio::AudioOutputStream;
use bitvec::slice::BitSlice;
use cpal::SupportedStreamConfig;
use std::{f32::consts::PI, time::Duration};

pub struct AtherStreamConfig {
    pub frequency: u32,
    pub bit_rate: u32,
    pub header: Header,
    pub symbols: (Symbol, Symbol),
    pub stream_config: SupportedStreamConfig,
}

impl AtherStreamConfig {
    pub fn new(frequency: u32, bit_rate: u32, stream_config: SupportedStreamConfig) -> Self {
        let elapse = 1.0 / bit_rate as f32;
        let sample_rate = stream_config.sample_rate().0;
        let symbol_zero = (0..(elapse * sample_rate as f32) as usize)
            .map(|item| {
                let t = item as f32 * 2.0 * PI / sample_rate as f32;
                (t * frequency as f32).sin()
            })
            .collect::<Symbol>();
        let symbol_one = symbol_zero.iter().map(|item| -item).collect::<Symbol>();

        let header_len = 10 * (elapse * sample_rate as f32) as u32;
        let header = (0..header_len)
            .map(|item| {
                if item < header_len / 2 {
                    2000.0 + item as f32 * 6000.0 / (header_len / 2) as f32
                } else {
                    8000.0 - (item - header_len / 2) as f32 * 6000.0 / (header_len / 2) as f32
                }
            })
            .scan(0.0f32, |acc, item| {
                *acc += item / sample_rate as f32;
                Some(*acc)
            })
            .map(|item| (item * 2.0 * PI).sin())
            .collect::<Header>();

        Self {
            frequency,
            bit_rate,
            header,
            symbols: (symbol_zero, symbol_one),
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
