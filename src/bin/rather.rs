use anyhow::Result;
use bitvec::prelude::*;
use clap::{Parser, Subcommand};
use cpal::SupportedStreamConfig;
use rathernet::{
    rather::{AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioOutputStream, AudioSamples, AudioTrack},
};
use rodio::DeviceTrait;
use std::{f32::consts::PI, fs};
use thiserror::Error;

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
        #[clap(short, long, default_value = "10")]
        elapse: u64,
        /// The device used to write the test signal.
        #[clap(short, long)]
        device: Option<String>,
    },
    /// Send bits from a file through the ather.
    #[command(arg_required_else_help = true)]
    Send {
        /// The file to send.
        #[arg(required = true)]
        file: String,
        /// The device used to send the file.
        #[clap(short, long)]
        device: Option<String>,
        /// Interprets the file as a text file consisting of 1s and 0s.
        #[clap(short, long, default_value = "false")]
        chars: bool,
    },
}

#[derive(Error, Debug)]
enum RatherError {
    #[error("Invalid character in file (expected 0 or 1, found `{0}`)")]
    InvalidChar(char),
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
        Commands::Send {
            file,
            device,
            chars,
        } => {
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
            let ather = AtherOutputStream::new(AtherStreamConfig::new(10000, 1000, config), stream);

            let file = fs::read_to_string(file)?;
            let mut bits = bitvec![];
            if chars {
                for ch in file.chars() {
                    match ch {
                        '0' => bits.push(false),
                        '1' => bits.push(true),
                        _ => return Err(RatherError::InvalidChar(ch).into()),
                    }
                }
            } else {
                for byte in file.bytes() {
                    bits.extend(byte.view_bits::<Lsb0>());
                }
            }

            ather.write(&bits).await;
        }
    }
    Ok(())
}
