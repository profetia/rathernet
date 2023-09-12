use anyhow::Result;
use cpal::{traits::HostTrait, Device, Host};
use rodio::{OutputStream, OutputStreamHandle, StreamError};

pub struct ASIOHost(Host);

impl ASIOHost {
    pub fn try_default() -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        Ok(Self(host))
    }
}

impl From<ASIOHost> for Host {
    fn from(value: ASIOHost) -> Self {
        value.0
    }
}

pub struct ASIOOutputStream(OutputStream);
pub struct ASIOOutputStreamHandle(OutputStreamHandle);

impl ASIOOutputStream {
    pub fn try_from_device(device: &Device) -> Result<(Self, ASIOOutputStreamHandle)> {
        let (stream, handle) = OutputStream::try_from_device(device)?;
        Ok((ASIOOutputStream(stream), ASIOOutputStreamHandle(handle)))
    }

    pub fn try_default() -> Result<(Self, ASIOOutputStreamHandle)> {
        let host = ASIOHost::try_default()?;
        match host.0.default_output_device() {
            Some(device) => ASIOOutputStream::try_from_device(&device),
            None => Err(StreamError::NoDevice.into()),
        }
    }
}

impl From<ASIOOutputStream> for OutputStream {
    fn from(value: ASIOOutputStream) -> Self {
        value.0
    }
}

impl From<ASIOOutputStreamHandle> for OutputStreamHandle {
    fn from(value: ASIOOutputStreamHandle) -> Self {
        value.0
    }
}
