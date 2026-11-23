//! `bash`: one shell command in the workdir, with a timeout, cancellation and capped output.

use std::time::Duration;

use base::process::Cmd;
use model::{ToolClass, ToolDisplay, ToolSpec};
use serde_json::{Value, json};

use crate::api::{Tool, ToolContext, ToolOutput};
use crate::args::{Args, peek_str};
use crate::builtin::names::BASH;

/// Seconds a command may run when the call names no timeout.
pub const BASH_DEFAULT_TIMEOUT_SECS: u64 = 120;
/// The longest timeout a call may ask for.
pub const BASH_MAX_TIMEOUT_SECS: u64 = 600;
/// Bytes kept from the start and from the end of stdout and of stderr.
const OUTPUT_CAP: usize = 15 * 1024;

const USAGE: &str = r#"{"command": "cargo test -p tools", "timeout_secs": 300}"#;

/// Runs `sh -c <command>` in the workdir.
pub struct Bash;

impl Tool for Bash {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: BASH.into(),
            description: "Run a shell command with `sh -c` in the workspace root. Every call starts fresh \
                there: `cd` and variables do not carry over. Stdin is closed, so commands must not wait for \
                input. Returns stdout then stderr (when long, the start and the end are kept) and the exit \
                code. Default timeout 120 s, at most 600 s; on timeout the command and everything it started \
                are killed. Use it for builds, tests, git and other command-line tools; use read_file, \
                list_dir, grep and edit_file to read and change files. The person may be asked to approve the \
                command."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The shell command to run."},
                    "timeout_secs": {"type": "integer", "minimum": 1, "maximum": BASH_MAX_TIMEOUT_SECS, "description": "Timeout in seconds (default 120, at most 600)."}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Execute
    }

    fn subject(&self, input: &Value) -> String {
        peek_str(input, "command").map(|c| c.trim().to_string()).unwrap_or_default()
    }

    fn preview(&self, _ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        ToolDisplay::Command { command: self.subject(input), output: String::new(), exit_code: None }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        let (command, timeout) = match parse(input) {
            Ok(parsed) => parsed,
            Err(e) => return ToolOutput::error(BASH, e),
        };
        let mut cmd = Cmd::shell(&command)
            .cwd(ctx.root)
            .timeout(Duration::from_secs(timeout))
            .capture_limit(OUTPUT_CAP, OUTPUT_CAP);
        for (key, value) in ctx.env {
            cmd = cmd.env(key, value);
        }
        let out = match cmd.run(ctx.cancel) {
            Ok(out) => out,
            Err(e) => return ToolOutput::error(BASH, format!("Could not start the command: {e}")),
        };

        let output = out.combined();
        let mut content = if output.trim().is_empty() { "(no output)\n".to_string() } else { output.clone() };
        if !content.ends_with('\n') {
            content.push('\n');
        }
        if out.truncated {
            content.push_str("[The output was long; only its start and its end are shown.]\n");
        }
        if out.cancelled {
            content.push_str("[Cancelled; the command and everything it started were stopped.]\n");
        } else if out.timed_out {
            content.push_str(&format!(
                "[Timed out after {timeout} s; the command and everything it started were killed. Pass a \
                 larger timeout_secs (at most {BASH_MAX_TIMEOUT_SECS}) or run a smaller step.]\n"
            ));
        } else {
            match out.code {
                Some(code) => content.push_str(&format!("[Exit code {code}]\n")),
                None => content.push_str("[Ended by a signal]\n"),
            }
        }
        ToolOutput {
            content,
            is_error: !out.success(),
            display: ToolDisplay::Command { command, output, exit_code: out.code },
            files: Vec::new(),
        }
    }
}

/// The command and the timeout in seconds, clamped to `1..=600`.
fn parse(input: &Value) -> Result<(String, u64), String> {
    let args = Args::new(BASH, USAGE, input)?;
    let command = args.str("command")?.trim();
    if command.is_empty() {
        return Err(format!("{BASH}: `command` must not be empty. Example input: {USAGE}"));
    }
    let timeout = args.opt_u64("timeout_secs")?.unwrap_or(BASH_DEFAULT_TIMEOUT_SECS).clamp(1, BASH_MAX_TIMEOUT_SECS);
    Ok((command.to_string(), timeout))
}
