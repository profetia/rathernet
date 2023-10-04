use crate::{
    rather::{AtherInputStream, AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream},
};
use anyhow::Result;
use thiserror::Error;

#[derive(Clone)]
pub struct AcsmaIoConfig {
    pub address: usize,
    pub stream_config: AtherStreamConfig,
}

impl AcsmaIoConfig {
    pub fn new(address: usize, stream_config: AtherStreamConfig) -> Self {
        Self {
            address,
            stream_config,
        }
    }
}

#[derive(Debug, Error)]
pub enum AcsmaIoError {
    #[error("Link error after {0} retries")]
    LinkError(usize),
}

pub struct AcsmaIoSocket {
    config: AcsmaIoConfig,
    iather: AtherInputStream,
    oather: AtherOutputStream,
    monitor: AudioInputStream<f32>,
}

impl AcsmaIoSocket {
    pub fn try_from_device(config: AcsmaIoConfig, device: &AsioDevice) -> Result<Self> {
        let monitor = AudioInputStream::try_from_device_config(
            device,
            config.stream_config.stream_config.clone(),
        )?;
        let iather = AtherInputStream::new(
            config.stream_config.clone(),
            AudioInputStream::try_from_device_config(
                device,
                config.stream_config.stream_config.clone(),
            )?,
        );
        let oather = AtherOutputStream::new(
            config.stream_config.clone(),
            AudioOutputStream::try_from_device_config(
                device,
                config.stream_config.stream_config.clone(),
            )?,
        );
        Ok(Self {
            config,
            iather,
            oather,
            monitor,
        })
    }

    pub fn try_default(config: AcsmaIoConfig) -> Result<Self> {
        let device = AsioDevice::try_default()?;
        Self::try_from_device(config, &device)
    }
}
