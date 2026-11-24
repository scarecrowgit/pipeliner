//! The command line of issues, runs, reviews and pipelines, parsed by hand.

use anyhow::{Result, anyhow, bail};

/// `issue …`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueCmd {
    New { title: String, body: String, pipeline: Option<String>, project: Option<String> },
    List { project: Option<String> },
    Show { key: String },
    Comment { key: String, text: String },
    Start { key: String },
}

/// `run …`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCmd {
    Start { pipeline: String, text: String, test: bool },
    List,
    Show { id: String },
    Log { id: String, follow: bool },
    Cancel { id: String },
    Resume { id: String },
    Answer { id: String, text: String, node: Option<String> },
}

/// Where a sent-back round starts, as typed after `--from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    All,
    Over,
    Variant(u32),
}

/// `review …`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewCmd {
    List,
    Show { id: String },
    Ship { id: String, variant: Option<u32> },
    Skip { id: String },
    SendBack { id: String, note: String, from: From, variants: u32 },
}

/// `pipeline …`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineCmd {
    List,
    Problems { id: String },
    Deploy { id: String, note: String },
}

/// Words and `--flag value` options of one subcommand.
struct Words<'a> {
    name: String,
    words: Vec<&'a str>,
    flags: Vec<(&'a str, Option<&'a str>)>,
}

/// Splits `options` by the flags each takes: `valued` take a value, `bare` do not.
fn split<'a>(name: &str, options: &[&'a str], valued: &[&str], bare: &[&str]) -> Result<Words<'a>> {
    let mut out = Words { name: name.to_string(), words: Vec::new(), flags: Vec::new() };
    let mut it = options.iter().copied();
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.words.extend(it.by_ref());
        } else if let Some((flag, value)) = arg.split_once('=').filter(|(f, _)| valued.contains(f)) {
            out.flags.push((flag, Some(value)));
        } else if valued.contains(&arg) {
            let value = it.next().ok_or_else(|| anyhow!("{name}: {arg} needs a value"))?;
            out.flags.push((arg, Some(value)));
        } else if bare.contains(&arg) {
            out.flags.push((arg, None));
        } else if arg.starts_with("--") || (arg.starts_with('-') && arg.len() == 2) {
            bail!("{name}: unknown option {arg}");
        } else {
            out.words.push(arg);
        }
    }
    Ok(out)
}

impl<'a> Words<'a> {
    fn value(&self, flag: &str) -> Option<String> {
        self.flags.iter().rev().find(|(f, _)| *f == flag).and_then(|(_, v)| v.map(str::to_string))
    }

    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|(f, _)| *f == flag)
    }

    /// The one word expected, named `what`.
    fn one(&self, what: &str) -> Result<String> {
        match self.words.as_slice() {
            [one] => Ok(one.to_string()),
            [] => bail!("{} needs {what}", self.name),
            [_, extra, ..] => bail!("{}: unexpected {extra}", self.name),
        }
    }

    /// The first word, and the rest joined as text.
    fn head_and_text(&self, head: &str, text: &str) -> Result<(String, String)> {
        let Some((first, rest)) = self.words.split_first() else { bail!("{} needs {head}", self.name) };
        let joined = rest.join(" ");
        if joined.trim().is_empty() {
            bail!("{} needs {text}", self.name);
        }
        Ok((first.to_string(), joined))
    }

    fn none(&self) -> Result<()> {
        match self.words.first() {
            None => Ok(()),
            Some(extra) => bail!("{}: unexpected {extra}", self.name),
        }
    }
}

fn sub<'a>(group: &str, options: &'a [&'a str]) -> Result<(&'a str, &'a [&'a str])> {
    options.split_first().map(|(name, rest)| (*name, rest)).ok_or_else(|| anyhow!("{group} needs a subcommand; see `harness-cli help`"))
}

