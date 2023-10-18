use super::{builtin::SOCKET_ARP_TIMEOUT, nat::find_device, AtewayIoError};
use anyhow::Result;
use packet::{
    ether::{self, Protocol},
    Builder,
};
use pcap::{Active, Capture, Error};
use pnet_base::MacAddr;
use pnet_packet::{
    arp::{ArpHardwareTypes, ArpOperations, ArpPacket, MutableArpPacket},
    ethernet::{EtherTypes, EthernetPacket},
    Packet as PnetPacket,
};
use std::{
    net::{IpAddr, Ipv4Addr},
    time::Instant,
};

pub struct AtewayIoSocket {
    cap: Capture<Active>,
    mac: (MacAddr, MacAddr),
}

impl AtewayIoSocket {
    pub fn try_new(host: Ipv4Addr) -> Result<Self> {
        let (gateway, mac) = find_adapter(host)?;
        let device = find_device(host)?;
        let mut cap = Capture::from_device(device)?
            .immediate_mode(true)
            .open()?
            .setnonblock()?;
        cap.filter(
            &format!("arp src host {} && arp dst host {}", gateway, host),
            true,
        )?;

        let arp = create_arp(host, mac, gateway);
        let ether = create_ether(mac, MacAddr::broadcast(), Protocol::Arp, &arp)?;
        cap.sendpacket(ether)?;

        let start = Instant::now();
        let gateway_mac = loop {
            if start.elapsed() > SOCKET_ARP_TIMEOUT {
                return Err(
                    AtewayIoError::ArpTimeout(start.elapsed().as_millis().try_into()?).into(),
                );
            }
            match cap.next_packet() {
                Ok(packet) => {
                    let ether = EthernetPacket::new(packet.data).unwrap();
                    if let Some(arp) = ArpPacket::new(ether.payload()) {
                        log::debug!("Got gateway MAC: {}", arp.get_sender_hw_addr());
                        break arp.get_sender_hw_addr();
                    }
                }
                Err(Error::TimeoutExpired) => {}
                Err(err) => return Err(err.into()),
            }
        };

        Ok(Self {
            cap,
            mac: (mac, gateway_mac),
        })
    }

    pub fn send(&mut self, payload: &[u8]) -> Result<()> {
        let ether = create_ether(self.mac.0, self.mac.1, Protocol::Ipv4, payload)?;
        self.cap.sendpacket(ether)?;
        Ok(())
    }
}

pub(super) fn find_adapter(host: Ipv4Addr) -> Result<(Ipv4Addr, MacAddr)> {
    let adapers = ipconfig::get_adapters()?;
    adapers
        .iter()
        .filter(|item| item.ip_addresses().contains(&IpAddr::V4(host)))
        .find_map(|item| {
            for gateway in item.gateways() {
                if let Some(mac) = item.physical_address() {
                    if let IpAddr::V4(gateway) = gateway {
                        let mac: [u8; 6] = mac.try_into().unwrap();
                        return Some((*gateway, MacAddr::from(mac)));
                    }
                }
            }
            None
        })
        .ok_or(AtewayIoError::DeviceNotFound(host).into())
}

fn create_arp(host: Ipv4Addr, mac: MacAddr, gateway: Ipv4Addr) -> Vec<u8> {
    let mut arp = MutableArpPacket::owned(vec![0; 28]).unwrap();
    arp.set_hardware_type(ArpHardwareTypes::Ethernet);
    arp.set_protocol_type(EtherTypes::Ipv4);
    arp.set_hw_addr_len(6);
    arp.set_proto_addr_len(4);
    arp.set_operation(ArpOperations::Request);
    arp.set_sender_hw_addr(mac);
    arp.set_sender_proto_addr(host);
    arp.set_target_hw_addr(MacAddr::zero());
    arp.set_target_proto_addr(gateway);
    arp.packet().to_vec()
}

fn create_ether(
    src: MacAddr,
    dest: MacAddr,
    protocol: Protocol,
    payload: &[u8],
) -> Result<Vec<u8>> {
    let mut ether = ether::Builder::default()
        .source(src.octets().into())?
        .destination(dest.octets().into())?
        .protocol(protocol)?
        .payload(payload)?
        .build()?;

    // IEEE 802.3 Ethernet frame minimum size is 64 bytes
    // Here the frame is padded to 60 bytes, the rest is left to the network card
    if ether.len() < 60 {
        ether.resize(60, 0);
    }

    Ok(ether)
}
