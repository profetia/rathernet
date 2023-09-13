use std::{fs::File, io::BufReader, time::Duration};

use anyhow::Result;

use rathernet::raudio::{track::Track, AudioOutputStream};
use rodio::Decoder;

#[tokio::main]
async fn main() -> Result<()> {
    let stream = AudioOutputStream::try_default()?;
    let file = BufReader::new(File::open("test.wav")?);
    let source = Decoder::new(file)?;

    let track = Track::from_source(source.into_iter());

    stream
        .write_timeout(track.into_iter(), Duration::from_secs(5))
        .await;

    Ok(())
}
