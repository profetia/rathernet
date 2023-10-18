mod adapter;
mod nat;

pub mod builtin;
pub mod utils;

pub use adapter::{AtewayAdapterConfig, AtewayIoAdaper};
pub use nat::{AtewayIoNat, AtewayNatConfig};

use std::net::Ipv4Addr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AtewayIoError {
    #[error("Device not found for {0}")]
    DeviceNotFound(Ipv4Addr),
}
