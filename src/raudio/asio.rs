use anyhow::Result;
use cpal::{traits::HostTrait, Device, Host};
use rodio::{OutputStream, OutputStreamHandle, StreamError};

pub struct AsioHost(Host);

impl AsioHost {
    pub fn try_default() -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        Ok(Self(host))
    }
}

impl From<AsioHost> for Host {
    fn from(value: AsioHost) -> Self {
        value.0
    }
}

pub struct AsioOutputStream(OutputStream);
pub struct AsioOutputStreamHandle(OutputStreamHandle);

impl AsioOutputStream {
    pub fn try_from_device(device: &Device) -> Result<(Self, AsioOutputStreamHandle)> {
        let (stream, handle) = OutputStream::try_from_device(device)?;
        Ok((AsioOutputStream(stream), AsioOutputStreamHandle(handle)))
    }

    pub fn try_default() -> Result<(Self, AsioOutputStreamHandle)> {
        let host = AsioHost::try_default()?;
        match host.0.default_output_device() {
            Some(device) => AsioOutputStream::try_from_device(&device),
            None => Err(StreamError::NoDevice.into()),
        }
    }
}

impl From<AsioOutputStream> for OutputStream {
    fn from(value: AsioOutputStream) -> Self {
        value.0
    }
}

impl From<AsioOutputStreamHandle> for OutputStreamHandle {
    fn from(value: AsioOutputStreamHandle) -> Self {
        value.0
    }
}
