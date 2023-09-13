use std::{fs::File, io::BufReader};

use anyhow::Result;

use rathernet::raudio::stream::AudioOutputStream;
use rodio::Decoder;

#[tokio::main]
async fn main() -> Result<()> {
    let stream = AudioOutputStream::try_default()?;
    let file = BufReader::new(File::open("test.wav")?);
    let source = Decoder::new(file)?;
    stream.write(source).await?;

    Ok(())
}
