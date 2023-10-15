use super::adapter::{create_reply, flatten, write_daemon, AtewayAdapterTask};
use crate::{
    racsma::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader},
    rather::encode::DecodeToBytes,
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::future::LocalBoxFuture;
use packet::{icmp, ip, Packet};
use socket2::{Domain, Socket, Type};
use std::{
    future::Future,
    mem::{self, MaybeUninit},
    net::{Ipv4Addr, SocketAddr},
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
    pub socket_config: AcsmaSocketConfig,
}

impl AtewayNatConfig {
    pub fn new(
        name: String,
        address: Ipv4Addr,
        port: u16,
        netmask: Ipv4Addr,
        socket_config: AcsmaSocketConfig,
    ) -> Self {
        Self {
            name,
            address,
            port,
            netmask,
            socket_config,
        }
    }
}

pub struct AtewayIoNat {
    config: AtewayNatConfig,
    device: AsioDevice,
    inner: Option<LocalBoxFuture<'static, Result<()>>>,
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
            let inner = Box::pin(adapter_daemon(config, device));
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

async fn adapter_daemon(config: AtewayNatConfig, device: AsioDevice) -> Result<()> {
    let (tx_socket, rx_socket) =
        AcsmaIoSocket::try_from_device(config.socket_config.clone(), &device)?;

    let tunnel = Arc::new(Socket::new(Domain::IPV4, Type::RAW, None)?);
    tunnel.set_header_included(true)?;

    let (write_tx, write_rx) = mpsc::unbounded_channel();

    let write_handle = tokio::spawn(write_daemon(tx_socket, write_rx));
    let receive_handle = tokio::spawn(receive_daemon(
        config,
        write_tx.clone(),
        rx_socket,
        tunnel.clone(),
    ));
    let send_handle = tokio::spawn(send_daemon(write_tx, tunnel));

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
) -> Result<()> {
    let baidu = "110.242.68.66".parse()?;

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
                            let reply = create_reply(packet.id(), dest, src, echo).await?;

                            let (tx, rx) = oneshot::channel();
                            write_tx.send((reply, tx))?;
                            rx.await??;
                            continue;
                        }
                    }
                }

                let mut packet = packet.to_owned();
                packet
                    .set_source(config.address)?
                    .set_destination(baidu)? // TODO: replace this with a real one.
                    .update_checksum()?;
                tunnel.send_to(packet.as_ref(), &SocketAddr::new(baidu.into(), 0).into())?;
            }
        }
    }
    Ok(())
}

async fn send_daemon(
    write_tx: UnboundedSender<AtewayAdapterTask>,
    tunnel: Arc<Socket>,
) -> Result<()> {
    while let Ok(bytes) = read_packet(tunnel.clone()).await {
        if let Ok(ip::Packet::V4(mut packet)) = ip::Packet::new(bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();

            log::info!("Send packet {} -> {} ({:?})", src, dest, protocol);

            packet
                .set_source("192.168.1.3".parse()?)?
                .set_destination("192.168.1.2".parse()?)?
                .update_checksum()?;

            let (tx, rx) = oneshot::channel();
            write_tx.send((packet.as_ref().to_owned(), tx))?;
            rx.await??;
        }
    }

    Ok(())
}

async fn read_packet(tunnel: Arc<Socket>) -> Result<Vec<u8>> {
    let mut raw = [MaybeUninit::uninit(); 2048];
    let (len, _) = tokio::task::spawn_blocking(move || tunnel.recv_from(&mut raw)).await??;
    let bytes = unsafe { mem::transmute::<_, &[u8; 2048]>(&raw) }[..len].to_owned();

    Ok(bytes)
}
