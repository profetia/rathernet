use anyhow::Result;
use cpal::{traits::HostTrait, Device, Host};
use rodio::{OutputStream, OutputStreamHandle, StreamError};

pub struct AsioHost {
    pub inner: Host,
}

impl AsioHost {
    pub fn try_default() -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        Ok(Self { inner: host })
    }
}

pub struct AsioOutputStream {
    pub stream: OutputStream,
    pub handle: OutputStreamHandle,
}

impl AsioOutputStream {
    pub fn try_from_device(device: &Device) -> Result<Self> {
        let (stream, handle) = OutputStream::try_from_device(device)?;
        Ok(Self { stream, handle })
    }

    pub fn try_default() -> Result<Self> {
        let host = AsioHost::try_default()?;
        match host.inner.default_output_device() {
            Some(device) => AsioOutputStream::try_from_device(&device),
            None => Err(StreamError::NoDevice.into()),
        }
    }
}