pub fn parse_issue(options: &[&str]) -> Result<IssueCmd> {
    let (name, rest) = sub("issue", options)?;
    let label = format!("issue {name}");
    Ok(match name {
        "new" => {
            let w = split(&label, rest, &["--body", "--pipeline", "--project"], &[])?;
            let title = w.words.join(" ");
            if title.trim().is_empty() {
                bail!("issue new needs a title");
            }
            IssueCmd::New { title, body: w.value("--body").unwrap_or_default(), pipeline: w.value("--pipeline"), project: w.value("--project") }
        }
        "list" => {
            let w = split(&label, rest, &["--project"], &[])?;
            w.none()?;
            IssueCmd::List { project: w.value("--project") }
        }
        "show" => IssueCmd::Show { key: split(&label, rest, &[], &[])?.one("an issue key")? },
        "comment" => {
            let (key, text) = split(&label, rest, &[], &[])?.head_and_text("an issue key", "the comment")?;
            IssueCmd::Comment { key, text }
        }
        "start" => IssueCmd::Start { key: split(&label, rest, &[], &[])?.one("an issue key")? },
        other => bail!("unknown issue command {other}; see `harness-cli help`"),
    })
}

pub fn parse_run(options: &[&str]) -> Result<RunCmd> {
    let (name, rest) = sub("run", options)?;
    let label = format!("run {name}");
    Ok(match name {
        "start" => {
            let w = split(&label, rest, &[], &["--test"])?;
            let (pipeline, text) = w.head_and_text("a pipeline id", "the text of the item")?;
            RunCmd::Start { pipeline, text, test: w.has("--test") }
        }
        "list" => {
            split(&label, rest, &[], &[])?.none()?;
            RunCmd::List
        }
        "show" => RunCmd::Show { id: split(&label, rest, &[], &[])?.one("a run id")? },
        "log" => {
            let w = split(&label, rest, &[], &["-f", "--follow"])?;
            RunCmd::Log { id: w.one("a run id")?, follow: w.has("-f") || w.has("--follow") }
        }
        "cancel" => RunCmd::Cancel { id: split(&label, rest, &[], &[])?.one("a run id")? },
        "resume" => RunCmd::Resume { id: split(&label, rest, &[], &[])?.one("a run id")? },
        "answer" => {
            let w = split(&label, rest, &["--node"], &[])?;
            let (id, text) = w.head_and_text("a run id", "the answer")?;
            RunCmd::Answer { id, text, node: w.value("--node") }
        }
        other => bail!("unknown run command {other}; see `harness-cli help`"),
    })
}

fn parse_from(text: &str) -> Result<From> {
    match text {
        "all" => Ok(From::All),
        "over" => Ok(From::Over),
        other => match other.strip_prefix("variant:").map(str::parse::<u32>) {
            Some(Ok(n)) if n > 0 => Ok(From::Variant(n)),
            _ => bail!("--from takes all, over or variant:N"),
        },
    }
}

pub fn parse_review(options: &[&str]) -> Result<ReviewCmd> {
    let (name, rest) = sub("review", options)?;
    let label = format!("review {name}");
    Ok(match name {
        "list" => {
            split(&label, rest, &[], &[])?.none()?;
            ReviewCmd::List
        }
        "show" => ReviewCmd::Show { id: split(&label, rest, &[], &[])?.one("a review id")? },
        "ship" => {
            let w = split(&label, rest, &[], &[])?;
            match w.words.as_slice() {
                [id] => ReviewCmd::Ship { id: id.to_string(), variant: None },
                [id, variant] => ReviewCmd::Ship { id: id.to_string(), variant: Some(parse_variant(variant)?) },
                [] => bail!("review ship needs a review id"),
                [_, _, extra, ..] => bail!("review ship: unexpected {extra}"),
            }
        }
        "skip" => ReviewCmd::Skip { id: split(&label, rest, &[], &[])?.one("a review id")? },
        "send-back" => {
            let w = split(&label, rest, &["--from", "--variants"], &[])?;
            let (id, note) = w.head_and_text("a review id", "a note")?;
            let from = w.value("--from").map_or(Ok(From::All), |f| parse_from(&f))?;
            let variants = match w.value("--variants") {
                Some(n) => n.parse().map_err(|_| anyhow!("--variants takes a number"))?,
                None => 0,
            };
            ReviewCmd::SendBack { id, note, from, variants }
        }
        other => bail!("unknown review command {other}; see `harness-cli help`"),
    })
}

/// A variant as a number or a letter (`2` or `B`).
fn parse_variant(text: &str) -> Result<u32> {
    if let Ok(n) = text.parse::<u32>() {
        return Ok(n);
    }
    match text.chars().collect::<Vec<_>>().as_slice() {
        [c] if c.is_ascii_alphabetic() => Ok(c.to_ascii_uppercase() as u32 - 'A' as u32 + 1),
        _ => bail!("a variant is a number or a letter, not {text}"),
    }
}

