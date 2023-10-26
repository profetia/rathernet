use anyhow::Result;
use clap::Parser;
use rathernet::raftp::{AftpEditor, AftpIoError};

#[derive(Parser, Debug)]
#[clap(name = "raftp", version = "0.1.0")]
#[clap(about = "A command line interface for rathernet raftp.")]
struct RaftpCli {
    /// The address of the FTP server to connect to, in the form of \<user\>@\<host\>\[:port\].
    #[arg(required = true)]
    url: String,
}

fn parse_url(url: &str) -> Result<(String, String, u16)> {
    let mut split = url.splitn(2, '@');
    let Some(user) = split.next() else {
        return Err(AftpIoError::InvalidUrl(url.into()).into());
    };
    let Some(rest) = split.next() else {
        return Err(AftpIoError::InvalidUrl(url.into()).into());
    };
    let mut split = rest.splitn(2, ':');
    let Some(host) = split.next() else {
        return Err(AftpIoError::InvalidUrl(url.into()).into());
    };
    let port = split.next().map(|s| s.parse::<u16>()).unwrap_or(Ok(21))?; // Default to port 21
    Ok((user.into(), host.into(), port))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = RaftpCli::parse();
    let (user, host, port) = parse_url(&cli.url)?;
    let mut editor = AftpEditor::try_new(&user, &host, port).await?;
    editor.repl().await?;
    Ok(())
}
