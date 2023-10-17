use super::builtin::NAT_SENDTO_PLACEHOLDER;
use super::nat::find_device;
use anyhow::Result;
use packet::{ether, icmp, ip, Builder, Packet};
use pcap::{Active, Capture};
use rand::Rng;
use socket2::{Domain, Socket, Type};
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use tokio::{task, time};

pub async fn ping(
    address: Ipv4Addr,
    peer: Ipv4Addr,
    port: Option<u16>,
    timeout: Duration,
) -> Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::RAW, None)?;
    socket.set_header_included(true)?;
    let device = find_device(address)?;
    let mut cap = Capture::from_device(device)?.immediate_mode(true).open()?;
    cap.filter(
        &format!("ip dst host {} && ip src host {} && icmp", address, peer),
        true,
    )?;

    let mut rng = rand::thread_rng();
    loop {
        let mut payload = [0u8; 32];
        for item in payload.iter_mut() {
            *item = rng.gen_range(0x20..0x7f);
        }
        let id = rng.gen();
        let port = port.unwrap_or_else(|| rng.gen_range(0x8000..0xffff));

        let packet = ip::v4::Builder::default()
            .id(id)?
            .ttl(64)?
            .source(address)?
            .destination(peer)?
            .icmp()?
            .echo()?
            .request()?
            .identifier(port)?
            .sequence(0)?
            .payload(&payload)?
            .build()?;
        socket.send_to(packet.as_ref(), &NAT_SENDTO_PLACEHOLDER.into())?;

        tokio::select! {
            _ = time::sleep(timeout) => {
                println!("Request timed out.");
            }
            err = ping_daemon(&mut cap, port) => {
                err?;
                time::sleep(timeout).await;
            }
        }
    }
}

async fn ping_daemon(cap: &mut Capture<Active>, port: u16) -> Result<()> {
    let start = Instant::now();
    while let Ok(packet) = task::block_in_place(|| cap.next_packet()) {
        let ether = ether::Packet::new(packet.data)?;
        let ip: ip::v4::Packet<&[u8]> = ip::v4::Packet::new(ether.payload())?;
        let icmp = icmp::Packet::new(ip.payload())?;
        let echo = icmp.echo()?;
        if echo.is_reply() && echo.identifier() == port {
            println!(
                "Reply from {}: bytes={} time={:?} TTL={}",
                ip.source(),
                icmp.payload().len(),
                start.elapsed().as_millis(),
                ip.ttl()
            );
            break;
        }
    }
    Ok(())
}
