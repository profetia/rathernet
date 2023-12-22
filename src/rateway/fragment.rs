use crate::racsma::builtin::SOCKET_BYTES_MTU;
use anyhow::Result;
use packet::{ip::v4::Packet as Ipv4Packet, Packet};
use pnet_packet::ipv4;
use std::collections::HashMap;

pub fn split_packet(source: Ipv4Packet<Vec<u8>>) -> Result<Vec<Vec<u8>>> {
    // Assume that the packet is not fragmented and DONT_FRAGMENT flag is not set.
    let mtu = SOCKET_BYTES_MTU - ((source.header() as usize) << 2);
    if source.payload().len() <= mtu {
        return Ok(vec![source.as_ref().to_owned()]);
    }

    let mtu = mtu / 8 * 8;
    let chunks = source.payload().chunks(mtu);
    let mut fragments = vec![vec![]; chunks.len()];
    let mut offset: u16 = 0;
    for (i, chunk) in chunks.enumerate() {
        let mut buf = source.split().0.to_vec();
        let total_len = buf.len() + chunk.len();
        buf[2..=3].copy_from_slice(&(total_len as u16).to_be_bytes());
        if i != fragments.len() - 1 {
            buf[6..=7].copy_from_slice(
                &(offset | (ipv4::Ipv4Flags::MoreFragments as u16) << 13).to_be_bytes(),
            );
        } else {
            buf[6..=7].copy_from_slice(&offset.to_be_bytes());
        }

        buf.extend_from_slice(chunk);
        let mut packet = Ipv4Packet::new(buf)?;
        packet.update_checksum()?;

        fragments[i] = packet.as_ref().to_owned();
        offset += (chunk.len() / 8) as u16;
    }

    log::debug!(
        "Packet of {} bytes is fragmented into {} fragments",
        source.payload().len(),
        fragments.len()
    );

    Ok(fragments)
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
        if !has_more_fragment(&packet) && is_first_fragment(&packet) {
            log::debug!("Packet {} is not fragmented", packet.id());
            return Ok(Some(packet));
        }

        let id = packet.id();
        let entry = self.jar.entry(packet.id()).or_default();
        entry.push(packet);
        entry.sort_by_key(|item| item.patched_offset());

        if !is_assembliable(entry) {
            log::debug!("Packet {} is not assembliable", id);
            return Ok(None);
        }

        log::debug!("Packet {} is fully received and can be assembled", id);
        let fragments = self.jar.remove(&id).unwrap();
        let payload = fragments
            .iter()
            .flat_map(|packet| packet.payload().to_owned())
            .collect::<Vec<_>>();

        let mut buf = fragments[0].split().0.to_vec();
        let total_len = buf.len() + payload.len();
        buf[2..=3].copy_from_slice(&(total_len as u16).to_be_bytes());
        buf[6..=7].copy_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&payload);

        let mut packet = Ipv4Packet::new(buf)?;
        packet.update_checksum()?;

        Ok(Some(packet))
    }
}

pub fn has_more_fragment(packet: &Ipv4Packet<Vec<u8>>) -> bool {
    (packet.patched_flags() & (ipv4::Ipv4Flags::MoreFragments as u16)) != 0
}

pub fn is_first_fragment(packet: &Ipv4Packet<Vec<u8>>) -> bool {
    packet.patched_offset() == 0
}

fn is_assembliable(entry: &[Ipv4Packet<Vec<u8>>]) -> bool {
    if has_more_fragment(entry.last().unwrap()) {
        return false;
    }
    let mut offset = 0;
    for packet in entry.iter() {
        if packet.patched_offset() != offset {
            return false;
        }
        offset += (packet.payload().len() / 8) as u16;
    }
    true
}

trait Patch {
    fn patched_flags(&self) -> u16;
    fn patched_offset(&self) -> u16;
}

impl<T> Patch for Ipv4Packet<T>
where
    T: AsRef<[u8]>,
{
    fn patched_flags(&self) -> u16 {
        let flags = u16::from_be_bytes(self.as_ref()[6..=7].try_into().unwrap());
        flags >> 13
    }

    fn patched_offset(&self) -> u16 {
        let offset = u16::from_be_bytes(self.as_ref()[6..=7].try_into().unwrap());
        offset & 0b0001_1111_1111_1111
    }
}