pub fn parse_pipeline(options: &[&str]) -> Result<PipelineCmd> {
    let (name, rest) = sub("pipeline", options)?;
    let label = format!("pipeline {name}");
    Ok(match name {
        "list" => {
            split(&label, rest, &[], &[])?.none()?;
            PipelineCmd::List
        }
        "problems" => PipelineCmd::Problems { id: split(&label, rest, &[], &[])?.one("a pipeline id")? },
        "deploy" => {
            let w = split(&label, rest, &["--note"], &[])?;
            PipelineCmd::Deploy { id: w.one("a pipeline id")?, note: w.value("--note").unwrap_or_default() }
        }
        other => bail!("unknown pipeline command {other}; see `harness-cli help`"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    fn strs(owned: &[String]) -> Vec<&str> {
        owned.iter().map(String::as_str).collect()
    }

    #[test]
    fn issue_commands() {
        let a = v(&["new", "Fix", "the", "retry", "--pipeline", "self", "--body=It retries twice"]);
        assert_eq!(
            parse_issue(&strs(&a)).unwrap(),
            IssueCmd::New { title: "Fix the retry".into(), body: "It retries twice".into(), pipeline: Some("self".into()), project: None }
        );
        assert_eq!(parse_issue(&strs(&v(&["comment", "HAR-1", "@fixer", "look"]))).unwrap(), IssueCmd::Comment { key: "HAR-1".into(), text: "@fixer look".into() });
        assert_eq!(parse_issue(&strs(&v(&["start", "HAR-1"]))).unwrap(), IssueCmd::Start { key: "HAR-1".into() });
        assert!(parse_issue(&strs(&v(&["new"]))).is_err());
        assert!(parse_issue(&strs(&v(&["show"]))).is_err());
        assert!(parse_issue(&strs(&v(&["start", "A", "B"]))).is_err());
        assert!(parse_issue(&strs(&v(&["frob"]))).is_err());
        assert!(parse_issue(&[]).is_err());
    }

    #[test]
    fn run_commands() {
        assert_eq!(
            parse_run(&strs(&v(&["start", "demo", "Make", "it", "--test", "short"]))).unwrap(),
            RunCmd::Start { pipeline: "demo".into(), text: "Make it short".into(), test: true }
        );
        assert_eq!(parse_run(&strs(&v(&["log", "r1", "-f"]))).unwrap(), RunCmd::Log { id: "r1".into(), follow: true });
        assert_eq!(
            parse_run(&strs(&v(&["answer", "r1", "use", "v2", "--node", "coder"]))).unwrap(),
            RunCmd::Answer { id: "r1".into(), text: "use v2".into(), node: Some("coder".into()) }
        );
        assert!(parse_run(&strs(&v(&["start", "demo"]))).is_err());
        assert!(parse_run(&strs(&v(&["list", "x"]))).is_err());
        assert!(parse_run(&strs(&v(&["log", "r1", "--bogus"]))).is_err());
    }

    #[test]
    fn review_and_pipeline_commands() {
        assert_eq!(parse_review(&strs(&v(&["ship", "rv", "B"]))).unwrap(), ReviewCmd::Ship { id: "rv".into(), variant: Some(2) });
        assert_eq!(parse_review(&strs(&v(&["ship", "rv"]))).unwrap(), ReviewCmd::Ship { id: "rv".into(), variant: None });
        assert_eq!(
            parse_review(&strs(&v(&["send-back", "rv", "Shorter", "please", "--from", "variant:2", "--variants", "3"]))).unwrap(),
            ReviewCmd::SendBack { id: "rv".into(), note: "Shorter please".into(), from: From::Variant(2), variants: 3 }
        );
        assert!(parse_review(&strs(&v(&["send-back", "rv"]))).is_err());
        assert!(parse_review(&strs(&v(&["send-back", "rv", "x", "--from", "some"]))).is_err());
        assert_eq!(parse_pipeline(&strs(&v(&["deploy", "demo", "--note", "v2"]))).unwrap(), PipelineCmd::Deploy { id: "demo".into(), note: "v2".into() });
        assert_eq!(parse_pipeline(&strs(&v(&["problems", "demo"]))).unwrap(), PipelineCmd::Problems { id: "demo".into() });
        assert!(parse_pipeline(&strs(&v(&["deploy"]))).is_err());
    }
}
