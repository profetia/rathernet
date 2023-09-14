use anyhow::Result;
use cpal::{traits::HostTrait, Device, Host};
use rodio::{DeviceTrait, OutputStream, OutputStreamHandle, StreamError};

pub struct AsioHost {
    pub inner: Host,
}

impl AsioHost {
    pub fn try_new() -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        Ok(Self { inner: host })
    }
}

pub struct AsioDevice {
    pub inner: Device,
}

impl AsioDevice {
    pub fn try_default() -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host.inner.default_input_device() {
            Some(device) => Ok(Self { inner: device }),
            None => Err(StreamError::NoDevice.into()),
        }
    }

    pub fn try_from_name(name: &str) -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host
            .inner
            .devices()?
            .find(|d| d.name().map(|s| s == name).unwrap_or(false))
        {
            Some(device) => Ok(Self { inner: device }),
            None => Err(StreamError::NoDevice.into()),
        }
    }
}

pub struct AsioOutputStream {
    pub stream: OutputStream,
    pub handle: OutputStreamHandle,
}

impl AsioOutputStream {
    pub fn try_from_device(device: &AsioDevice) -> Result<Self> {
        let (stream, handle) = OutputStream::try_from_device(&device.inner)?;
        Ok(Self { stream, handle })
    }

    pub fn try_from_name(name: &str) -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host
            .inner
            .devices()?
            .find(|d| d.name().map(|s| s == name).unwrap_or(false))
        {
            Some(ref device) => {
                let (stream, handle) = OutputStream::try_from_device(device)?;
                Ok(Self { stream, handle })
            }
            None => Err(StreamError::NoDevice.into()),
        }
    }

    pub fn try_default() -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host.inner.default_output_device() {
            Some(ref device) => {
                let (stream, handle) = OutputStream::try_from_device(device)?;
                Ok(Self { stream, handle })
            }
            None => Err(StreamError::NoDevice.into()),
        }
    }
}
