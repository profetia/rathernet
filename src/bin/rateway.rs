use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use cpal::SupportedStreamConfig;
use env_logger;
use rand::{rngs::SmallRng, Rng, SeedableRng};
use rathernet::{
    racsma::AcsmaSocketConfig,
    rateway::{
        builtin::{CALIBRATE_BUFFER_SIZE, CALIBRATE_SEND_INTERVAL},
        AtewayAdapterConfig, AtewayIoAdaper, AtewayIoNat, AtewayNatConfig,
    },
    rather::AtherStreamConfig,
    raudio::AsioDevice,
};
use rodio::DeviceTrait;
use serde::{de::Error, Deserialize};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
};
use tokio::{net::UdpSocket, time};

#[derive(Parser, Debug)]
#[clap(name = "rateway", version = "0.1.0", author = "Rathernet")]
#[clap(about = "A command line interface for rathernet rateway", long_about = None)]
struct RatewayCli {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    /// Calibrate the ateway by transmitting a file in UDP.
    Calibrate {
        /// The address that will be used to send the file.
        #[clap(short, long, default_value = "127.0.0.1:8080")]
        address: String,
        /// The peer address that will receive the file.
        #[clap(short, long, default_value = "127.0.0.1:8080")]
        peer: String,
        /// The type of calibration to perform.
        #[clap(short, long, default_value = "duplex")]
        r#type: CalibrateType,
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
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CalibrateType {
    Read,
    Write,
    Duplex,
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
        SubCommand::Calibrate {
            address,
            peer,
            r#type,
        } => {
            let dest = SocketAddr::from(SocketAddrV4::from_str(&peer)?);
            let socket = UdpSocket::bind(address).await?;
            socket.connect(dest).await?;

            let send_future = calibrate_send(&socket);
            let receive_future = calibrate_receive(&socket, &dest);

            match r#type {
                CalibrateType::Read => receive_future.await?,
                CalibrateType::Write => send_future.await?,
                CalibrateType::Duplex => {
                    tokio::try_join!(send_future, receive_future)?;
                }
            }
        }
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

            let nat_config = translate_nat(config, ather_config);
            let nat = AtewayIoNat::new(nat_config, device);
            nat.await?;
        }
    }

    Ok(())
}

async fn calibrate_send(socket: &UdpSocket) -> Result<()> {
    let mut rng = SmallRng::from_entropy();
    let mut buf = [0u8; CALIBRATE_BUFFER_SIZE];
    loop {
        rng.fill(&mut buf);
        socket.send(&buf).await?;
        println!("Sent {} bytes", buf.len());
        println!("{:?}", &buf);
        time::sleep(CALIBRATE_SEND_INTERVAL).await;
    }
}

async fn calibrate_receive(socket: &UdpSocket, dest: &SocketAddr) -> Result<()> {
    let mut buf = [0u8; CALIBRATE_BUFFER_SIZE];
    loop {
        let len = socket.recv(&mut buf).await?;
        println!("Received {} bytes from {}", len, dest);
        println!("{:?}", &buf[..len]);
    }
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
    #[serde(deserialize_with = "deserialize_port")]
    port: u16,
    netmask: Ipv4Addr,
    #[serde(rename = "socket")]
    socket_config: RatewaySocketConfig,
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
        AcsmaSocketConfig::new(config.socket_config.address, ather_config),
    )
}

fn translate_nat(config: RatewayNatConfig, ather_config: AtherStreamConfig) -> AtewayNatConfig {
    AtewayNatConfig::new(
        config.name,
        config.address,
        config.port,
        config.netmask,
        AcsmaSocketConfig::new(config.socket_config.address, ather_config),
    )
}

fn deserialize_port<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let port = String::deserialize(deserializer)?;
    port.parse::<u16>().map_err(Error::custom)
}

fn deserialize_mac<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let mac = String::deserialize(deserializer)?;
    mac.parse::<usize>().map_err(Error::custom)
}
