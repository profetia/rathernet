use crate::racsma::AcsmaSocketConfig;
use std::net::Ipv4Addr;

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

pub struct AtewayIoNat {}
