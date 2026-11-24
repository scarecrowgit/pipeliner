//! The command line, parsed by hand.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

/// Environment variable that points the CLI at another data directory, like `--data-dir`.
pub const DATA_DIR_ENV: &str = "HARNESS_CLI_DATA_DIR";

pub const USAGE: &str = "\
Usage: harness-cli <command> [options]

Commands:
  chat [--dir <path>] [--model <provider/model>] [--session <id>] [--yes] [<message>]
        Talks to the agent. With a message: one turn, then exit. Without: one turn per line,
        until `quit` or end of input. Ctrl-C stops the running turn.
        --dir      the folder the agent works in (default: the current one)
        --model    the model to use, e.g. anthropic/claude-sonnet-5-5
        --session  continue an existing chat instead of starting one
        --yes      allow every tool call without asking
  sessions        lists the chats, most recent first
  models          lists the configured providers and their models
  import [--dry-run]
        imports chats, issues and the Inbox of the old app
  help            shows this text
";

/// What `chat` was asked to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatArgs {
    pub dir: Option<PathBuf>,
    pub model: Option<String>,
    pub session: Option<String>,
    pub yes: bool,
    /// The one-shot message; `None` starts the line-based loop.
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Chat(ChatArgs),
    Sessions,
    Models,
    Import { dry_run: bool },
    Help,
}

/// A parsed command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    /// Hidden: a directory that stands in for the user's config and data.
    pub data_dir: Option<PathBuf>,
    pub command: Command,
}

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Args> {
    let mut data_dir = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(arg) = it.next() {
        if arg == "--data-dir" {
            data_dir = Some(PathBuf::from(it.next().ok_or_else(|| anyhow!("--data-dir needs a path"))?));
        } else if let Some(value) = arg.strip_prefix("--data-dir=") {
            data_dir = Some(PathBuf::from(value));
        } else {
            rest.push(arg);
        }
    }
    let Some((&name, options)) = rest.split_first() else {
        return Ok(Args { data_dir, command: Command::Help });
    };
    let command = match name {
        "chat" => Command::Chat(parse_chat(options)?),
        "sessions" => no_options("sessions", options, Command::Sessions)?,
        "models" => no_options("models", options, Command::Models)?,
        "import" => match options {
            [] => Command::Import { dry_run: false },
            ["--dry-run"] => Command::Import { dry_run: true },
            [other, ..] => bail!("import: unknown option {other}"),
        },
        "help" | "--help" | "-h" => Command::Help,
        other => bail!("unknown command {other}; see `harness-cli help`"),
    };
    Ok(Args { data_dir, command })
}

fn no_options(name: &str, options: &[&str], command: Command) -> Result<Command> {
    match options.first() {
        None => Ok(command),
        Some(other) => bail!("{name}: unknown option {other}"),
    }
}

fn parse_chat(options: &[&str]) -> Result<ChatArgs> {
    let mut chat = ChatArgs::default();
    let mut words: Vec<&str> = Vec::new();
    let mut it = options.iter().copied();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| it.next().map(str::to_string).ok_or_else(|| anyhow!("{flag} needs a value"));
        match arg {
            "--dir" => chat.dir = Some(PathBuf::from(value("--dir")?)),
            "--model" => chat.model = Some(value("--model")?),
            "--session" => chat.session = Some(value("--session")?),
            "--yes" | "-y" => chat.yes = true,
            "--" => words.extend(it.by_ref()),
            flag if flag.starts_with("--") => bail!("chat: unknown option {flag}"),
            word => words.push(word),
        }
    }
    let message = words.join(" ");
    chat.message = (!message.trim().is_empty()).then_some(message);
    if chat.model.as_deref().is_some_and(|m| !m.contains('/')) {
        bail!("--model takes provider/model, e.g. anthropic/claude-sonnet-5-5");
    }
    Ok(chat)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &[&str]) -> Result<Args> {
        parse(&text.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn chat_takes_its_options_and_the_message_in_any_order() {
        let parsed = args(&["--data-dir", "/tmp/x", "chat", "--yes", "fix", "the", "--model", "a/b", "bug"]).expect("parse");
        assert_eq!(parsed.data_dir, Some(PathBuf::from("/tmp/x")));
        assert_eq!(
            parsed.command,
            Command::Chat(ChatArgs {
                dir: None,
                model: Some("a/b".into()),
                session: None,
                yes: true,
                message: Some("fix the bug".into())
            })
        );
        let parsed = args(&["chat", "--dir", "src", "--session", "s1", "--", "--literal"]).expect("parse");
        let Command::Chat(chat) = parsed.command else { panic!("not chat") };
        assert_eq!((chat.dir, chat.session, chat.message), (Some("src".into()), Some("s1".into()), Some("--literal".into())));
        let Command::Chat(chat) = args(&["chat"]).expect("parse").command else { panic!("not chat") };
        assert_eq!(chat.message, None, "no message starts the loop");
    }

    #[test]
    fn other_commands_and_mistakes() {
        assert_eq!(args(&[]).expect("parse").command, Command::Help);
        assert_eq!(args(&["sessions"]).expect("parse").command, Command::Sessions);
        assert_eq!(args(&["models"]).expect("parse").command, Command::Models);
        assert_eq!(args(&["import", "--dry-run"]).expect("parse").command, Command::Import { dry_run: true });
        assert_eq!(args(&["--data-dir=/d", "import"]).expect("parse").data_dir, Some(PathBuf::from("/d")));
        assert!(args(&["chat", "--model"]).is_err());
        assert!(args(&["chat", "--model", "nomodel"]).is_err());
        assert!(args(&["chat", "--bogus"]).is_err());
        assert!(args(&["sessions", "--all"]).is_err());
        assert!(args(&["frobnicate"]).is_err());
        assert!(args(&["--data-dir"]).is_err());
    }
}
