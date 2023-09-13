use std::{
    slice::{Iter, IterMut},
    time::Duration,
    vec::IntoIter,
};

use anyhow::Result;
use cpal::SupportedStreamConfig;
use hound::{Sample, WavSpec};
use rodio::Source;

pub trait IntoSpec {
    fn into_spec(self) -> WavSpec;
}

impl IntoSpec for SupportedStreamConfig {
    fn into_spec(self) -> WavSpec {
        WavSpec {
            channels: self.channels(),
            sample_rate: self.sample_rate().0,
            bits_per_sample: (self.sample_format().sample_size() * 8) as _,
            sample_format: if self.sample_format().is_float() {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        }
    }
}

#[derive(Clone)]
pub struct Track<T: Sample + rodio::Sample> {
    spec: WavSpec,
    buf: Vec<T>,
}

impl<T: Sample + rodio::Sample> Track<T> {
    pub fn new(spec: WavSpec) -> Self {
        Self {
            spec,
            buf: Vec::new(),
        }
    }

    pub fn from_source(source: impl Source<Item = T>) -> Self {
        let spec = WavSpec {
            channels: source.channels(),
            sample_rate: source.sample_rate(),
            bits_per_sample: (std::mem::size_of::<T>() * 8) as _,
            sample_format: if std::mem::size_of::<T>() == 4 {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        };

        let buf = source.collect::<Vec<_>>();

        Self { spec, buf }
    }
}

impl<T: Sample + rodio::Sample> Track<T> {
    pub fn push(&mut self, data: T) {
        self.buf.push(data);
    }

    pub fn extend(&mut self, data: impl IntoIterator<Item = T>) {
        self.buf.extend(data);
    }

    pub fn iter(&self) -> TrackIter<'_, T> {
        TrackIter {
            inner: self.buf.iter(),
        }
    }

    pub fn iter_mut(&mut self) -> TrackIterMut<'_, T> {
        TrackIterMut {
            inner: self.buf.iter_mut(),
        }
    }

    pub fn save_as(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let mut writer = hound::WavWriter::create(path, self.spec)?;

        for sample in &self.buf {
            writer.write_sample(*sample)?;
        }

        writer.finalize()?;

        Ok(())
    }
}

pub struct TrackIter<'a, T: Sample + rodio::Sample> {
    inner: Iter<'a, T>,
}

impl<'a, T: Sample + rodio::Sample> Iterator for TrackIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a, T: Sample + rodio::Sample> ExactSizeIterator for TrackIter<'a, T> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> IntoIterator for &'a Track<f32> {
    type Item = &'a f32;
    type IntoIter = TrackIter<'a, f32>;

    fn into_iter(self) -> Self::IntoIter {
        TrackIter {
            inner: self.buf.iter(),
        }
    }
}

pub struct TrackIterMut<'a, T: Sample + rodio::Sample> {
    inner: IterMut<'a, T>,
}

impl<'a, T: Sample + rodio::Sample> Iterator for TrackIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a, T: Sample + rodio::Sample> ExactSizeIterator for TrackIterMut<'a, T> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> IntoIterator for &'a mut Track<f32> {
    type Item = &'a mut f32;
    type IntoIter = TrackIterMut<'a, f32>;

    fn into_iter(self) -> Self::IntoIter {
        TrackIterMut {
            inner: self.buf.iter_mut(),
        }
    }
}

pub struct TrackIntoIter<T: Sample + rodio::Sample> {
    spec: WavSpec,
    inner: IntoIter<T>,
}

impl<T: Sample + rodio::Sample> Iterator for TrackIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<T: Sample + rodio::Sample> ExactSizeIterator for TrackIntoIter<T> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<T: Sample + rodio::Sample> IntoIterator for Track<T> {
    type Item = T;
    type IntoIter = TrackIntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        TrackIntoIter {
            spec: self.spec,
            inner: self.buf.into_iter(),
        }
    }
}

impl<T: Sample + rodio::Sample> Source for TrackIntoIter<T> {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.inner.len())
    }

    fn channels(&self) -> u16 {
        self.spec.channels
    }

    fn sample_rate(&self) -> u32 {
        self.spec.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
