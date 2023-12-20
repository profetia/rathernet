use super::{
    adapter::{flatten, write_packet, AtewayAdapterTask},
    AtewayIoError, AtewayIoSocket,
};
use crate::{
    racsma::{
        builtin::SOCKET_BROADCAST_ADDRESS, AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader,
        AcsmaSocketWriter,
    },
    rather::encode::{DecodeToBytes, EncodeFromBytes},
    raudio::AsioDevice,
};
use anyhow::Result;
use futures::future::BoxFuture;
use ipnet::Ipv4Net;
use packet::{
    ether, icmp,
    ip::{self, Protocol},
    Packet, PacketMut,
};
use pcap::{Active, Capture, Device};
use std::{
    collections::HashMap,
    future::Future,
    net::{Ipv4Addr, SocketAddrV4},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    sync::mpsc::{self, UnboundedReceiver, UnboundedSender},
    task,
};

#[derive(Clone)]
pub struct AtewayNatConfig {
    pub name: String,
    pub address: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub host: Ipv4Addr,
    pub socket_config: AcsmaSocketConfig,
    pub route_config: Option<HashMap<u16, SocketAddrV4>>,
}

impl AtewayNatConfig {
    pub fn new(
        name: String,
        address: Ipv4Addr,
        netmask: Ipv4Addr,
        host: Ipv4Addr,
        socket_config: AcsmaSocketConfig,
        route_config: Option<HashMap<u16, SocketAddrV4>>,
    ) -> Self {
        Self {
            name,
            address,
            netmask,
            host,
            socket_config,
            route_config,
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

pub(super) fn find_device(ip: Ipv4Addr) -> Result<Device> {
    for device in Device::list()? {
        if device.addresses.iter().any(|addr| addr.addr == ip) {
            return Ok(device);
        }
    }
    Err(AtewayIoError::DeviceNotFound(ip).into())
}

async fn nat_daemon(config: AtewayNatConfig, device: AsioDevice) -> Result<()> {
    let (tx_socket, rx_socket) =
        AcsmaIoSocket::try_from_device(config.socket_config.clone(), &device)?;

    let raw_socket = AtewayIoSocket::try_new(config.host)?;

    let device = find_device(config.host)?;
    let mut cap = Capture::from_device(device)?.immediate_mode(true).open()?;
    cap.filter("ip", true)?;

    let (write_tx, write_rx) = mpsc::unbounded_channel();

    let write_handle = tokio::spawn(write_daemon(config.clone(), tx_socket, write_rx));
    let receive_handle = tokio::spawn(receive_daemon(
        config.clone(),
        write_tx.clone(),
        rx_socket,
        raw_socket,
    ));
    let send_handle = tokio::spawn(send_daemon(config, write_tx, cap));

    tokio::try_join!(
        flatten(write_handle),
        flatten(receive_handle),
        flatten(send_handle)
    )?;
    Ok(())
}

async fn write_daemon(
    config: AtewayNatConfig,
    tx_socket: AcsmaSocketWriter,
    mut write_rx: UnboundedReceiver<AtewayAdapterTask>,
) -> Result<()> {
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
            Ok(config.socket_config.mac)
        };

        let result = match dest {
            Ok(inner) => {
                log::debug!("Resolve MAC address: {} -> {}", ip, inner);
                tx_socket.write(inner, &packet.as_ref().encode()).await
            }
            Err(err) => Err(err),
        };
        let _ = tx.send(result);
    }
    Ok(())
}

async fn receive_daemon(
    config: AtewayNatConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    mut rx_socket: AcsmaSocketReader,
    mut raw_socket: AtewayIoSocket,
) -> Result<()> {
    let net = Ipv4Net::with_netmask(config.address, config.netmask)?;

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
                        if let Err(err) = write_packet(&write_tx, packet.to_owned()).await {
                            log::warn!("Packet dropped {}", err);
                        }
                    }
                }
            } else if !net.contains(&dest) {
                log::debug!("Receive packet {} -> {} ({:?})", src, dest, protocol);
                raw_socket.send(packet.as_ref())?;
            }
        }
    }
    Ok(())
}

async fn send_daemon(
    config: AtewayNatConfig,
    write_tx: UnboundedSender<AtewayAdapterTask>,
    mut cap: Capture<Active>,
) -> Result<()> {
    let net = Ipv4Net::with_netmask(config.address, config.netmask)?;
    while let Ok(capture) = task::block_in_place(|| cap.next_packet()) {
        let eth = ether::Packet::new(capture.data)?;
        if eth.protocol() == ether::Protocol::Ipv4 {
            let packet = ip::v4::Packet::new(eth.payload().to_owned())?;
            let src = packet.source();
            let dest = packet.destination();
            let protocol = packet.protocol();

            if dest.is_broadcast() || net.contains(&dest) {
                log::debug!("Send packet {} -> {} ({:?})", src, dest, protocol);
                if let Err(err) = write_packet(&write_tx, packet.to_owned()).await {
                    log::warn!("Packet dropped {}", err);
                }
            }
        }
    }

    Ok(())
}
