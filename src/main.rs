//! Records a WAV file (roughly 3 seconds long) using the default input device and config.
//!
//! The input data is recorded to "$CARGO_MANIFEST_DIR/recorded.wav".

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use hound::SampleFormat;
use rathernet::raudio::{AsioDevice, AudioInputStream, AudioOutputStream};
use rathernet::raudio::{IntoSpec, Track};
use rodio::Decoder;
use tokio_stream::StreamExt;

use std::fs::File;
use std::io::BufReader;

use std::thread;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let mut stream = AudioInputStream::<f32>::try_default()?;

    let data = stream.read_timeout(Duration::from_secs(10)).await;

    let mut spec = stream.config().clone().into_spec();
    spec.sample_format = SampleFormat::Float;

    let track = Track::from_vec(spec, data);

    drop(stream);

    let stream = AudioOutputStream::try_default()?;
    stream.write(track.into_iter()).await;

    Ok(())
}
