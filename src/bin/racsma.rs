use anyhow::Result;
use bitvec::prelude::*;
use clap::{Parser, Subcommand, ValueEnum};
use rathernet::racsma::{AcsmaIoSocket, AcsmaIoStream, AcsmaSocketConfig, AcsmaStreamConfig};
use rathernet::rather::builtin::PAYLOAD_BITS_LEN;
use rathernet::rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig};
use rathernet::raudio::{AsioDevice, AudioInputStream, AudioOutputStream};
use rodio::DeviceTrait;
use rodio::SupportedStreamConfig;
use std::fs::{self, File};
use std::io;
use std::net::Ipv4Addr;
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
        source: PathBuf,
        /// The device used to send the file.
        #[clap(short, long)]
        device: Option<String>,
        /// Interprets the file as a text file consisting of 1s and 0s.
        #[clap(short, long, default_value = "false")]
        chars: bool,
        /// The address that will be used to send the file.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The peer address that will receive the file.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        peer: usize,
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
        /// The address that will be used to recieve the bits.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The peer address that will send the bits from.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        peer: usize,
    },
    /// Write bits from a file through the acsma while reading bits from the acsma
    Duplex {
        /// The path to the file to transmit.
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
        /// The address that will be used to send the file.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The peer address that will receive the file.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        peer: usize,
    },
    /// Measure the performance of the acsma.
    Perf {
        /// The device used to send the bits.
        #[clap(short, long)]
        device: Option<String>,
        /// The address that will be used to send the bits.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The peer address that will receive the bits.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        peer: usize,
    },
    /// Keep the acsma running to serve activities from peers
    Serve {
        /// The device used to send the bits.
        #[clap(short, long)]
        device: Option<String>,
        /// The address that will be used to send the bits.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The ip address that will be used to serve the activities.
        #[clap(short, long)]
        ip: Option<Ipv4Addr>,
    },
    /// Ping a peer to check if it is alive.
    Ping {
        /// The device used to ping the peer.
        #[clap(short, long)]
        device: Option<String>,
        /// The address that will be used to ping the peer.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The peer address that will be pinged.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        peer: usize,
    },
    /// Arp a peer to check get its ip address.
    Arp {
        /// The device used to arp the peer.
        #[clap(short, long)]
        device: Option<String>,
        /// The address that will be used to arp the peer.
        #[clap(short, long, default_value = "0", value_parser = parse_address)]
        address: usize,
        /// The target ip address that will be arped.
        #[arg(required = true)]
        target: Ipv4Addr,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CalibrateType {
    Read,
    Write,
    Duplex,
}

fn parse_address(src: &str) -> Result<usize> {
    let num = src.parse::<usize>()?;
    if num < 16 {
        Ok(num)
    } else {
        Err(RacsmaError::InvalidAddress(num).into())
    }
}

#[derive(Error, Debug)]
enum RacsmaError {
    #[error("Invalid character in file (expect 0 or 1, found `{0}`)")]
    InvalidChar(char),
    #[error("Invalid address (expect 0-15, found `{0}`)")]
    InvalidAddress(usize),
}

fn create_device(device: Option<String>) -> Result<AsioDevice> {
    let device = match device {
        Some(name) => AsioDevice::try_from_name(&name)?,
        None => AsioDevice::try_default()?,
    };
    Ok(device)
}

fn create_stream_config(device: &AsioDevice) -> Result<SupportedStreamConfig> {
    let device_config = device.0.default_output_config()?;
    let stream_config = SupportedStreamConfig::new(
        1,
        cpal::SampleRate(48000),
        device_config.buffer_size().clone(),
        device_config.sample_format(),
    );

    Ok(stream_config)
}

fn load_bits(source: PathBuf, chars: bool) -> Result<BitVec> {
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

    Ok(bits)
}

fn dump_bits(mut buf: BitVec, file: Option<PathBuf>, chars: bool) -> Result<()> {
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

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = RacsmaCli::parse();
    match cli.subcmd {
        Commands::Calibrate {
            source,
            file,
            device,
            chars,
            r#type,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;

            let read_stream =
                AudioInputStream::try_from_device_config(&device, stream_config.clone())?;
            let write_stream =
                AudioOutputStream::try_from_device_config(&device, stream_config.clone())?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());
            let mut read_ather = AtherInputStream::new(ather_config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(ather_config.clone(), write_stream);

            let bits = load_bits(source, chars)?;

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
                    let buf = read_future.await;
                    dump_bits(buf, file, chars)?;
                }
                CalibrateType::Read => {
                    write_future.await;
                }
                CalibrateType::Duplex => {
                    let (_, buf) = tokio::join!(write_future, read_future);
                    dump_bits(buf, file, chars)?;
                }
            }
        }
        Commands::Write {
            source,
            device,
            chars,
            address,
            peer,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let read_stream =
                AudioInputStream::try_from_device_config(&device, stream_config.clone())?;
            let write_stream =
                AudioOutputStream::try_from_device_config(&device, stream_config.clone())?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());
            let read_ather = AtherInputStream::new(ather_config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(ather_config.clone(), write_stream);

            let stream_config = AcsmaStreamConfig::new(address);
            let mut acsma_stream = AcsmaIoStream::new(stream_config, read_ather, write_ather);

            let bits = load_bits(source, chars)?;
            acsma_stream.write(peer, &bits).await?;
        }
        Commands::Read {
            file,
            device,
            chars,
            address,
            peer,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let read_stream =
                AudioInputStream::try_from_device_config(&device, stream_config.clone())?;
            let write_stream =
                AudioOutputStream::try_from_device_config(&device, stream_config.clone())?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());
            let read_ather = AtherInputStream::new(ather_config.clone(), read_stream);
            let write_ather = AtherOutputStream::new(ather_config.clone(), write_stream);

            let stream_config = AcsmaStreamConfig::new(address);
            let mut acsma_stream = AcsmaIoStream::new(stream_config, read_ather, write_ather);

            let buf = acsma_stream.read(peer).await?;
            dump_bits(buf, file, chars)?;
        }
        Commands::Duplex {
            source,
            file,
            device,
            chars,
            address,
            peer,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let socket_config = AcsmaSocketConfig::new(address, None, ather_config);
            let (tx_socket, mut rx_socket) =
                AcsmaIoSocket::try_from_device(socket_config, &device)?;

            let bits = load_bits(source, chars)?;
            let (_, buf) = tokio::try_join!(tx_socket.write(peer, &bits), rx_socket.read(peer))?;
            dump_bits(buf, file, chars)?;
        }
        Commands::Perf {
            device,
            address,
            peer,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let socket_config = AcsmaSocketConfig::new(address, None, ather_config);
            let (tx_socket, _) = AcsmaIoSocket::try_from_device(socket_config, &device)?;

            tx_socket.perf(peer).await?;
        }
        Commands::Serve {
            device,
            address,
            ip,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let ip = ip.map(|ip| u32::from_be_bytes(ip.octets()) as usize);
            let socket_config = AcsmaSocketConfig::new(address, ip, ather_config);
            let (_, mut rx_socket) = AcsmaIoSocket::try_from_device(socket_config, &device)?;

            rx_socket.serve().await?;
        }
        Commands::Ping {
            device,
            address,
            peer,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let socket_config = AcsmaSocketConfig::new(address, None, ather_config);
            let (tx_socket, _) = AcsmaIoSocket::try_from_device(socket_config, &device)?;
            tx_socket.ping(peer).await?;
        }
        Commands::Arp {
            device,
            address,
            target: ip,
        } => {
            let device = create_device(device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let socket_config = AcsmaSocketConfig::new(address, None, ather_config);
            let (tx_socket, _) = AcsmaIoSocket::try_from_device(socket_config, &device)?;

            let target = u32::from_be_bytes(ip.octets()) as usize;
            println!("Who has {}? Tell {}", ip, address);
            let result = tx_socket.arp(target).await?;
            println!("{} is at {}", ip, result);
        }
    }
    Ok(())
}
