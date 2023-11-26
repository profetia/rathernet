use super::super::nat::find_device;
use super::super::AtewayIoSocket;
use anyhow::Result;
use packet::{ether, icmp, ip, Builder, Packet};
use pcap::{Active, Capture, Error};
use rand::Rng;
use std::{
    net::Ipv4Addr,
    time::{Duration, Instant},
};
use tokio::{
    sync::{
        mpsc::{self, error::TryRecvError, UnboundedReceiver},
        oneshot::{self, Sender},
    },
    task, time,
};

pub async fn ping(
    address: Ipv4Addr,
    peer: Ipv4Addr,
    port: Option<u16>,
    timeout: Duration,
    length: usize,
) -> Result<()> {
    let mut socket = AtewayIoSocket::try_new(address)?;

    let device = find_device(address)?;
    let mut cap = Capture::from_device(device)?
        .immediate_mode(true)
        .open()?
        .setnonblock()?;
    cap.filter(
        &format!("ip dst host {} && ip src host {} && icmp", address, peer),
        true,
    )?;

    let (task_tx, task_rx) = mpsc::unbounded_channel::<PingTask>();
    task::spawn_blocking(move || ping_daemon(cap, task_rx));

    let mut rng = rand::thread_rng();
    loop {
        let mut payload = vec![0; length];
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

        let (tx, rx) = oneshot::channel();
        task_tx.send((tx, port))?;
        let start = Instant::now();
        socket.send(packet.as_ref())?;
        if let Ok(Ok(packet)) = time::timeout(timeout, rx).await {
            let icmp = icmp::Packet::new(packet.payload())?;
            println!(
                "Reply from {}: bytes={} time={:?}ms TTL={}",
                packet.source(),
                icmp.payload().len(),
                start.elapsed().as_millis(),
                packet.ttl()
            );
            time::sleep(timeout).await;
        } else {
            println!("Request timeout.");
        }
    }
}

type PingTask = (Sender<ip::v4::Packet<Vec<u8>>>, u16);

fn ping_daemon(mut cap: Capture<Active>, mut task_rx: UnboundedReceiver<PingTask>) -> Result<()> {
    let mut task = None;
    loop {
        match task_rx.try_recv() {
            Ok(inner) => task = Some(inner),
            Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {}
        }
        match cap.next_packet() {
            Ok(packet) => {
                if let Some((_, port)) = task {
                    let ether = ether::Packet::new(packet.data)?;
                    let packet = ip::v4::Packet::new(ether.payload().to_owned())?;
                    let icmp = icmp::Packet::new(packet.payload())?;
                    let echo = icmp.echo()?;
                    if echo.is_reply() && echo.identifier() == port {
                        let (sender, _) = task.take().unwrap();
                        let _ = sender.send(packet);
                    }
                }
            }
            Err(Error::TimeoutExpired) => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}
