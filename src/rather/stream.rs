use super::{Frame, Header, Symbol};
use crate::raudio::AudioOutputStream;
use cpal::SupportedStreamConfig;
use std::f32::consts::PI;

pub struct AtherStreamConfig {
    pub frequency: u32,
    pub bit_rate: u32,
    pub header: Header,
    pub symbols: (Symbol, Symbol),
    pub stream_config: SupportedStreamConfig,
}

impl AtherStreamConfig {
    pub fn new(stream_config: SupportedStreamConfig) -> Self {
        let frequency = 10000u32;
        let bit_rate = 1000u32;
        let header_len = 480u32;

        let elapse = 1.0 / bit_rate as f32;
        let sample_rate = stream_config.sample_rate().0;
        let symbol_zero = (0..(elapse * sample_rate as f32) as usize)
            .map(|item| {
                let t = item as f32 * 2.0 * PI / sample_rate as f32;
                (t * frequency as f32).sin()
            })
            .collect::<Symbol>();
        let symbol_one = symbol_zero.iter().map(|item| -item).collect::<Symbol>();

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
