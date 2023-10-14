use crate::{
    racsma::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader, AcsmaSocketWriter},
    rather::encode::{DecodeToBytes, EncodeFromBytes},
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::{
    future::LocalBoxFuture,
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use packet::{icmp, ip, Builder, Packet};
use std::{
    future::Future,
    net::Ipv4Addr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::task::JoinHandle;
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
    inner: Option<LocalBoxFuture<'static, Result<()>>>,
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
    let dev = tun::create_as_async(&tun_config)?;
    let (tx_tun, rx_tun) = dev.into_framed().split();

    let receive_handle = tokio::spawn(receive_daemon(config, tx_socket.clone(), rx_socket, tx_tun));
    let send_handle = tokio::spawn(send_daemon(tx_socket, rx_tun));

    tokio::try_join!(flatten(receive_handle), flatten(send_handle))?;
    Ok(())
}

pub async fn flatten<T>(handle: JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(err.into()),
    }
}

async fn receive_daemon(
    config: AtewayAdapterConfig,
    tx_socket: AcsmaSocketWriter,
    mut rx_socket: AcsmaSocketReader,
    mut tx_tun: SplitSink<Framed<AsyncDevice, TunPacketCodec>, TunPacket>,
) -> Result<()> {
    while let Ok(packet) = rx_socket.read_packet_unchecked().await {
        let bytes = DecodeToBytes::decode(&packet);
        // log::debug!("Receive packet: {}", bytes.len());
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
                            tx_socket.write_packet_unchecked(&reply.encode()).await?;
                            continue;
                        }
                    }
                }

                let packet = TunPacket::new(bytes);
                tx_tun.send(packet).await?;
            }
        }
    }
    Ok(())
}

async fn send_daemon(
    tx_socket: AcsmaSocketWriter,
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
            let bits = bytes.encode();
            tx_socket.write_packet_unchecked(&bits).await?;
        }
    }
    Ok(())
}

pub async fn create_reply(
    id: u16,
    src: Ipv4Addr,
    dest: Ipv4Addr,
    req: icmp::echo::Packet<&&[u8]>,
) -> Result<Vec<u8>> {
    let reply = ip::v4::Builder::default()
        .id(id)?
        .ttl(64)?
        .source(src)?
        .destination(dest)?
        .icmp()?
        .echo()?
        .reply()?
        .identifier(req.identifier())?
        .sequence(req.sequence())?
        .payload(req.payload())?
        .build()?;
    Ok(reply)
}
