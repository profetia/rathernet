use std::{fs::File, io::BufReader, thread, time::Duration};

use anyhow::Result;

use rathernet::raudio::{track::Track, AudioOutputStream};
use rodio::Decoder;

#[tokio::main]
async fn main() -> Result<()> {
    let stream = AudioOutputStream::try_default()?;
    let file = BufReader::new(File::open("assets/audio/A_Horse_With_No_Name.wav")?);
    let source = Decoder::new(file)?;

    let track = Track::from_source(source.into_iter());

    let future = stream.write_timeout(track.into_iter(), Duration::from_secs(20));

    println!("Waiting for audio to not be playing...");
    thread::sleep(Duration::from_secs(5));

    println!("Audio should be playing now.");

    future.await;

    println!("Audio should be done playing now.");

    thread::sleep(Duration::from_secs(5));

    println!("Exiting...");

    Ok(())
}
