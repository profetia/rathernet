use anyhow::Result;
use bitvec::prelude::*;
use clap::{Parser, Subcommand};
use cpal::SupportedStreamConfig;
use rathernet::{
    rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream, AudioSamples, AudioTrack},
};
use rodio::DeviceTrait;
use std::{
    f32::consts::PI,
    fs::{self, File},
    io,
    path::PathBuf,
};
use thiserror::Error;
use tokio_stream::StreamExt;

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
    /// Write bits from a file through the ather.
    #[command(arg_required_else_help = true)]
    Write {
        /// The file to send.
        #[arg(required = true)]
        file: PathBuf,
        /// The device used to send the file.
        #[clap(short, long)]
        device: Option<String>,
        /// Interprets the file as a text file consisting of 1s and 0s.
        #[clap(short, long, default_value = "false")]
        chars: bool,
    },
    /// Read bits from the ather and write them to a file.
    Read {
        /// The file to write the received bits to.
        #[clap(short, long)]
        file: Option<PathBuf>,
        /// The device used to receive the bits.
        #[clap(short, long)]
        device: Option<String>,
        /// Writes the received bits as a text file consisting of 1s and 0s.
        #[clap(short, long, default_value = "false")]
        chars: bool,
    },
    /// Write bits from a file to an output device, while reading bits from an input device.
    #[command(arg_required_else_help = true)]
    Duplex {
        /// The path to the file to write.
        #[arg(required = true)]
        source: PathBuf,
        /// The name of the device to read bits from and write bits to.
        #[clap(short, long)]
        device: Option<String>,
        /// The path to the file to store the received bits in.
        /// If not specified, the bits will be written to the default output device.
        #[clap(short, long)]
        file: Option<PathBuf>,
        /// Interprets bits as chars of 1s and 0s.
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
        Commands::Write {
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
            let ather = AtherOutputStream::new(
                AtherStreamConfig::new((10000, 10001), 1000, config),
                stream,
            );

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
        Commands::Read {
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
            let stream = AudioInputStream::try_from_device_config(&device, config.clone())?;
            let mut ather =
                AtherInputStream::new(AtherStreamConfig::new((10000, 10001), 1000, config), stream);
            let buf = ather.next().await.unwrap();

            if let Some(file) = file {
                if chars {
                    fs::write(
                        file,
                        buf.into_iter()
                            .map(|bit| if bit { '1' } else { '0' })
                            .collect::<String>(),
                    )
                    .unwrap();
                } else {
                    fs::write(
                        file,
                        buf.chunks(8)
                            .map(|chunk| {
                                let mut byte = 0u8;
                                for (index, bit) in chunk.iter().enumerate() {
                                    byte |= (*bit as u8) << index;
                                }
                                byte
                            })
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
                }
            } else {
                eprintln!("No output file specified. Write the bits to stdout.");
                println!(
                    "{}",
                    buf.into_iter()
                        .map(|bit| if bit { '1' } else { '0' })
                        .collect::<String>()
                );
            }
        }
        Commands::Duplex {
            source,
            device,
            file,
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

            let read_stream = AudioInputStream::try_from_device_config(&device, config.clone())?;
            let mut read_ather = AtherInputStream::new(
                AtherStreamConfig::new((10000, 10001), 1000, config.clone()),
                read_stream,
            );
            let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
            let write_ather = AtherOutputStream::new(
                AtherStreamConfig::new((10000, 10001), 1000, config),
                write_stream,
            );

            let mut bits = bitvec![];
            if chars {
                let source = fs::read_to_string(source)?;
                for ch in source.chars() {
                    match ch {
                        '0' => bits.push(false),
                        '1' => bits.push(true),
                        _ => return Err(RatherError::InvalidChar(ch).into()),
                    }
                }
            } else {
                let mut file = File::open(source)?;
                io::copy(&mut file, &mut bits)?;
            }

            let (_, bits) = tokio::join!(write_ather.write(&bits), read_ather.next());
            let mut buf = bits.unwrap();

            if let Some(file) = file {
                if chars {
                    fs::write(
                        file,
                        buf.into_iter()
                            .map(|bit| if bit { '1' } else { '0' })
                            .collect::<String>(),
                    )
                    .unwrap();
                } else {
                    let file = File::create(file)?;
                    io::copy(&mut buf, &mut &file)?;
                }
            } else {
                eprintln!("No output file specified. Write the bits to stdout.");
                println!(
                    "{}",
                    buf.into_iter()
                        .map(|bit| if bit { '1' } else { '0' })
                        .collect::<String>()
                );
            }
        }
    }
    Ok(())
}
