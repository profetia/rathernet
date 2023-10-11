use anyhow::Result;
use bitvec::prelude::*;
use cpal::{traits::DeviceTrait, SupportedStreamConfig};
use rathernet::rather::builtin::PAYLOAD_BITS_LEN;
use rathernet::rather::AtherInputStream;
use rathernet::rather::signal::Energy;
use rathernet::{
    rather::{AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream},
};
use std::fs::File;
use std::io;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let device = AsioDevice::try_default()?;

    let default_config = device.0.default_output_config()?;
    let config = SupportedStreamConfig::new(
        1,
        cpal::SampleRate(48000),
        default_config.buffer_size().clone(),
        default_config.sample_format(),
    );

    let read_stream = AudioInputStream::try_from_device_config(&device, config.clone())?;
    let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;

    let mut monitor_stream =
        AudioInputStream::<f32>::try_from_device_config(&device, config.clone())?;

    let config = AtherStreamConfig::new(15000, config.clone());

    let mut read_ather = AtherInputStream::new(config.clone(), read_stream);
    let write_ather = AtherOutputStream::new(config.clone(), write_stream);

    let mut bits = bitvec![];
    let mut file = File::open("./INPUT.bin")?;
    io::copy(&mut file, &mut bits)?;

    let write_future = async {
        for chunk in bits.chunks(PAYLOAD_BITS_LEN) {
            write_ather.write(chunk).await.unwrap();
        }
    };
    let read_future = async {
        let mut buf = bitvec![0; bits.len()];
        read(&mut read_ather, &mut buf).await;
        buf
    };
    let monitor_future = async { 
        while let Some(sample) = monitor_stream.next().await {
            let energy = sample.energy(48000);
            if energy > 1e-4 {
                eprintln!("Monitor: {}", energy);
            }
        }
     };

    let (_, mut read_buf, _) = tokio::join!(write_future, read_future, monitor_future);

    let file = File::create("output.bin")?;
    io::copy(&mut read_buf, &mut &file)?;

    Ok(())
}

async fn read(ather: &mut AtherInputStream, buf: &mut BitSlice) {
    let mut bits = bitvec![];
    while let Some(frame) = ather.next().await {
        let len = frame.len();
        bits.extend(frame);
        eprintln!("Received: {}, new: {}", bits.len(), len);
        if bits.len() >= buf.len() {
            break;
        }
    }
    buf.copy_from_bitslice(&bits);
}
