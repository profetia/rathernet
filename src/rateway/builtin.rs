use std::{
    net::{Ipv4Addr, SocketAddrV4},
    ops::Range,
};

pub const NAT_PORT_RANGE: Range<u16> = 10000..14999;
pub const NAT_SENDTO_PLACEHOLDER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
