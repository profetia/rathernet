use std::f32::consts::PI;

use anyhow::Result;
use clap::{Parser, Subcommand};
use cpal::SupportedStreamConfig;

use rathernet::raudio::{AsioDevice, AudioOutputStream, AudioSamples, AudioTrack};
use rodio::DeviceTrait;

#[derive(Debug, Parser)]
#[clap(name = "rather", version = "0.1.0", author = "Rathernet")]
#[clap(about = "A command line interface for rathernet ather.", long_about = None)]
struct RatherCli {
    #[clap(subcommand)]
    subcmd: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Calibrate the ather by writing a test signal.
    Calibrate {
        /// The elapse time in seconds to write the test signal.
        #[clap(short, long)]
        #[arg(default_value = "10")]
        elapse: u64,
        /// The device used to write the test signal.
        #[clap(short, long)]
        device: Option<String>,
    },
}

async fn calibrate(elapse: u64, device: Option<String>) -> Result<()> {
    let device = match device {
        Some(name) => AsioDevice::try_from_name(&name)?,
        None => AsioDevice::try_default()?,
    };
    let default_config = device.0.default_output_config()?;
    let config = SupportedStreamConfig::new(
        1,
        cpal::SampleRate(48000),
        default_config.buffer_size().clone(),
        default_config.sample_format(),
    );
    let stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;

    let sample_rate = config.sample_rate().0;
    let signal_len = (sample_rate as u64 * elapse) as usize;
    let signal = (0..signal_len)
        .map(|item| {
            let t = item as f32 * 2.0 * PI / sample_rate as f32;
            (t * 1000f32).sin() + (t * 10000f32).sin()
        })
        .collect::<AudioSamples<f32>>();

    let track = AudioTrack::new(config, signal);

    stream.write(track).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = RatherCli::parse();
    match cli.subcmd {
        Commands::Calibrate { elapse, device } => calibrate(elapse, device).await?,
    }
    Ok(())
}
