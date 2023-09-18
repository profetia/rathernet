use std::time::Duration;

use cpal::SupportedStreamConfig;
use rodio::Source;

use crate::raudio::SharedSamples;

pub type Header = SharedSamples<f32>;
pub type Symbol = SharedSamples<f32>;
pub type Body = Vec<Symbol>;

#[derive(Debug, Clone)]
pub struct Frame {
    config: SupportedStreamConfig,
    header: Header,
    body: Body,
    state: FrameState,
}

#[derive(Debug, Clone, Copy)]
enum FrameState {
    Header(usize),
    Samples(usize, usize),
    Complete,
}

impl Frame {
    pub fn new(config: SupportedStreamConfig, header: Header, body: Body) -> Self {
        Self {
            config,
            header,
            body,
            state: FrameState::Header(0),
        }
    }
}

impl Iterator for Frame {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            FrameState::Header(i) => {
                if i < self.header.len() {
                    let sample = self.header[i];
                    self.state = FrameState::Header(i + 1);
                    Some(sample)
                } else {
                    self.state = FrameState::Samples(0, 0);
                    self.next()
                }
            }
            FrameState::Samples(i, j) => {
                if i < self.body.len() {
                    let symbol = &self.body[i];
                    if j < symbol.len() {
                        let sample = symbol[j];
                        self.state = FrameState::Samples(i, j + 1);
                        Some(sample)
                    } else {
                        self.state = FrameState::Samples(i + 1, 0);
                        self.next()
                    }
                } else {
                    self.state = FrameState::Complete;
                    None
                }
            }
            FrameState::Complete => None,
        }
    }
}

impl Source for Frame {
    fn current_frame_len(&self) -> Option<usize> {
        match self.state {
            FrameState::Header(i) => self
                .body
                .iter()
                .map(|symbol| symbol.len())
                .sum::<usize>()
                .checked_add(self.header.len() - i),
            FrameState::Samples(i, j) => self
                .body
                .iter()
                .skip(i)
                .map(|symbol| symbol.len())
                .sum::<usize>()
                .checked_add(self.body[i].len() - j),
            FrameState::Complete => Some(0),
        }
    }

    fn channels(&self) -> u16 {
        self.config.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate().0
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(
            self.header.len() as f32
                + self
                    .body
                    .iter()
                    .map(|symbol| symbol.len() as f32)
                    .sum::<f32>()
                    / (self.config.sample_rate().0 as f32 * self.config.channels() as f32),
        ))
    }
}
