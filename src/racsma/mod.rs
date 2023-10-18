mod frame;
mod socket;
mod stream;

pub mod builtin;

pub use socket::{AcsmaIoSocket, AcsmaSocketConfig, AcsmaSocketReader, AcsmaSocketWriter};
pub use stream::{AcsmaIoStream, AcsmaStreamConfig};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcsmaIoError {
    #[error("Link error after {0} retries")]
    LinkError(usize),
    #[error("Perf timeout after {0} ms")]
    PerfTimeout(usize),
}
