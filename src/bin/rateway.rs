use anyhow::Result;
use clap::{Parser, Subcommand};
use cpal::SupportedStreamConfig;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use rathernet::{
    racsma::AcsmaSocketConfig,
    rateway::{tools::ping, AtewayAdapterConfig, AtewayIoAdaper, AtewayIoNat, AtewayNatConfig},
    rather::AtherStreamConfig,
    raudio::AsioDevice,
};
use rodio::DeviceTrait;
use serde::{de::Error, Deserialize};
use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Write},
    net::{Ipv4Addr, SocketAddrV4},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpSocket, UdpSocket},
    time,
};

#[derive(Parser, Debug)]
#[clap(name = "rateway", version = "0.1.0", author = "Rathernet")]
#[clap(about = "A command line interface for rathernet rateway", long_about = None)]
struct RatewayCli {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    /// Transmit a file line by line in UDP.
    Udp {
        #[clap(subcommand)]
        cmd: UdpSubCommand,
    },
    /// Install rathernet rateway as a network adapter to the athernet.
    Install {
        /// The path to the configuration file.
        #[clap(short, long, default_value = "rateway.toml")]
        config: String,
    },
    /// Start an NAT server on the athernet.
    Nat {
        /// The path to the configuration file.
        #[clap(short, long, default_value = "nat.toml")]
        config: String,
    },
    /// Ping a device on the athernet.
    Ping {
        /// The peer address that will be pinged.
        #[arg(required = true)]
        peer: Ipv4Addr,
        /// The address that will be used to ping the peer.
        #[clap(short, long, default_value = "127.0.0.1")]
        address: Ipv4Addr,
        /// The elapsed time between each ping in seconds.
        #[clap(short, long, default_value = "1")]
        elapsed: u64,
        /// The port on athernet nat that will be pinged.
        #[clap(short, long, default_value = "14999")]
        port: Option<u16>,
        /// The length of the ping payload.
        #[clap(short, long, default_value = "32")]
        length: usize,
    },
    /// Transmit a file line by line in TCP.
    Tcp {
        #[clap(subcommand)]
        cmd: TcpSubCommand,
    },
}

