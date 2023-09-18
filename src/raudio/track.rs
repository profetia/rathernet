use anyhow::Result;
use cpal::SupportedStreamConfig;
use hound::{SampleFormat, WavSpec, WavWriter};
use rodio::{Sample, Source};
use std::{path::Path, sync::Arc, time::Duration};

pub type AudioSamples<S> = Box<[S]>;
pub type SharedSamples<S> = Arc<[S]>;

#[derive(Debug, Clone)]
pub struct AudioTrack<T: Sample> {
    config: SupportedStreamConfig,
    offset: usize,
    buf: AudioSamples<T>,
}

impl<T: Sample> AudioTrack<T> {
    pub fn new(config: SupportedStreamConfig, buf: AudioSamples<T>) -> Self {
        Self {
            config,
            buf,
            offset: 0,
        }
    }
}

impl<T: Sample + hound::Sample> AudioTrack<T> {
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let spec = WavSpec {
            channels: self.config.channels(),
            sample_rate: self.config.sample_rate().0,
            bits_per_sample: (self.config.sample_format().sample_size() * 8) as _,
            sample_format: if self.config.sample_format().is_float() {
                SampleFormat::Float
            } else {
                SampleFormat::Int
            },
        };

        let mut writer = WavWriter::create(path, spec)?;
        for sample in self.buf.iter() {
            writer.write_sample(*sample)?;
        }
        writer.finalize()?;
        Ok(())
    }
}

impl<T: Sample> Iterator for AudioTrack<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }

        let sample = self.buf[self.offset];
        self.offset += 1;
        Some(sample)
    }
}

impl<T: Sample> Source for AudioTrack<T> {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buf.len() - self.offset)
    }

    fn channels(&self) -> u16 {
        self.config.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate().0
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(
            self.buf.len() as f32
                / (self.config.sample_rate().0 as f32 * self.config.channels() as f32),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct SharedTrack<T: Sample> {
    config: SupportedStreamConfig,
    buf: SharedSamples<T>,
    offset: usize,
}

impl<T: Sample> SharedTrack<T> {
    pub fn new(config: SupportedStreamConfig, buf: SharedSamples<T>) -> Self {
        Self {
            config,
            buf,
            offset: 0,
        }
    }
}

impl<T: Sample + hound::Sample> SharedTrack<T> {
    pub fn write_to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let spec = WavSpec {
            channels: self.config.channels(),
            sample_rate: self.config.sample_rate().0,
            bits_per_sample: (self.config.sample_format().sample_size() * 8) as _,
            sample_format: if self.config.sample_format().is_float() {
                SampleFormat::Float
            } else {
                SampleFormat::Int
            },
        };

        let mut writer = WavWriter::create(path, spec)?;
        for sample in self.buf.iter() {
            writer.write_sample(*sample)?;
        }
        writer.finalize()?;
        Ok(())
    }
}

impl<T: Sample> Iterator for SharedTrack<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.buf.len() {
            return None;
        }

        let sample = self.buf[self.offset];
        self.offset += 1;
        Some(sample)
    }
}

impl<T: Sample> Source for SharedTrack<T> {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.buf.len() - self.offset)
    }

    fn channels(&self) -> u16 {
        self.config.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.config.sample_rate().0
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f32(
            self.buf.len() as f32
                / (self.config.sample_rate().0 as f32 * self.config.channels() as f32),
        ))
    }
}

impl<T> From<AudioTrack<T>> for SharedTrack<T>
where
    T: Sample,
{
    fn from(track: AudioTrack<T>) -> Self {
        Self {
            config: track.config,
            buf: track.buf.into(),
            offset: track.offset,
        }
    }
}
