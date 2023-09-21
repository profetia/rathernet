use crate::raudio::{AudioSamples, AudioTrack, SharedSamples};
use cpal::SupportedStreamConfig;
use rodio::Source;
use std::{f32::consts::PI, time::Duration};

#[derive(Debug, Clone)]
pub struct Warmup(pub SharedSamples<f32>);

impl Warmup {
    pub fn new(warmup_len: usize, sample_rate: u32, duration: f32) -> Self {
        let len = warmup_len as u32 * (duration * sample_rate as f32) as u32;
        let warmup = (0..len)
            .map(|item| {
                if item < len / 2 {
                    2000.0 + item as f32 * 6000.0 / (len / 2) as f32
                } else {
                    8000.0 - (item - len / 2) as f32 * 6000.0 / (len / 2) as f32
                }
            })
            .map(|item| (item * 2.0 * PI / sample_rate as f32).sin())
            .collect::<SharedSamples<f32>>();
        Self(warmup)
    }
}

impl From<Warmup> for Preamble {
    fn from(value: Warmup) -> Self {
        Self(value.0)
    }
}

#[derive(Debug, Clone)]
pub struct Preamble(pub SharedSamples<f32>);

impl Preamble {
    pub fn new(preamble_len: usize, sample_rate: u32, duration: f32) -> Self {
        let len = preamble_len as u32 * (duration * sample_rate as f32) as u32;
        let preamble = (0..len)
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
        Self(preamble)
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    preamble: Preamble,
    length: Vec<Symbol>,
    state: HeaderState,
}

#[derive(Debug, Clone)]
enum HeaderState {
    Preamble(usize),
    Length(usize, usize),
    Completed,
}

impl Header {
    pub fn new(preamble: Preamble, length: Vec<Symbol>) -> Self {
        Self {
            preamble,
            length,
            state: HeaderState::Preamble(0),
        }
    }
}

impl Iterator for Header {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            HeaderState::Preamble(offset) => {
                if offset < self.preamble.0.len() {
                    self.state = HeaderState::Preamble(offset + 1);
                    Some(self.preamble.0[offset])
                } else {
                    self.state = HeaderState::Length(0, 0);
                    self.next()
                }
            }
            HeaderState::Length(index, offset) => {
                if index < self.length.len() {
                    if offset < self.length[index].0.len() {
                        self.state = HeaderState::Length(index, offset + 1);
                        Some(self.length[index].0[offset])
                    } else {
                        self.state = HeaderState::Length(index + 1, 0);
                        self.next()
                    }
                } else {
                    self.state = HeaderState::Completed;
                    None
                }
            }
            HeaderState::Completed => None,
        }
    }
}

impl ExactSizeIterator for Header {
    fn len(&self) -> usize {
        match self.state {
            HeaderState::Preamble(offset) => {
                self.length
                    .iter()
                    .map(|symbol| symbol.0.len())
                    .sum::<usize>()
                    + (self.preamble.0.len() - offset)
            }
            HeaderState::Length(index, offset) => {
                self.length
                    .iter()
                    .skip(index)
                    .map(|symbol| symbol.0.len())
                    .sum::<usize>()
                    + (self.length[index].0.len() - offset)
            }
            HeaderState::Completed => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Symbol(pub SharedSamples<f32>);

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

#[derive(Debug, Clone)]
pub struct Body {
    payload: Vec<Symbol>,
    state: BodyState,
}

#[derive(Debug, Clone)]
enum BodyState {
    Payload(Option<(usize, usize)>),
    Completed,
}

impl Body {
    pub fn new(payload: Vec<Symbol>) -> Self {
        Self {
            state: BodyState::Payload(if !payload.is_empty() {
                Some((0, 0))
            } else {
                None
            }),
            payload,
        }
    }
}

impl Iterator for Body {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            BodyState::Payload(payload) => {
                if let Some((index, offset)) = payload {
                    if index < self.payload.len() {
                        if offset < self.payload[index].0.len() {
                            self.state = BodyState::Payload(Some((index, offset + 1)));
                            Some(self.payload[index].0[offset])
                        } else {
                            self.state = BodyState::Payload(Some((index + 1, 0)));
                            self.next()
                        }
                    } else {
                        self.state = BodyState::Completed;
                        self.next()
                    }
                } else {
                    self.state = BodyState::Completed;
                    self.next()
                }
            }
            BodyState::Completed => None,
        }
    }
}

impl ExactSizeIterator for Body {
    fn len(&self) -> usize {
        match self.state {
            BodyState::Payload(payload) => {
                if let Some((index, offset)) = payload {
                    self.payload
                        .iter()
                        .skip(index)
                        .map(|symbol| symbol.0.len())
                        .sum::<usize>()
                        + (self.payload[index].0.len() - offset)
                } else {
                    0
                }
            }
            BodyState::Completed => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frame {
    config: SupportedStreamConfig,
    header: Header,
    body: Body,
    state: FrameState,
}

#[derive(Debug, Clone)]
enum FrameState {
    Header,
    Body,
    Completed,
}

impl Frame {
    pub fn new(config: SupportedStreamConfig, header: Header, body: Body) -> Self {
        Self {
            config,
            header,
            body,
            state: FrameState::Header,
        }
    }
}

impl Iterator for Frame {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.state {
            FrameState::Header => {
                if let Some(item) = self.header.next() {
                    Some(item)
                } else {
                    self.state = FrameState::Body;
                    self.next()
                }
            }
            FrameState::Body => {
                if let Some(item) = self.body.next() {
                    Some(item)
                } else {
                    self.state = FrameState::Completed;
                    None
                }
            }
            FrameState::Completed => None,
        }
    }
}

impl ExactSizeIterator for Frame {
    fn len(&self) -> usize {
        self.header.len() + self.body.len()
    }
}

impl Source for Frame {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.len())
    }

    fn channels(&self) -> u16 {
        self.config.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate().0
    }

    fn total_duration(&self) -> Option<Duration> {
        let header_len = self.header.preamble.0.len()
            + self
                .header
                .length
                .iter()
                .map(|symbol| symbol.0.len())
                .sum::<usize>();
        let body_len = self
            .body
            .payload
            .iter()
            .map(|symbol| symbol.0.len())
            .sum::<usize>();
        Some(Duration::from_secs_f32(
            (header_len + body_len) as f32
                / (self.config.sample_rate().0 as f32 * self.config.channels() as f32),
        ))
    }
}

impl From<Frame> for AudioTrack<f32> {
    fn from(value: Frame) -> Self {
        let config = value.config.clone();
        AudioTrack::new(config, value.into())
    }
}

impl From<Frame> for AudioSamples<f32> {
    fn from(value: Frame) -> Self {
        let mut source = vec![];
        source.extend(Into::<AudioSamples<f32>>::into(value.header).iter());
        source.extend(Into::<AudioSamples<f32>>::into(value.body).iter());
        source.into()
    }
}

impl From<Header> for AudioSamples<f32> {
    fn from(value: Header) -> Self {
        let mut source = value.preamble.0.to_vec();
        for symbol in value.length.iter() {
            source.extend(symbol.0.to_vec());
        }
        source.into()
    }
}

impl From<Body> for AudioSamples<f32> {
    fn from(value: Body) -> Self {
        let mut source = vec![];
        for symbol in value.payload.iter() {
            source.extend(symbol.0.to_vec());
        }
        source.into()
    }
}