#[derive(Subcommand, Debug)]
enum UdpSubCommand {
    /// Transmit a file line by line in UDP.
    Send {
        /// The address that will be used to send the file.
        #[clap(short, long, default_value = "127.0.0.1:80")]
        address: SocketAddrV4,
        /// The peer address that will receive the file.
        #[arg(required = true)]
        peer: SocketAddrV4,
        /// The path to the file to send.
        /// If no file is specified, random data will be sent.
        #[clap(short, long)]
        source: Option<PathBuf>,
    },
    /// Receive a file line by line in UDP.
    Receive {
        /// The address that will be used to receive the file.
        #[clap(short, long, default_value = "127.0.0.1:80")]
        address: SocketAddrV4,
        /// The path to the file to store the received data.
        #[clap(short, long)]
        file: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
enum TcpSubCommand {
    /// Transmit a file line by line in TCP.
    Send {
        /// The address that will be used to send the file.
        #[clap(short, long)]
        address: Option<SocketAddrV4>,
        /// The peer address that will receive the file.
        #[arg(required = true)]
        peer: SocketAddrV4,
        /// The path to the file to send.
        /// If no file is specified, random data will be sent.
        #[clap(short, long)]
        source: Option<PathBuf>,
    },
    /// Receive a file line by line in TCP.
    Receive {
        /// The address that will be used to receive the file.
        #[clap(short, long, default_value = "127.0.0.1:80")]
        address: SocketAddrV4,
        /// The path to the file to store the received data.
        /// If no file is specified, the data will be printed to stdout.
        #[clap(short, long)]
        file: Option<PathBuf>,
    },
}

fn create_device(device: &Option<String>) -> Result<AsioDevice> {
    let device = match device {
        Some(name) => AsioDevice::try_from_name(name)?,
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = RatewayCli::parse();
    match cli.subcmd {
        SubCommand::Install { config } => {
            let config = fs::read_to_string(config)?;
            let config: RatewayAdapterConfig = toml::from_str(&config)?;

            let device = create_device(&config.socket_config.device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let adapter_config = translate_adapter(config, ather_config);
            let adapter = AtewayIoAdaper::new(adapter_config, device);
            adapter.await?;
        }
        SubCommand::Nat { config } => {
            let config = fs::read_to_string(config)?;
            let config: RatewayNatConfig = toml::from_str(&config)?;

            let device = create_device(&config.socket_config.device)?;
            let stream_config = create_stream_config(&device)?;
            let ather_config = AtherStreamConfig::new(24000, stream_config.clone());

            let nat_config = translate_nat(config, ather_config)?;
            let nat = AtewayIoNat::new(nat_config, device);
            nat.await?;
        }
        SubCommand::Udp { cmd } => match cmd {
            UdpSubCommand::Send {
                address,
                peer,
                source,
            } => {
                let socket = UdpSocket::bind(address).await?;
                if let Some(source) = source {
                    let source = fs::read_to_string(source)?;
                    for line in source.lines() {
                        socket.send_to(line.as_bytes(), peer).await?;
                    }
                } else {
                    eprintln!("No source file specified, sending random data.");
                    let mut rng = SmallRng::from_entropy();
                    let mut buffer = [0u8; 20];
                    loop {
                        for byte in buffer.iter_mut() {
                            *byte = rng.gen_range(0x20..0x7F) as u8;
                        }
                        socket.send_to(&buffer, peer).await?;
                        time::sleep(Duration::from_millis(1000)).await;
                    }
                }
            }
            UdpSubCommand::Receive { address, file } => {
                let socket = UdpSocket::bind(address).await?;
                if let Some(file) = file {
                    let mut file = fs::File::create(file)?;
                    let mut buffer = vec![0u8; 1024];
                    loop {
                        let (size, _) = socket.recv_from(&mut buffer).await?;
                        file.write_all(&buffer[..size])?;
                    }
                } else {
                    eprintln!("No file specified, printing to stdout.");
                    let mut buffer = [0u8; 1024];
                    loop {
                        let (size, addr) = socket.recv_from(&mut buffer).await?;
                        println!(
                            "From {} received: {}",
                            addr,
                            String::from_utf8_lossy(&buffer[..size])
                        );
                    }
                }
            }
        },
        SubCommand::Ping {
            peer,
            address,
            elapsed,
            port,
            length,
        } => {
            ping::ping(address, peer, port, Duration::from_secs(elapsed), length).await?;
        }
        SubCommand::Tcp { cmd } => match cmd {
            TcpSubCommand::Receive { address, file } => {
                let listener = TcpListener::bind(address).await?;
                println!("Listening on {}", address);
                let (socket, addr) = listener.accept().await?;
                println!("Accepted connection from {}", addr);
                if let Some(file) = file {
                    let mut file = fs::File::create(file)?;
                    let mut buffer = vec![0u8; 1024];
                    loop {
                        socket.readable().await?;
                        match socket.try_read(&mut buffer) {
                            Ok(0) => break,
                            Ok(size) => {
                                file.write_all(&buffer[..size])?;
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                return Err(e.into());
                            }
                        }
                    }
                    println!("Connection closed by peer.");
                } else {
                    eprintln!("No file specified, printing to stdout.");
                    let mut buffer = [0u8; 1024];
                    loop {
                        socket.readable().await?;
                        match socket.try_read(&mut buffer) {
                            Ok(0) => break,
                            Ok(size) => {
                                println!(
                                    "From {} received: {}",
                                    addr,
                                    String::from_utf8_lossy(&buffer[..size])
                                );
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                return Err(e.into());
                            }
                        }
                    }
                }
            }
            TcpSubCommand::Send {
                address,
                peer,
                source,
            } => {
                let socket = TcpSocket::new_v4()?;
                if let Some(address) = address {
                    socket.bind(address.into())?;
                }
                println!("Connecting to {}", peer);
                let mut stream = socket.connect(peer.into()).await?;
                println!("Established connection to {}", peer);
                if let Some(source) = source {
                    let source = fs::read_to_string(source)?;
                    for line in source.lines() {
                        loop {
                            stream.writable().await?;
                            match stream.try_write(line.as_bytes()) {
                                Ok(_) => break,
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                    continue;
                                }
                                Err(e) => {
                                    return Err(e.into());
                                }
                            }
                        }
                        time::sleep(Duration::from_millis(400)).await;
                    }
                    stream.shutdown().await?;
                } else {
                    eprintln!("No source file specified, sending random data.");
                    let mut rng = SmallRng::from_entropy();
                    let mut buffer = [0u8; 20];
                    loop {
                        for byte in buffer.iter_mut() {
                            *byte = rng.gen_range(0x20..0x7F) as u8;
                        }
                        loop {
                            stream.writable().await?;
                            match stream.try_write(&buffer) {
                                Ok(_) => break,
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                                    continue;
                                }
                                Err(e) => {
                                    return Err(e.into());
                                }
                            }
                        }
                        time::sleep(Duration::from_millis(1000)).await;
                    }
                }
            }
        },
    }

    Ok(())
}

#[derive(Clone, Deserialize, Debug)]
struct RatewayAdapterConfig {
    name: String,
    #[serde(rename = "ip")]
    address: Ipv4Addr,
    netmask: Ipv4Addr,
    gateway: Ipv4Addr,
    #[serde(rename = "socket")]
    socket_config: RatewaySocketConfig,
}

#[derive(Clone, Deserialize, Debug)]
struct RatewaySocketConfig {
    #[serde(rename = "mac", deserialize_with = "deserialize_mac")]
    address: usize,
    device: Option<String>,
}

#[derive(Clone, Deserialize, Debug)]
struct RatewayNatConfig {
    name: String,
    #[serde(rename = "ip")]
    address: Ipv4Addr,
    netmask: Ipv4Addr,
    host: Ipv4Addr,
    #[serde(rename = "socket")]
    socket_config: RatewaySocketConfig,
    #[serde(rename = "routes")]
    route_config: Option<HashMap<String, SocketAddrV4>>,
}

fn translate_adapter(
    config: RatewayAdapterConfig,
    ather_config: AtherStreamConfig,
) -> AtewayAdapterConfig {
    AtewayAdapterConfig::new(
        config.name,
        config.address,
        config.netmask,
        config.gateway,
        AcsmaSocketConfig::new(
            config.socket_config.address,
            Some(u32::from_be_bytes(config.address.octets()) as usize),
            ather_config,
        ),
    )
}

fn translate_nat(
    config: RatewayNatConfig,
    ather_config: AtherStreamConfig,
) -> Result<AtewayNatConfig> {
    let route_config = match config.route_config {
        Some(routes) => {
            let mut map = HashMap::new();
            for (name, address) in routes {
                map.insert(name.parse()?, address);
            }
            Some(map)
        }
        None => None,
    };
    Ok(AtewayNatConfig::new(
        config.name,
        config.address,
        config.netmask,
        config.host,
        AcsmaSocketConfig::new(
            config.socket_config.address,
            Some(u32::from_be_bytes(config.address.octets()) as usize),
            ather_config,
        ),
        route_config,
    ))
}

fn deserialize_mac<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mac = String::deserialize(deserializer)?;
    mac.parse::<usize>().map_err(Error::custom)
}
