//! Driver scripts for `harness-dev run --script`: one driver command per line (the socket
//! protocol, e.g. `click increment`, `type "hello world"`), plus two client-side lines:
//! `shot <path>` (wait for a frame, screenshot the window) and `sleep <ms>`. Blank lines and
//! lines starting with `#` are skipped.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail};
use ui::dev::proto::{self, Command};

/// One line of a script.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Command(Command),
    Shot(PathBuf),
    Sleep(Duration),
}

/// Parse a script; errors name the line.
pub fn parse(text: &str) -> anyhow::Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let step = parse_line(line).map_err(|e| anyhow!("line {}: {e}", i + 1))?;
        steps.push(step);
    }
    Ok(steps)
}

fn parse_line(line: &str) -> anyhow::Result<Step> {
    let words = proto::split(line).map_err(|e| anyhow!(e))?;
    Ok(match words.as_slice() {
        [verb, path] if verb == "shot" => Step::Shot(PathBuf::from(path)),
        [verb, ..] if verb == "shot" => bail!("shot takes one path"),
        [verb, ms] if verb == "sleep" => {
            Step::Sleep(Duration::from_millis(ms.parse().map_err(|_| anyhow!("bad ms: {ms}"))?))
        }
        [verb, ..] if verb == "sleep" => bail!("sleep takes milliseconds"),
        _ => Step::Command(Command::parse(line).map_err(|e| anyhow!(e))?),
    })
}
