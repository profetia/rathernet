use crate::raudio::{AudioTrack, SharedSamples};
use cpal::SupportedStreamConfig;
use rodio::Source;
use std::{f32::consts::PI, time::Duration};

#[derive(Debug, Clone)]
pub struct Header(SharedSamples<f32>);

impl Header {
    pub fn new(sample_rate: u32, duration: f32) -> Self {
        let len = 8 * (duration * sample_rate as f32) as u32;
        let header = (0..len)
            .map(|item| {
                if item < len / 2 {
                    2000.0 + item as f32 * 6000.0 / (len / 2) as f32
                } else {
                    8000.0 - (item - len / 2) as f32 * 6000.0 / (len / 2) as f32
                }
            })
            .scan(0.0f32, |acc, item| {
                *acc += item / sample_rate as f32;
                Some(*acc)
            })
            .map(|item| (item * 2.0 * PI).sin())
            .collect::<SharedSamples<f32>>();
        Self(header)
    }
}

#[derive(Debug, Clone)]
pub struct Symbol(SharedSamples<f32>);

impl Symbol {
    pub fn new(frequency: u32, sample_rate: u32, duration: f32) -> (Self, Self) {
        let zero = (0..(duration * sample_rate as f32) as usize)
            .map(|item| {
                let t = item as f32 * 2.0 * PI / sample_rate as f32;
                (t * frequency as f32).sin()
            })
            .collect::<SharedSamples<f32>>();
        let one = zero
            .iter()
            .map(|item| -item)
            .collect::<SharedSamples<f32>>();

        (Self(zero), Self(one))
    }
}

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
                if i < self.header.0.len() {
                    let sample = self.header.0[i];
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
                    if j < symbol.0.len() {
                        let sample = symbol.0[j];
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
                .map(|symbol| symbol.0.len())
                .sum::<usize>()
                .checked_add(self.header.0.len() - i),
            FrameState::Samples(i, j) => self
                .body
                .iter()
                .skip(i)
                .map(|symbol| symbol.0.len())
                .sum::<usize>()
                .checked_add(self.body[i].0.len() - j),
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
            self.header.0.len() as f32
                + self
                    .body
                    .iter()
                    .map(|symbol| symbol.0.len() as f32)
                    .sum::<f32>()
                    / (self.config.sample_rate().0 as f32 * self.config.channels() as f32),
        ))
    }
}

impl From<Frame> for AudioTrack<f32> {
    fn from(value: Frame) -> Self {
        let mut source = value.header.0.to_vec();
        for symbol in value.body.into_iter() {
            source.extend(symbol.0.iter());
        }
        AudioTrack::new(value.config, source.into())
    }
}
