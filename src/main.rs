use hound::SampleFormat;
use rathernet::raudio::{AsioDevice, AudioInputStream, AudioOutputStream};
use rathernet::raudio::{IntoSpec, Track};
use rodio::Decoder;

use std::fs::File;
use std::io::BufReader;

use anyhow::Result;
use std::thread;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    let device = AsioDevice::try_default()?;

    let write_stream = AudioOutputStream::try_from_device(&device)?;
    let file = BufReader::new(File::open("assets/audio/A_Horse_With_No_Name.wav")?);
    let source = Decoder::new(file)?;

    let write_future = write_stream.write_timeout(source, Duration::from_secs(10));

    let mut read_stream = AudioInputStream::<f32>::try_from_device(&device)?;

    let mut spec = read_stream.config().clone().into_spec();
    spec.sample_format = SampleFormat::Float;

    let read_future = read_stream.read_timeout(Duration::from_secs(10));

    println!("Read and write are ready.");
    thread::sleep(Duration::from_secs(5));
    println!("Start read and write.");
    let (_, data) = tokio::join!(write_future, read_future);

    println!("Read and write are done.");
    thread::sleep(Duration::from_secs(5));

    println!("Start playback");
    let track = Track::from_vec(spec, data);
    let stream = AudioOutputStream::try_default()?;
    stream.write(track.into_iter()).await;
    Ok(())
}
