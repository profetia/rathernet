use super::builtin::SOCKET_IP_MTU;
use crate::{
    racsma::{
        builtin::SOCKET_BROADCAST_ADDRESS, AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader,
        AcsmaSocketWriter,
    },
    rather::encode::{DecodeToBytes, EncodeFromBytes},
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::{
    future::BoxFuture,
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use ipnet::Ipv4Net;
use packet::{
    icmp,
    ip::{self, v4::Packet as Ipv4Packet, Protocol},
    Builder, Packet, PacketMut,
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

    let write_handle = tokio::spawn(write_daemon(config.clone(), tx_socket, write_rx));
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

pub(super) type AtewayAdapterTask = (Ipv4Packet<Vec<u8>>, Sender<Result<()>>);

async fn write_daemon(
    config: AtewayAdapterConfig,
    tx_socket: AcsmaSocketWriter,
    mut write_rx: UnboundedReceiver<AtewayAdapterTask>,
) -> Result<()> {
    let gateway_mac = tx_socket
        .arp(u32::from_be_bytes(config.gateway.octets()) as usize)
        .await?;
    let net = Ipv4Net::with_netmask(config.address, config.netmask)?;
    while let Some((packet, tx)) = write_rx.recv().await {
        let ip = packet.destination();
        let dest = if ip == net.broadcast() || ip.is_broadcast() {
            Ok(SOCKET_BROADCAST_ADDRESS)
        } else if net.contains(&ip) {
            log::debug!("Resolving MAC address: {}", ip);
            tx_socket
                .arp(u32::from_be_bytes(ip.octets()) as usize)
                .await
        } else {
            Ok(gateway_mac)
        };

        let result = match dest {
            Ok(inner) => {
                log::debug!("Resolve MAC address: {} -> {}", ip, inner);
                write_packet_fragmented(&tx_socket, packet, inner).await
            }
            Err(err) => Err(err),
        };
        tx.send(result).ok();
    }
    Ok(())
}

pub(super) async fn write_packet_fragmented(
    tx_socket: &AcsmaSocketWriter,
    packet: Ipv4Packet<Vec<u8>>,
    dest: usize,
) -> Result<()> {
    let packets = if packet.flags().contains(ip::v4::Flags::DONT_FRAGMENT) {
        log::debug!("Don't fragment is set, send packet as is");
        vec![packet.as_ref().to_owned()]
    } else {
        let chunks = packet.payload().chunks(SOCKET_IP_MTU);
        let mut packets = vec![vec![]; chunks.len()];
        let mut offset = packet.offset();
        for (index, chunk) in chunks.enumerate() {
            let mut builder = ip::v4::Builder::default()
                .dscp(packet.dscp())?
                .ecn(packet.ecn())?
                .id(packet.id())?
                .ttl(packet.ttl())?
                .protocol(packet.protocol())?
                .source(packet.source())?
                .destination(packet.destination())?
                .payload(chunk)?
                .offset(offset)?;

            if index != packets.len() - 1 {
                builder = builder.flags(ip::v4::Flags::MORE_FRAGMENTS)?
            } else {
                builder = builder.flags(packet.flags())?
            };

            packets[index] = builder.build()?;
            offset += (chunk.len() / 8) as u16;
        }
        log::debug!("Fragment packet into {} packets", packets.len());
        packets
    };
    for packet in packets {
        tx_socket.write(dest, &packet.encode()).await?;
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
        if let Ok(ip::Packet::V4(mut packet)) = ip::Packet::new(bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();

            if dest == config.address {
                if protocol == Protocol::Icmp {
                    log::debug!("Receive packet {} -> {} ({:?})", src, dest, protocol);
                    let mut icmp = icmp::Packet::new(packet.payload_mut())?;
                    if let Some(mut echo) = icmp.echo_mut().ok().filter(|echo| echo.is_request()) {
                        log::debug!("Reply to ICMP echo request");
                        echo.make_reply()?.checked();
                        packet
                            .set_destination(src)?
                            .set_source(dest)?
                            .update_checksum()?;

                        if send_packet(&write_tx, packet).await.is_err() {
                            log::debug!("Packet dropped");
                        }
                        continue;
                    }
                } else if protocol == Protocol::Udp {
                    log::debug!("Receive packet {} -> {} ({:?})", src, dest, protocol);
                } else if protocol == Protocol::Tcp {
                    log::debug!("Receive packet {} -> {} ({:?})", src, dest, protocol);
                } else {
                    continue;
                }

                let packet = TunPacket::new(packet.as_ref().to_owned());
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
        if let Ok(ip::Packet::V4(packet)) = ip::Packet::new(bytes) {
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();

            if src == config.address {
                if protocol != Protocol::Icmp
                    && protocol != Protocol::Udp
                    && protocol != Protocol::Tcp
                {
                    continue;
                }
                log::debug!("Send packet {} -> {} ({:?})", src, dest, protocol);

                if send_packet(&write_tx, packet.to_owned()).await.is_err() {
                    log::debug!("Packet dropped");
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn send_packet(
    write_tx: &UnboundedSender<AtewayAdapterTask>,
    packet: Ipv4Packet<Vec<u8>>,
) -> Result<()> {
    let (tx, rx) = oneshot::channel();
    write_tx.send((packet, tx))?;
    rx.await??;
    Ok(())
}
