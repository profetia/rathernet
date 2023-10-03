use anyhow::Result;
use bitvec::prelude::*;
use clap::{Parser, Subcommand, ValueEnum};
use rathernet::racsma::{AcsmaIoConfig, AcsmaIoStream};
use rathernet::rather::builtin::PAYLOAD_BITS_LEN;
use rathernet::rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig};
use rathernet::raudio::{AsioDevice, AudioInputStream, AudioOutputStream};
use rodio::DeviceTrait;
use rodio::SupportedStreamConfig;
use std::fs::{self, File};
use std::io;
use std::path::PathBuf;
use thiserror::Error;
use tokio_stream::StreamExt;

#[derive(Parser, Debug)]
#[clap(name = "racsma", version = "0.1.0", author = "Rathernet")]
#[clap(about = "A command line interface for rathernet acsma", long_about = None)]
struct RacsmaCli {
    #[clap(subcommand)]
    subcmd: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Calibrate the acsma by transmitting a file.
    #[command(arg_required_else_help = true)]
    Calibrate {
        /// The path to the file to transmit.
        #[arg(required = true)]
        source: PathBuf,
        /// The path to the file to write the received bits to. If not specified, the bits will be
        /// written to stdout.
        #[clap(short, long)]
        file: Option<PathBuf>,
        /// The device used to transmit.
        #[clap(short, long)]
        device: Option<String>,
        /// Interprets the file as a text file consisting of 1s and 0s.
        #[clap(short, long, default_value = "false")]
        chars: bool,
        /// The type of calibration to perform.
        #[clap(short, long, default_value = "duplex")]
        r#type: CalibrateType,
    },
    /// Write bits from a file through the acsma.
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
    /// Read bits from the acsma and write them to a file.
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
        /// Number of bits to read.
        #[clap(short, long, default_value = "0")]
        num_bits: usize,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CalibrateType {
    Read,
    Write,
    Duplex,
}

#[derive(Error, Debug)]
enum RacsmaError {
    #[error("Invalid character in file (expected 0 or 1, found `{0}`)")]
    InvalidChar(char),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = RacsmaCli::parse();
    match cli.subcmd {
        Commands::Calibrate {
            source,
            file,
            device,
            chars,
            r#type,
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
            let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
            let config = AtherStreamConfig::new(10000, 15000, config.clone());
            let mut read_ather = AtherInputStream::new(config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(config.clone(), write_stream);

            let mut bits = bitvec![];
            if chars {
                let source = fs::read_to_string(source)?;
                for ch in source.chars() {
                    match ch {
                        '0' => bits.push(false),
                        '1' => bits.push(true),
                        _ => return Err(RacsmaError::InvalidChar(ch).into()),
                    }
                }
            } else {
                let mut file = File::open(source)?;
                io::copy(&mut file, &mut bits)?;
            }

            let write_future = async {
                for chunk in bits.chunks(PAYLOAD_BITS_LEN) {
                    write_ather.write(chunk).await.unwrap();
                }
            };
            let read_future = async {
                let mut buf = bitvec![];
                while let Some(frame) = read_ather.next().await {
                    let len = frame.len();
                    buf.extend(frame);
                    eprintln!("Received: {}, new: {}", buf.len(), len);
                    if buf.len() >= bits.len() {
                        break;
                    }
                }
                buf
            };

            match r#type {
                CalibrateType::Write => {
                    let mut buf = read_future.await;
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
                CalibrateType::Read => {
                    write_future.await;
                }
                CalibrateType::Duplex => {
                    let (_, mut buf) = tokio::join!(write_future, read_future);
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
        }
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

            let read_stream = AudioInputStream::try_from_device_config(&device, config.clone())?;
            let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
            let config = AtherStreamConfig::new(10000, 15000, config.clone());
            let read_ather = AtherInputStream::new(config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(config.clone(), write_stream);

            let config = AcsmaIoConfig::new(0);
            let mut acsma = AcsmaIoStream::new(config, read_ather, write_ather);

            let mut bits = bitvec![];
            if chars {
                let source = fs::read_to_string(file)?;
                for ch in source.chars() {
                    match ch {
                        '0' => bits.push(false),
                        '1' => bits.push(true),
                        _ => return Err(RacsmaError::InvalidChar(ch).into()),
                    }
                }
            } else {
                let mut file = File::open(file)?;
                io::copy(&mut file, &mut bits)?;
            }

            acsma.write(0usize, &bits).await?;
        }
        Commands::Read {
            file,
            device,
            chars,
            num_bits,
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
            let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
            let config = AtherStreamConfig::new(10000, 15000, config.clone());
            let read_ather = AtherInputStream::new(config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(config.clone(), write_stream);

            let config = AcsmaIoConfig::new(0);
            let mut acsma = AcsmaIoStream::new(config, read_ather, write_ather);

            let mut buf = bitvec![0; num_bits];
            acsma.read(0usize, &mut buf).await?;

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
                    let mut file = File::create(file)?;
                    io::copy(&mut buf, &mut file)?;
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
