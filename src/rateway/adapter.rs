use crate::{
    racsma::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader, AcsmaSocketWriter},
    rather::encode::{DecodeToBytes, EncodeFromBytes},
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::{
    future::BoxFuture,
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use packet::{
    icmp,
    ip::{self, Protocol},
    Packet,
};
use std::{
    future::Future,
    net::Ipv4Addr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        oneshot::{self, Sender},
    },
    task::JoinHandle,
};
use tokio_util::codec::Framed;
use tun::{AsyncDevice, Configuration, TunPacket, TunPacketCodec};

#[derive(Clone)]
pub struct AtewayAdapterConfig {
    pub name: String,
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub gateway: Ipv4Addr,
    pub socket_config: AcsmaSocketConfig,
}

impl AtewayAdapterConfig {
    pub fn new(
        name: String,
        address: Ipv4Addr,
        netmask: Ipv4Addr,
        gateway: Ipv4Addr,
        socket_config: AcsmaSocketConfig,
    ) -> Self {
        Self {
            name,
            address,
            netmask,
            gateway,
            socket_config,
        }
    }
}

pub struct AtewayIoAdaper {
    config: AtewayAdapterConfig,
    device: AsioDevice,
    inner: Option<BoxFuture<'static, Result<()>>>,
}

impl AtewayIoAdaper {
    pub fn new(config: AtewayAdapterConfig, device: AsioDevice) -> Self {
        Self {
            config,
            device,
            inner: None,
        }
    }
}

impl Future for AtewayIoAdaper {
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

async fn adapter_daemon(config: AtewayAdapterConfig, device: AsioDevice) -> Result<()> {
    let (tx_socket, rx_socket) =
        AcsmaIoSocket::try_from_device(config.socket_config.clone(), &device)?;

    let dev = {
        let mut tun_config = Configuration::default();
        tun_config
            .name(&config.name)
            .address(config.address)
            .netmask(config.netmask)
            .up();
        #[cfg(target_os = "windows")]
        tun_config.platform(|config| {
            config.initialize();
        });
        tun::create_as_async(&tun_config)
    }?;
    let (tx_tun, rx_tun) = dev.into_framed().split();

    let (write_tx, write_rx) = mpsc::unbounded_channel();

    let write_handle = tokio::spawn(write_daemon(tx_socket, write_rx));
    let receive_handle = tokio::spawn(receive_daemon(
        config.clone(),
        write_tx.clone(),
        rx_socket,
        tx_tun,
    ));
    let send_handle = tokio::spawn(send_daemon(config, write_tx, rx_tun));

    tokio::try_join!(
        flatten(write_handle),
        flatten(receive_handle),
        flatten(send_handle)
    )?;
    Ok(())
}

pub(super) async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err.into()),
    }
}

pub(super) type AtewayAdapterTask = (Vec<u8>, Sender<Result<()>>);

pub(super) async fn write_daemon(
    tx_socket: AcsmaSocketWriter,
    mut write_rx: UnboundedReceiver<AtewayAdapterTask>,
) -> Result<()> {
    while let Some((bytes, tx)) = write_rx.recv().await {
        let result = tx_socket.write_unchecked(&bytes.encode()).await;
        let _ = tx.send(result);
    }
    Ok(())
}

async fn receive_daemon(
    config: AtewayAdapterConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    mut rx_socket: AcsmaSocketReader,
    mut tx_tun: SplitSink<Framed<AsyncDevice, TunPacketCodec>, TunPacket>,
) -> Result<()> {
    while let Ok(packet) = rx_socket.read_unchecked().await {
        let bytes = DecodeToBytes::decode(&packet);
        if let Ok(ip::Packet::V4(packet)) = ip::Packet::new(&bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();

            log::info!("Receive packet {} -> {} ({:?})", src, dest, protocol);
            if dest == config.address {
                if protocol == Protocol::Icmp {
                    let icmp = icmp::Packet::new(packet.payload())?;
                    if let Some(echo) = icmp.echo().ok().filter(|echo| echo.is_request()) {
                        log::debug!("Reply to ICMP echo request");
                        let mut reply = echo.to_owned();
                        reply.make_reply()?;
                        write_packet(&write_tx, reply.as_ref().to_owned()).await?;
                        continue;
                    }
                } else if protocol == Protocol::Udp {
                } else {
                    continue;
                }

                let packet = TunPacket::new(bytes);
                tx_tun.send(packet).await?;
            }
        }
    }
    Ok(())
}

async fn send_daemon(
    config: AtewayAdapterConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    mut rx_tun: SplitStream<Framed<AsyncDevice, TunPacketCodec>>,
) -> Result<()> {
    while let Some(Ok(packet)) = rx_tun.next().await {
        let bytes = packet.get_bytes();
        // log::debug!("Receive packet: {}", bytes.len());
        if let Ok(ip::Packet::V4(packet)) = ip::Packet::new(bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();
            log::info!("Send packet {} -> {} ({:?})", src, dest, protocol);

            if src == config.address {
                if protocol != Protocol::Icmp && protocol != Protocol::Udp {
                    continue;
                }

                let (tx, rx) = oneshot::channel();
                write_tx.send((bytes.to_owned(), tx))?;
                rx.await??;
            }
        }
    }
    Ok(())
}

pub(super) async fn write_packet(
    write_tx: &UnboundedSender<AtewayAdapterTask>,
    bytes: Vec<u8>,
) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    write_tx.send((bytes, tx))?;
    rx.await??;
    Ok(())
}
