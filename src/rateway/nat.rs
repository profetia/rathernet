use super::{
    adapter::{create_icmp_reply, flatten, write_daemon, AtewayAdapterTask},
    builtin::NAT_PORT_RANGE,
};
use crate::{
    racsma::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader},
    rateway::builtin::NAT_SENDTO_PLACEHOLDER,
    rather::encode::DecodeToBytes,
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::future::BoxFuture;
use ipnet::Ipv4Net;
use lru::LruCache;
use packet::{icmp, ip, Packet};
use parking_lot::Mutex;
use socket2::{Domain, Socket, Type};
use std::{
    future::Future,
    mem::MaybeUninit,
    net::Ipv4Addr,
    num::NonZeroUsize,
    ops::Range,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    oneshot,
};

#[derive(Clone)]
pub struct AtewayNatConfig {
    pub name: String,
    pub address: Ipv4Addr,
    pub port: u16,
    pub netmask: Ipv4Addr,
    pub host: Ipv4Addr,
    pub socket_config: AcsmaSocketConfig,
}

impl AtewayNatConfig {
    pub fn new(
        name: String,
        address: Ipv4Addr,
        port: u16,
        netmask: Ipv4Addr,
        host: Ipv4Addr,
        socket_config: AcsmaSocketConfig,
    ) -> Self {
        Self {
            name,
            address,
            port,
            netmask,
            host,
            socket_config,
        }
    }
}

pub struct AtewayIoNat {
    config: AtewayNatConfig,
    device: AsioDevice,
    inner: Option<BoxFuture<'static, Result<()>>>,
}

impl AtewayIoNat {
    pub fn new(config: AtewayNatConfig, device: AsioDevice) -> Self {
        Self {
            config,
            device,
            inner: None,
        }
    }
}

