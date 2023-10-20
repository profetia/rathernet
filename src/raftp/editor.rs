use anyhow::Result;
use async_ftp::FtpStream;
use rustyline::{history::FileHistory, Completer, Editor, Helper, Highlighter, Hinter, Validator};

use super::AftpIoError;

pub struct AftpEditor {
    user: String,
    host: String,
    workspace: String,
    stream: FtpStream,
    editor: Editor<AftpEditorHelper, FileHistory>,
}

impl AftpEditor {
    pub async fn try_new(user: &str, host: &str, port: u16) -> Result<Self> {
        let mut editor = Editor::new()?;
        editor.set_helper(Some(AftpEditorHelper {}));
        let url = format!("{}:{}", host, port);
        let mut stream = FtpStream::connect(url).await?;

        let prompt = format!("{}@{}'s password: ", user, host);
        let password = editor.readline(&prompt)?;
        stream.login(user, &password).await?;
        if let Some(welcome) = stream.get_welcome_msg() {
            println!("{}", welcome);
        }
        let workspace = stream.pwd().await?;

        Ok(Self {
            user: user.into(),
            host: host.into(),
            workspace,
            stream,
            editor,
        })
    }

    pub async fn repl(&mut self) -> Result<()> {
        loop {
            let line = self
                .editor
                .readline(&format!("{}@{} {}# ", self.user, self.host, self.workspace))?;
            match self.execute(&line).await {
                Ok(_) => {}
                Err(AftpIoError::FtpError(err)) => return Err(err.into()),
                Err(err) => println!("{}", err),
            };
        }
    }
}

impl AftpEditor {
    async fn execute(&mut self, line: &str) -> Result<(), AftpIoError> {
        let mut args = line.split_whitespace();
        let Some(command) = args.next() else {
            return Err(AftpIoError::InvalidCommand(line.into()));
        };

        match command {
            "ls" => command_ls(self, args.collect::<Vec<_>>().as_slice()).await,
            _ => Err(AftpIoError::InvalidCommand(command.into())),
        }
    }
}

async fn command_ls(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    let result = editor.stream.list(args.first().copied()).await;
    match result {
        Ok(list) => {
            for item in list {
                println!("{}", item);
            }
        }
        Err(err) => return Err(AftpIoError::FtpError(err)),
    }
    Ok(())
}

#[derive(Completer, Helper, Hinter, Validator, Highlighter)]
struct AftpEditorHelper {}
