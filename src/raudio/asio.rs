use anyhow::Result;
use cpal::{traits::HostTrait, Device, Host, Stream, SupportedStreamConfig};
use rodio::{DeviceTrait, OutputStream, OutputStreamHandle, StreamError};
use std::sync::Arc;

pub struct AsioHost(pub Host);

impl AsioHost {
    pub fn try_new() -> Result<Self> {
        let host = cpal::host_from_id(cpal::HostId::Asio)?;
        Ok(Self(host))
    }
}

#[derive(Clone)]
pub struct AsioDevice(pub Arc<Device>);

impl AsioDevice {
    pub fn try_default() -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host.0.default_input_device() {
            Some(device) => Ok(Self(device.into())),
            None => Err(StreamError::NoDevice.into()),
        }
    }

    pub fn try_from_name(name: &str) -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host
            .0
            .devices()?
            .find(|d| d.name().map(|s| s == name).unwrap_or(false))
        {
            Some(device) => Ok(Self(device.into())),
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
        let (stream, handle) = OutputStream::try_from_device(&device.0)?;
        Ok(Self { stream, handle })
    }

    pub fn try_from_device_config(
        device: &AsioDevice,
        config: SupportedStreamConfig,
    ) -> Result<Self> {
        let (stream, handle) = OutputStream::try_from_device_config(&device.0, config)?;
        Ok(Self { stream, handle })
    }

    pub fn try_default() -> Result<Self> {
        let host = AsioHost::try_new()?;
        match host.0.default_output_device() {
            Some(ref device) => {
                let (stream, handle) = OutputStream::try_from_device(device)?;
                Ok(Self { stream, handle })
            }
            None => Err(StreamError::NoDevice.into()),
        }
    }
}

unsafe impl Send for AsioOutputStream {}

unsafe impl Sync for AsioOutputStream {}

pub struct AsioInputStream(pub Stream);

impl AsioInputStream {
    pub fn new(stream: Stream) -> Self {
        Self(stream)
    }
}

unsafe impl Send for AsioInputStream {}

unsafe impl Sync for AsioInputStream {}
