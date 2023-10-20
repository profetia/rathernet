use anyhow::Result;
use async_ftp::FtpStream;
use rustyline::{
    highlight::Highlighter,
    hint::{Hint, Hinter},
    history::MemHistory,
    Completer, Config, Editor, Helper, Validator,
};
use std::{borrow::Cow, collections::HashSet, process};
use tokio::{fs::File, io::AsyncWriteExt};

use super::AftpIoError;

pub struct AftpEditor {
    user: String,
    host: String,
    workspace: String,
    stream: FtpStream,
    editor: Editor<AftpEditorHelper, MemHistory>,
}

impl AftpEditor {
    pub async fn try_new(user: &str, host: &str, port: u16) -> Result<Self> {
        let config = Config::builder().auto_add_history(true).build();
        let history = MemHistory::with_config(config);
        let mut editor = Editor::with_history(config, history)?;
        let mut hints = HashSet::new();
        hints.insert(AftpEditorHint::new("help".into()));
        hints.insert(AftpEditorHint::new("ls".into()));
        hints.insert(AftpEditorHint::new("pwd".into()));
        hints.insert(AftpEditorHint::new("cd".into()));
        hints.insert(AftpEditorHint::new("exit".into()));
        hints.insert(AftpEditorHint::new("get".into()));
        hints.insert(AftpEditorHint::new("put".into()));
        editor.set_helper(Some(AftpEditorHelper {
            masking: true,
            hints,
        }));
        let url = format!("{}:{}", host, port);
        let mut stream = FtpStream::connect(url).await?;

        let prompt = format!("{}@{}'s password: ", user, host);
        let password = editor.readline(&prompt)?;
        stream.login(user, &password).await?;
        if let Some(welcome) = stream.get_welcome_msg() {
            println!("{}", welcome);
        }
        let workspace = stream.pwd().await?;
        editor.helper_mut().unwrap().masking = false;

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
            "help" => help(self, args.collect::<Vec<_>>().as_slice()).await,
            "ls" => ls(self, args.collect::<Vec<_>>().as_slice()).await,
            "pwd" => pwd(self, args.collect::<Vec<_>>().as_slice()).await,
            "cd" => cd(self, args.collect::<Vec<_>>().as_slice()).await,
            "exit" => exit(self, args.collect::<Vec<_>>().as_slice()).await,
            "get" => get(self, args.collect::<Vec<_>>().as_slice()).await,
            "put" => put(self, args.collect::<Vec<_>>().as_slice()).await,
            _ => Err(AftpIoError::InvalidCommand(command.into())),
        }
    }
}

async fn help(_: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if !args.is_empty() {
        return Err(AftpIoError::InvalidArguments);
    }
    println!(
        r#"
help - print this help message
ls [path] - list files in path
pwd - print working directory
cd <path> - change working directory to path
exit - exit the program
get <remote> [local] - download remote file to local file
put <local> [remote] - upload local file to remote file
    "#
    );
    Ok(())
}

async fn ls(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if args.len() > 1 {
        return Err(AftpIoError::InvalidArguments);
    }
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

async fn pwd(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if !args.is_empty() {
        return Err(AftpIoError::InvalidArguments);
    }
    let result = editor.stream.pwd().await;
    match result {
        Ok(path) => println!("{}", path),
        Err(err) => return Err(AftpIoError::FtpError(err)),
    }
    Ok(())
}

async fn cd(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if args.len() != 1 {
        return Err(AftpIoError::InvalidArguments);
    }
    editor.stream.cwd(args.first().unwrap()).await?;
    editor.workspace = editor.stream.pwd().await?;
    Ok(())
}

async fn exit(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if !args.is_empty() {
        return Err(AftpIoError::InvalidArguments);
    }
    editor.stream.quit().await?;
    process::exit(0);
}

async fn get(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if !(1..=2).contains(&args.len()) {
        return Err(AftpIoError::InvalidArguments);
    }
    let &remote = args.first().unwrap();
    let &local = args.get(1).unwrap_or(&remote);
    let mut file = File::create(local).await?;
    let result = editor.stream.simple_retr(remote).await?;
    file.write_all(&result.into_inner()).await?;
    Ok(())
}

async fn put(editor: &mut AftpEditor, args: &[&str]) -> Result<(), AftpIoError> {
    if !(1..=2).contains(&args.len()) {
        return Err(AftpIoError::InvalidArguments);
    }
    let &local = args.first().unwrap();
    let &remote = args.get(1).unwrap_or(&local);
    let mut file = File::open(local).await?;
    editor.stream.put(remote, &mut file).await?;
    Ok(())
}

#[derive(Completer, Helper, Validator)]
struct AftpEditorHelper {
    masking: bool,
    hints: HashSet<AftpEditorHint>,
}

impl Highlighter for AftpEditorHelper {
    fn highlight<'l>(&self, line: &'l str, _: usize) -> Cow<'l, str> {
        if self.masking {
            Cow::Owned("*".repeat(line.len()))
        } else {
            Cow::Borrowed(line)
        }
    }

    fn highlight_char(&self, _: &str, _: usize) -> bool {
        self.masking
    }
}

#[derive(Hash, Debug, PartialEq, Eq)]
struct AftpEditorHint {
    content: String,
}

impl Hint for AftpEditorHint {
    fn display(&self) -> &str {
        &self.content
    }

    fn completion(&self) -> Option<&str> {
        Some(&self.content[..])
    }
}

impl AftpEditorHint {
    fn new(content: String) -> Self {
        Self { content }
    }

    fn suffix(&self, index: usize) -> Self {
        Self {
            content: self.content[index..].into(),
        }
    }
}

impl Hinter for AftpEditorHelper {
    type Hint = AftpEditorHint;

    fn hint(&self, line: &str, pos: usize, _: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if line.is_empty() || pos < line.len() {
            return None;
        }

        self.hints.iter().find_map(|hint| {
            if hint.content.starts_with(line) {
                Some(hint.suffix(pos))
            } else {
                None
            }
        })
    }
}
