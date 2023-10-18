mod adapter;
mod nat;
mod socket;

pub mod builtin;
pub mod utils;

pub use adapter::{AtewayAdapterConfig, AtewayIoAdaper};
pub use nat::{AtewayIoNat, AtewayNatConfig};
pub use socket::AtewayIoSocket;

use std::net::Ipv4Addr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AtewayIoError {
    #[error("Device not found for {0}")]
    DeviceNotFound(Ipv4Addr),
    #[error("ARP timeout after {0}ms")]
    ArpTimeout(u64),
}
