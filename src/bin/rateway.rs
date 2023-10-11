use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use rand::{rngs::SmallRng, Rng, SeedableRng};
use rathernet::rateway::builtin::{CALIBRATE_BUFFER_SIZE, CALIBRATE_SEND_INTERVAL};
use std::{
    net::{SocketAddr, SocketAddrV4},
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
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum CalibrateType {
    Read,
    Write,
    Duplex,
}

#[tokio::main]
async fn main() -> Result<()> {
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
