use crate::racsma::builtin::SOCKET_BYTES_MTU;
use anyhow::Result;
use packet::{
    ip::{self, v4::Packet as Ipv4Packet},
    Builder, Packet,
};
use std::collections::HashMap;

pub fn split_packet(packet: Ipv4Packet<Vec<u8>>) -> Result<Vec<Vec<u8>>> {
    // Assume that the packet is not fragmented and DONT_FRAGMENT flag is not set.
    let mtu = SOCKET_BYTES_MTU - ((packet.header() as usize) << 2);
    if packet.payload().len() <= mtu {
        return Ok(vec![packet.as_ref().to_owned()]);
    }
    let mtu = mtu / 8 * 8;
    let chunks = packet.payload().chunks(mtu);
    let mut packets = Vec::new();
    let chunk_len = chunks.len();
    let mut offset = 0;
    for (i, chunk) in chunks.enumerate() {
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
        if i != chunk_len - 1 {
            builder = builder.flags(ip::v4::Flags::MORE_FRAGMENTS)?;
        }

        packets.push(builder.build()?);
        offset += (chunk.len() / 8) as u16;
    }

    Ok(packets)
}

pub struct Assembler {
    jar: HashMap<u16, Vec<Ipv4Packet<Vec<u8>>>>,
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            jar: HashMap::new(),
        }
    }

    pub fn assemble(&mut self, packet: Ipv4Packet<Vec<u8>>) -> Result<Option<Ipv4Packet<Vec<u8>>>> {
        if !packet.flags().contains(ip::v4::Flags::MORE_FRAGMENTS) && packet.offset() == 0 {
            return Ok(Some(packet));
        }

        let id = packet.id();
        let entry = self.jar.entry(packet.id()).or_default();
        entry.push(packet);
        entry.sort_by_key(|packet| packet.offset());

        let mut offset = 0;
        for packet in entry.iter() {
            if packet.offset() != offset {
                return Ok(None);
            }
            offset += (packet.payload().len() / 8) as u16;
        }

        let packets = self.jar.remove(&id).unwrap();
        let bytes = ip::v4::Builder::default()
            .dscp(packets[0].dscp())?
            .ecn(packets[0].ecn())?
            .id(packets[0].id())?
            .ttl(packets[0].ttl())?
            .protocol(packets[0].protocol())?
            .source(packets[0].source())?
            .destination(packets[0].destination())?
            .payload(
                packets
                    .into_iter()
                    .flat_map(|packet| packet.payload().to_owned())
                    .collect::<Vec<_>>()
                    .iter(),
            )?
            .build()?;

        match ip::Packet::new(bytes) {
            Ok(ip::Packet::V4(packet)) => Ok(Some(packet)),
            _ => unreachable!(),
        }
    }
}
