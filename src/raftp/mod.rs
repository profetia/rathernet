mod editor;

pub use editor::AftpEditor;

pub use async_ftp::FtpError;
pub use async_ftp::FtpStream;

use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AftpIoError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
    #[error("Invalid arguments")]
    InvalidArguments,
    #[error("{0}")]
    FtpError(#[from] FtpError),
    #[error("{0}")]
    IoError(#[from] io::Error),
}
