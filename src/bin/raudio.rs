use std::{fs::File, io::BufReader, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use cpal::{traits::HostTrait, SupportedInputConfigs, SupportedOutputConfigs};
use hound::SampleFormat;
use rathernet::raudio::{AsioDevice, AsioHost, AudioInputStream, AudioOutputStream, IntoSpec};
use rodio::{Decoder, DeviceTrait};

#[derive(Debug, Parser)]
#[clap(name = "raudio", version = "0.1.0", author = "Rathernet")]
#[clap(about = "A command line interface for rathernet audio.", long_about = None)]
struct RaudioCli {
    #[clap(subcommand)]
    subcmd: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Write audio from a file to an output device.
    #[command(arg_required_else_help = true)]
    Write {
        /// The path to the audio file to write.
        #[arg(required = true)]
        source: PathBuf,
        /// The name of the output device to write to.
        #[clap(short, long)]
        device: Option<String>,
        /// The elapsed time to write audio for.
        #[clap(short, long)]
        elapse: Option<u64>,
    },
    /// Read audio from an input device.
    #[command(arg_required_else_help = true)]
    Read {
        /// The name of the input device to read from.
        #[clap(short, long)]
        device: Option<String>,
        /// The path to the file to write the audio to.
        /// If not specified, the audio will be written to the default output device.
        #[clap(short, long)]
        file: Option<PathBuf>,
        /// The elapsed time to read audio for.
        #[arg(required = true, default_value = "10")]
        elapse: u64,
    },
    /// Write audio from a file to an output device, while reading audio from an input device.
    #[command(arg_required_else_help = true)]
    Duplex {
        /// The path to the audio file to write.
        #[arg(required = true)]
        source: PathBuf,
        /// The name of the device to read audio from and write audio to.
        #[clap(short, long)]
        device: Option<String>,
        /// The path to the file to write the audio to.
        /// If not specified, the audio will be written to the default output device.
        #[clap(short, long)]
        file: Option<PathBuf>,
        /// The elapsed time to read and write audio for.
        #[clap(short, long, default_value = "10")]
        elapse: u64,
    },
    /// List the available audio devices.
    List {
        /// The type of devices to be displayed.
        #[clap(short, long)]
        r#type: Option<DeviceType>,
        /// Display supported configurations for each device.
        #[clap(short, long, default_value = "false")]
        config: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum DeviceType {
    Input,
    Output,
    Duplex,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = RaudioCli::parse();
    match cli.subcmd {
        Commands::Write {
            source,
            device,
            elapse,
        } => {
            let stream = match device {
                Some(name) => AudioOutputStream::try_from_name(&name)?,
                None => AudioOutputStream::try_default()?,
            };
            let file = BufReader::new(File::open(source)?);
            let source = Decoder::new(file)?;
            if let Some(duration) = elapse {
                stream
                    .write_timeout(source, std::time::Duration::from_secs(duration))
                    .await;
            } else {
                stream.write(source).await;
            }
        }
        Commands::Read {
            device,
            file,
            elapse,
        } => {
            let device = match device {
                Some(name) => AsioDevice::try_from_name(&name)?,
                None => AsioDevice::try_default()?,
            };
            let mut stream = AudioInputStream::<f32>::try_from_device(&device)?;
            let data = stream
                .read_timeout(std::time::Duration::from_secs(elapse))
                .await;
            let track = rathernet::raudio::Track::from_vec(
                {
                    let mut spec = stream.config().clone().into_spec();
                    spec.sample_format = SampleFormat::Float;
                    spec
                },
                data,
            );
            drop(stream);
            if let Some(path) = file {
                track.write_to_file(path)?;
            } else {
                eprintln!("No output file specified. Playing audio to default output device.");
                let stream = AudioOutputStream::try_default()?;
                stream.write(track.into_iter()).await;
            }
        }
        Commands::Duplex {
            source,
            device,
            file,
            elapse,
        } => {
            let device = match device {
                Some(name) => AsioDevice::try_from_name(&name)?,
                None => AsioDevice::try_default()?,
            };
            let mut read_stream = AudioInputStream::<f32>::try_from_device(&device)?;
            let write_stream = AudioOutputStream::try_from_device(&device)?;

            let source = Decoder::new(BufReader::new(File::open(source)?))?;

            let (_, data) = tokio::join!(
                write_stream.write_timeout(source, std::time::Duration::from_secs(elapse)),
                read_stream.read_timeout(std::time::Duration::from_secs(elapse))
            );

            let mut spec = read_stream.config().clone().into_spec();
            spec.sample_format = SampleFormat::Float;
            let track = rathernet::raudio::Track::from_vec(spec, data);

            drop(read_stream);
            drop(write_stream);

            if let Some(path) = file {
                track.write_to_file(path)?;
            } else {
                eprintln!("No output file specified. Playing audio to default output device.");
                let stream = AudioOutputStream::try_default()?;
                stream.write(track.into_iter()).await;
            }
        }
        Commands::List { r#type, config } => {
            let host = AsioHost::try_new()?;
            for (index, device) in host.inner.devices()?.enumerate() {
                let name = device.name()?;
                let input_configs = device.supported_input_configs().ok();
                let output_configs = device.supported_output_configs().ok();

                if let Some(device_type) = r#type {
                    match device_type {
                        DeviceType::Duplex
                            if input_configs.is_none() || output_configs.is_none() =>
                        {
                            continue;
                        }
                        DeviceType::Input if input_configs.is_none() => {
                            continue;
                        }
                        DeviceType::Output if output_configs.is_none() => {
                            continue;
                        }
                        _ => {}
                    }
                    println!("{} {}", index, name);
                    if config {
                        display_device(input_configs, output_configs);
                    }
                } else {
                    println!("{} {}", index, name);
                    if config {
                        display_device(input_configs, output_configs);
                    }
                }
            }
        }
    }
    Ok(())
}

fn display_device(
    input_configs: Option<SupportedInputConfigs>,
    output_configs: Option<SupportedOutputConfigs>,
) {
    if let Some(input_configs) = input_configs {
        println!("Supported input configs:");
        for (index, config) in input_configs.enumerate() {
            println!("  {} {:?}", index, config);
        }
    }
    if let Some(output_configs) = output_configs {
        println!("Supported output configs:");
        for (index, config) in output_configs.enumerate() {
            println!("  {} {:?}", index, config);
        }
    }
}