impl Future for AtewayIoNat {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.inner.is_none() {
            let config = this.config.clone();
            let device = this.device.clone();
            let inner = Box::pin(nat_daemon(config, device));
            this.inner.replace(inner);
        }
        let inner = this.inner.as_mut().unwrap();
        match inner.as_mut().poll(cx) {
            Poll::Ready(result) => {
                this.inner.take();
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

async fn nat_daemon(config: AtewayNatConfig, device: AsioDevice) -> Result<()> {
    let table = Arc::new(Mutex::new(AtewayNatTable::new(NAT_PORT_RANGE)));

    let (tx_socket, rx_socket) =
        AcsmaIoSocket::try_from_device(config.socket_config.clone(), &device)?;

    let tunnel = Arc::new(Socket::new(Domain::IPV4, Type::RAW, None)?);
    tunnel.set_header_included(true)?;

    let (write_tx, write_rx) = mpsc::unbounded_channel();

    let write_handle = tokio::spawn(write_daemon(tx_socket, write_rx));
    let receive_handle = tokio::spawn(receive_daemon(
        config.clone(),
        write_tx.clone(),
        rx_socket,
        tunnel.clone(),
        table.clone(),
    ));
    let send_handle = tokio::spawn(send_daemon(config, write_tx, tunnel, table));

    tokio::try_join!(
        flatten(write_handle),
        flatten(receive_handle),
        flatten(send_handle)
    )?;
    Ok(())
}

async fn receive_daemon(
    config: AtewayNatConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    mut rx_socket: AcsmaSocketReader,
    tunnel: Arc<Socket>,
    table: Arc<Mutex<AtewayNatTable>>,
) -> Result<()> {
    let net = Ipv4Net::with_netmask(config.address, config.netmask)?;

    while let Ok(packet) = rx_socket.read_unchecked().await {
        let bytes = DecodeToBytes::decode(&packet);
        if let Ok(ip::Packet::V4(packet)) = ip::Packet::new(&bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();
            log::info!("Receive packet {} -> {} ({:?})", src, dest, protocol);

            if dest == config.address {
                if let Ok(icmp) = icmp::Packet::new(packet.payload()) {
                    if let Ok(echo) = icmp.echo() {
                        if echo.is_request() {
                            log::debug!("Receive ICMP echo request");
                            let reply = create_icmp_reply(packet.id(), dest, src, echo).await?;

                            let (tx, rx) = oneshot::channel();
                            write_tx.send((reply, tx))?;
                            rx.await??;
                            continue;
                        }
                    }
                }
            } else if !net.contains(&dest) {
                let mut packet = packet.to_owned();
                packet.set_source(config.host)?;

                if icmp::Packet::new(packet.payload()).is_ok() {
                    let mut guard = table.lock();
                    let id = guard.forward((src, packet.id()));
                    packet.set_id(id)?;
                }

                packet.update_checksum()?;
                tunnel.send_to(packet.as_ref(), &NAT_SENDTO_PLACEHOLDER.into())?;
            }
        }
    }
    Ok(())
}

async fn send_daemon(
    config: AtewayNatConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    tunnel: Arc<Socket>,
    table: Arc<Mutex<AtewayNatTable>>,
) -> Result<()> {
    while let Ok(bytes) = read_packet(tunnel.clone()).await {
        if let Ok(ip::Packet::V4(mut packet)) = ip::Packet::new(bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();
            log::info!("Send packet {} -> {} ({:?})", src, dest, protocol);

            if dest == config.host {
                if icmp::Packet::new(packet.payload()).is_ok() {
                    let mut guard = table.lock();
                    if let Some((src, id)) = guard.backward(packet.id()) {
                        packet.set_source(src)?;
                        packet.set_id(id)?;
                    } else {
                        continue;
                    }
                }

                packet.update_checksum()?;
                let (tx, rx) = oneshot::channel();
                write_tx.send((packet.as_ref().to_owned(), tx))?;
                rx.await??;
            }
        }
    }

    Ok(())
}

async fn read_packet(tunnel: Arc<Socket>) -> Result<Vec<u8>> {
    let mut raw = [MaybeUninit::uninit(); u16::MAX as usize];
    let (len, _) = tokio::task::spawn_blocking(move || tunnel.recv_from(&mut raw)).await??;
    let bytes = raw.map(|x| unsafe { x.assume_init() })[..len].to_vec();

    Ok(bytes)
}

type AtewayNatEntry = (Ipv4Addr, u16);

struct AtewayNatDynTable(LruCache<u16, Option<AtewayNatEntry>>);

impl AtewayNatDynTable {
    fn new(range: Range<u16>) -> Self {
        let len = range.len();
        let mut table = if len > 0 {
            LruCache::new(NonZeroUsize::new(len).unwrap())
        } else {
            LruCache::unbounded()
        };
        for port in range {
            table.put(port, None);
        }
        Self(table)
    }

    fn forward(&mut self, entry: AtewayNatEntry) -> u16 {
        let pair = self.0.iter().find(|(_, &v)| v == Some(entry));
        match pair {
            Some((&port, _)) => {
                self.0.promote(&port);
                port
            }
            None => {
                let (port, _) = self.0.pop_lru().unwrap();
                self.0.put(port, Some(entry));
                port
            }
        }
    }

    fn backward(&mut self, port: u16) -> Option<AtewayNatEntry> {
        match self.0.get(&port) {
            Some(Some(entry)) => Some(*entry),
            _ => None,
        }
    }
}

struct AtewayNatTable {
    dyn_table: AtewayNatDynTable,
}

impl AtewayNatTable {
    fn new(range: Range<u16>) -> Self {
        Self {
            dyn_table: AtewayNatDynTable::new(range),
        }
    }

    fn forward(&mut self, entry: AtewayNatEntry) -> u16 {
        self.dyn_table.forward(entry)
    }

    fn backward(&mut self, port: u16) -> Option<AtewayNatEntry> {
        self.dyn_table.backward(port)
    }
}
