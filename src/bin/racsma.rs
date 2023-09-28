use anyhow::Result;
use bitvec::prelude::*;
use clap::{Parser, Subcommand};
use rathernet::rather::ather::PAYLOAD_BITS_LEN;
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
    },
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
            let config = AtherStreamConfig::new(10000, 10000, config.clone());
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

            let (_, mut buf) = tokio::join!(
                async {
                    for chunk in bits.chunks(PAYLOAD_BITS_LEN) {
                        write_ather.write(chunk).await;
                    }
                },
                async {
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
                }
            );

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
