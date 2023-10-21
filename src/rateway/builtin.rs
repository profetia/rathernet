use std::{
    net::{Ipv4Addr, SocketAddrV4},
    ops::Range,
    time::Duration,
};

pub const NAT_PORT_RANGE: Range<u16> = 10000..14999;
pub const NAT_SENDTO_PLACEHOLDER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);

pub const SOCKET_ARP_TIMEOUT: Duration = Duration::from_millis(1000);

pub const TCP_BUFFER_LEN: u16 = 1024;
