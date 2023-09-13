//! Records a WAV file (roughly 3 seconds long) using the default input device and config.
//!
//! The input data is recorded to "$CARGO_MANIFEST_DIR/recorded.wav".

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use hound::SampleFormat;
use rathernet::raudio::track::{IntoSpec, Track};
use rathernet::raudio::AudioOutputStream;

use std::sync::mpsc;
use std::sync::mpsc::Sender;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let host = cpal::default_host();

    // Set up the input device and stream with the default input config.
    let device = host
        .default_input_device()
        .expect("failed to find input device");

    println!("Input device: {}", device.name()?);

    let config = device
        .default_input_config()
        .expect("Failed to get default input config");
    println!("Default input config: {:?}", config);

    // The WAV file we're recording to.
    const PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/recorded.wav");

    // A flag to indicate that recording is in progress.
    println!("Begin recording...");

    let err_fn = move |err| {
        eprintln!("an error occurred on stream: {}", err);
    };

    let mut spec = config.clone().into_spec();
    spec.sample_format = SampleFormat::Float;

    let (sender, reciever) = mpsc::channel::<Vec<f32>>();

    let stream = match config.sample_format() {
        cpal::SampleFormat::I8 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i8, f32>(data, sender.clone()),
            err_fn,
            None,
        )?,

        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i16, f32>(data, sender.clone()),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I32 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<i32, f32>(data, sender.clone()),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data, _: &_| write_input_data::<f32, f32>(data, sender.clone()),
            err_fn,
            None,
        )?,
        sample_format => {
            return Err(anyhow::Error::msg(format!(
                "Unsupported sample format '{sample_format}'"
            )))
        }
    };

    stream.play()?;

    // Let recording go for roughly three seconds.
    std::thread::sleep(std::time::Duration::from_secs(10));
    drop(stream);

    let mut track = Track::new(spec);
    while let Ok(data) = reciever.recv() {
        track.extend(data.into_iter())
    }

    println!("Recording {} complete!", PATH);

    println!("Begin playback...");
    let stream = AudioOutputStream::try_default()?;

    stream.write(track.clone().into_iter()).await;

    println!("Saving {}...", PATH);

    track.save_as(PATH)?;

    Ok(())
}

fn write_input_data<T, U>(input: &[T], sender: Sender<Vec<U>>)
where
    T: Sample,
    U: Sample + rodio::Sample + hound::Sample + FromSample<T>,
{
    sender
        .send(
            input
                .iter()
                .map(|sample| U::from_sample(*sample))
                .collect::<Vec<U>>(),
        )
        .unwrap();
}
