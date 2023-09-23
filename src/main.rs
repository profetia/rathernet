use anyhow::Result;
use bitvec::prelude::*;
use cpal::FromSample;
use cpal::{traits::DeviceTrait, SupportedStreamConfig};
// use rathernet::rather::signal::BandPass;
use rathernet::rather::{self, AtherInputStream};
use rathernet::raudio::AudioTrack;
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
    let mut read_ather = AtherInputStream::new(config.clone(), read_stream);

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

    tokio::join!(
        async {
            write_ather.write(&bits).await;
            eprintln!("Transmitted: {}", bits.len());
        },
        async {
            let mut buf = bitvec![];
            let mut received = 0;
            let mut id = 0;
            while let Some(bits) = read_ather.next().await {
                // if let Some(bits) = read_ather.next().await {
                println!("packet ID: {} - {}", id, bits);
                received += bits.len();
                eprintln!("ID: {} - New: {} - Total: {}", id, bits.len(), received);
                buf.extend(bits);
                id += 1;
                if received >= 10000 {
                    break;
                }
            }
            fs::write(
                "output.txt",
                buf.into_iter()
                    .map(|bit| if bit { '1' } else { '0' })
                    .collect::<String>(),
            )
            .unwrap();
        }
    );

    // let mut buf = vec![];
    // let binding = Decoder::new(BufReader::new(File::open("test.wav")?))?
    //     .map(f32::from_sample_)
    //     .collect::<Vec<f32>>();
    // let mut stream = binding.chunks(384);
    // let mut bits = bitvec![];
    // let mut id = 0;
    // while let Some(data) = rather::decode_from_buf(&config, &mut stream, &mut buf).await {
    //     let len = data.len();
    //     eprintln!("[{}] Received: {}", id, len);
    //     bits.extend(data);
    //     id += 1;
    //     if len != 127 {
    //         break;
    //     }
    // }

    // Create a new file and write the bits to it as a string
    // let mut file = File::create("output.txt")?;
    // let content = bits
    //     .iter()
    //     .map(|bit| if *bit { '1' } else { '0' })
    //     .collect::<String>();
    // file.write_all(content.as_bytes())?;

    // let mut buf = Decoder::new(BufReader::new(File::open("rpal.wav")?))?
    //     .map(f32::from_sample_)
    //     .collect::<Vec<f32>>();
    // // buf.band_pass(48000., (9000., 11000.));
    // // let track = AudioTrack::new(config.stream_config, buf.into());
    // fs::write("rpal.json", format!("{:?}", buf)).unwrap();

    Ok(())
}
