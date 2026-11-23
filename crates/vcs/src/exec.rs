//! The one place that starts `git` and `gh`: environment, timeouts and error messages.
//!
//! Every command runs without a terminal prompt, an editor or a pager, in its own process
//! group (through [`base::process::Cmd`]), with a timeout. A failed command becomes an
//! `anyhow` error that carries the command line and its stderr, with credentials in URLs
//! removed.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow};
use base::cancel::CancelToken;
use base::process::{Cmd, Output};

/// Read-only queries: status, rev-parse, branch listings.
pub(crate) const QUICK: Duration = Duration::from_secs(20);
/// Local writes that touch the working tree: checkouts, commits, merges.
pub(crate) const LOCAL: Duration = Duration::from_secs(120);
/// Anything that talks to a remote: clone, push, `gh`.
pub(crate) const NETWORK: Duration = Duration::from_secs(600);

/// Bytes kept from the start of a command's output; the rest of a huge diff is dropped.
const CAPTURE_HEAD: usize = 8 * 1024 * 1024;
const CAPTURE_TAIL: usize = 64 * 1024;

/// Environment that keeps every command from waiting for a person.
const ENV: [(&str, &str); 11] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GCM_INTERACTIVE", "never"),
    ("GH_PROMPT_DISABLED", "1"),
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_EDITOR", "true"),
    ("GIT_SEQUENCE_EDITOR", "true"),
    ("GIT_MERGE_AUTOEDIT", "no"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("GH_NO_UPDATE_NOTIFIER", "1"),
    ("NO_COLOR", "1"),
];

/// Variables that would point git at a different repository than the folder we run in (they
/// are set when the app itself is started from a git hook).
const ENV_REMOVE: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_NAMESPACE",
];

/// A `git` or `gh` command being put together.
#[derive(Clone, Debug)]
pub(crate) struct Tool {
    program: &'static str,
    cwd: Option<PathBuf>,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    stdin: Option<String>,
    timeout: Duration,
}

impl Tool {
    /// `git` running in `dir`.
    pub(crate) fn git(dir: &Path) -> Self {
        Self::new("git", Some(dir))
    }

    /// `gh`, in `dir` when given (it reads the repository from there).
    pub(crate) fn gh(dir: Option<&Path>) -> Self {
        let mut tool = Self::new("gh", dir);
        tool.timeout = LOCAL;
        tool
    }

    fn new(program: &'static str, dir: Option<&Path>) -> Self {
        Self {
            program,
            cwd: dir.map(Path::to_path_buf),
            args: Vec::new(),
            env: Vec::new(),
            stdin: None,
            timeout: QUICK,
        }
    }

    pub(crate) fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub(crate) fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub(crate) fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    pub(crate) fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The command line for error messages, credentials removed.
    pub(crate) fn describe(&self) -> String {
        let mut text = self.program.to_string();
        for arg in &self.args {
            text.push(' ');
            text.push_str(&arg.to_string_lossy());
        }
        redact_credentials(&text)
    }

    /// Runs the command. A non-zero exit is an `Ok`; failing to start, a timeout and a cancel
    /// are errors.
    pub(crate) fn run(self, cancel: &CancelToken) -> Result<Output> {
        let described = self.describe();
        let timeout = self.timeout;
        let mut cmd = Cmd::new(self.program)
            .args(&self.args)
            .timeout(timeout)
            .capture_limit(CAPTURE_HEAD, CAPTURE_TAIL);
        if let Some(dir) = &self.cwd {
            cmd = cmd.cwd(dir);
        }
        for (key, value) in ENV {
            cmd = cmd.env(key, value);
        }
        for key in ENV_REMOVE {
            cmd = cmd.env_remove(key);
        }
        for (key, value) in &self.env {
            cmd = cmd.env(key, value);
        }
        if let Some(text) = self.stdin {
            cmd = cmd.stdin(text);
        }
        let out = cmd
            .run(cancel)
            .map_err(|e| anyhow!("could not start `{described}`: {e}"))?;
        if out.cancelled {
            return Err(anyhow!("`{described}` was cancelled"));
        }
        if out.timed_out {
            return Err(anyhow!("`{described}` timed out after {} s", timeout.as_secs()));
        }
        Ok(out)
    }

    /// Runs the command and returns its stdout; a non-zero exit is an error with stderr.
    pub(crate) fn check(self, cancel: &CancelToken) -> Result<String> {
        let described = self.describe();
        let out = self.run(cancel)?;
        if out.success() {
            Ok(out.stdout)
        } else {
            Err(failure(&described, &out))
        }
    }

    /// Stdout of a command that succeeded, `None` for anything else.
    pub(crate) fn query(self, cancel: &CancelToken) -> Option<String> {
        self.run(cancel).ok().filter(Output::success).map(|o| o.stdout)
    }

    /// Whether the command ran and exited with code 0.
    pub(crate) fn succeeds(self, cancel: &CancelToken) -> bool {
        self.run(cancel).is_ok_and(|o| o.success())
    }
}

/// The error for a command that ran and failed.
pub(crate) fn failure(described: &str, out: &Output) -> anyhow::Error {
    let text = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
    let code = out.code.map_or_else(|| "killed".to_string(), |c| format!("exit {c}"));
    if text.is_empty() {
        anyhow!("`{described}` failed ({code})")
    } else {
        anyhow!("`{described}` failed ({code}): {}", redact_credentials(text))
    }
}

/// A git command that will create commits: carries a fallback identity when none is
/// configured, so a machine without `user.name` still commits instead of failing.
pub(crate) fn git_committing(dir: &Path, cancel: &CancelToken) -> Tool {
    let configured = |key: &str| {
        Tool::git(dir)
            .args(["config", "--get", key])
            .query(cancel)
            .is_some_and(|v| !v.trim().is_empty())
    };
    let mut tool = Tool::git(dir);
    if !configured("user.name") || !configured("user.email") {
        tool = tool.args(["-c", "user.name=Harness", "-c", "user.email=harness@localhost"]);
    }
    tool
}

/// Removes `user[:password]@` after `scheme://` anywhere in `text`.
pub(crate) fn redact_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let (head, tail) = rest.split_at(i + 3);
        out.push_str(head);
        let authority_end = tail.find(|c: char| c == '/' || c.is_whitespace()).unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(&tail[at + 1..authority_end]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_removed_from_urls() {
        assert_eq!(
            redact_credentials("fatal: https://user:tok@github.com/a/b.git not found"),
            "fatal: https://github.com/a/b.git not found"
        );
        assert_eq!(redact_credentials("https://github.com/a/b"), "https://github.com/a/b");
        assert_eq!(
            redact_credentials("git clone https://a:b@h/x https://c@h2/y"),
            "git clone https://h/x https://h2/y"
        );
        assert_eq!(redact_credentials("no url here: a@b"), "no url here: a@b");
    }

    #[test]
    fn failed_commands_carry_the_command_and_stderr() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = Tool::git(dir.path())
            .args(["rev-parse", "--verify", "nope"])
            .check(&CancelToken::new())
            .expect_err("not a repo");
        let text = err.to_string();
        assert!(text.contains("git rev-parse --verify nope"), "{text}");
        assert!(text.contains("failed"), "{text}");
    }

    #[test]
    fn a_cancelled_token_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = Tool::git(dir.path()).arg("status").check(&cancel).expect_err("cancelled");
        assert!(err.to_string().contains("cancelled"));
    }
}
