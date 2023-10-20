use anyhow::Result;
use async_ftp::FtpStream;
use rustyline::{history::FileHistory, Completer, Editor, Helper, Highlighter, Hinter, Validator};

pub struct AftpEditor {
    user: String,
    host: String,
    stream: FtpStream,
    editor: Editor<AftpEditorHelper, FileHistory>,
}

impl AftpEditor {
    pub async fn try_new(user: &str, host: &str) -> Result<Self> {
        let mut editor = Editor::new()?;
        editor.set_helper(Some(AftpEditorHelper {}));
        let stream = FtpStream::connect(host).await?;

        Ok(Self {
            user: user.into(),
            host: host.into(),
            stream,
            editor,
        })
    }

    pub async fn login(&mut self) -> Result<()> {
        let prompt = format!("{}@{}'s password: ", self.user, self.host);
        let password = self.editor.readline(&prompt)?;
        self.stream.login(&self.user, &password).await?;
        Ok(())
    }
}

#[derive(Completer, Helper, Hinter, Validator, Highlighter)]
struct AftpEditorHelper {}
