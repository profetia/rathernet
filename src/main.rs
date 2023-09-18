use anyhow::Result;
use cpal::{traits::DeviceTrait, SupportedStreamConfig};
use rathernet::{rather::AtherStreamConfig, raudio::AsioDevice};

#[tokio::main]
async fn main() -> Result<()> {
    let device = AsioDevice::try_default()?;

    let default_config = device.0.default_output_config()?;
    let stream_config = SupportedStreamConfig::new(
        1,
        cpal::SampleRate(48000),
        default_config.buffer_size().clone(),
        default_config.sample_format(),
    );

    let config = AtherStreamConfig::new(stream_config);

    println!("{:?}", config.header);

    Ok(())
}
