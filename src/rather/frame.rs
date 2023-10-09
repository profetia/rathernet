use crate::raudio::AudioSamples;
use std::f32::consts::PI;

#[derive(Debug, Clone)]
pub struct Warmup(pub AudioSamples<f32>);

impl Warmup {
    pub fn new(warmup_len: usize, sample_rate: u32, duration: f32) -> Self {
        let len = warmup_len as u32 * (duration * sample_rate as f32) as u32;
        let warmup = (0..len)
            .map(|item| (item as f32 * 2.0 * PI / sample_rate as f32).sin())
            .collect::<AudioSamples<f32>>();
        Self(warmup)
    }
}

impl From<Warmup> for AudioSamples<f32> {
    fn from(value: Warmup) -> Self {
        value.0
    }
}

#[derive(Debug, Clone)]
pub struct Preamble(pub AudioSamples<f32>);

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
            .collect::<AudioSamples<f32>>();
        Self(preamble)
    }
}

impl From<Preamble> for AudioSamples<f32> {
    fn from(value: Preamble) -> Self {
        value.0
    }
}

#[derive(Debug, Clone)]
pub struct Symbol(pub AudioSamples<f32>);

impl Symbol {
    pub fn new(sample_rate: u32, duration: f32) -> (Self, Self) {
        let zero = (0..(duration * sample_rate as f32) as usize)
            .map(|item| 1. - 2. * (item as f32) / (duration * sample_rate as f32))
            .collect::<AudioSamples<f32>>();
        let one = zero.iter().map(|item| -item).collect::<AudioSamples<f32>>();

        (Self(zero), Self(one))
    }
}

impl From<Symbol> for AudioSamples<f32> {
    fn from(value: Symbol) -> Self {
        value.0
    }
}

impl FromIterator<Symbol> for AudioSamples<f32> {
    fn from_iter<T: IntoIterator<Item = Symbol>>(iter: T) -> Self {
        let mut samples = vec![];
        for item in iter {
            samples.extend(item.0.iter());
        }
        samples.into()
    }
}
