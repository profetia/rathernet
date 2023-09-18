use anyhow::Result;
use cpal::{traits::DeviceTrait, SupportedStreamConfig};
use rathernet::{
    rather::{AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioOutputStream},
};

#[tokio::main]
async fn main() -> Result<()> {
    let device = AsioDevice::try_default()?;

    let default_config = device.0.default_output_config()?;
    let config = SupportedStreamConfig::new(
        1,
        cpal::SampleRate(48000),
        default_config.buffer_size().clone(),
        default_config.sample_format(),
    );
    let stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
    let ather = AtherOutputStream::new(AtherStreamConfig::new(10000, 1000, config), stream);

    Ok(())
}
