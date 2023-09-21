use anyhow::Result;
use bitvec::prelude::*;
use cpal::FromSample;
use cpal::{traits::DeviceTrait, SupportedStreamConfig};
use rathernet::rather::{self, AtherInputStream};
use rathernet::{
    rather::{AtherOutputStream, AtherStreamConfig},
    raudio::{AsioDevice, AudioInputStream, AudioOutputStream},
};
use rodio::Decoder;
use std::io::Write;
use std::sync::Arc;
use std::{
    fs::{self, File},
    io::BufReader,
};
use tokio::sync;
use tokio_stream::StreamExt;

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

    let write_stream = AudioOutputStream::try_from_device_config(&device, config.clone())?;
    let read_stream = AudioInputStream::try_from_device_config(&device, config.clone())?;

    let config = AtherStreamConfig::new(10000, 1000, config);
    let write_ather = AtherOutputStream::new(config.clone(), write_stream);
    let mut read_ather = AtherInputStream::new(config, read_stream);

    // let file = fs::read_to_string("input.txt")?;
    let file = fs::read_to_string("assets/ather/INPUT.txt")?;
    let mut bits = bitvec![];
    for ch in file.chars() {
        match ch {
            '0' => bits.push(false),
            '1' => bits.push(true),
            _ => {}
        }
    }

    tokio::join!(write_ather.write(&bits), async {
        let mut received = 0;
        while let Some(bits) = read_ather.next().await {
            // if let Some(bits) = read_ather.next().await {
            println!("{}", bits);
            received += bits.len();
            eprintln!("Received: {}", received);
        }
    });

    // let mut buf = Decoder::new(BufReader::new(File::open("test.wav")?))?
    //     .map(f32::from_sample_)
    //     .collect::<Vec<f32>>();

    // let read_stream = Arc::new(sync::Mutex::new(read_stream));
    // let bits = rather::decode_packet(&config, &read_stream, &mut buf)
    //     .await
    //     .unwrap();

    // // Create a new file and write the bits to it as a string
    // let mut file = File::create("output.txt")?;
    // let content = bits
    //     .iter()
    //     .map(|bit| if *bit { '1' } else { '0' })
    //     .collect::<String>();
    // file.write_all(content.as_bytes())?;

    Ok(())
}
