//! Tools for the chat agent, scoped to a workspace directory.
//!
//! | tool | permission | what |
//! | --- | --- | --- |
//! | `read_file {path, offset?, limit?}` | auto | numbered lines, max 2000 lines / 256 KB |
//! | `list_dir {path?}` | auto | entries (dirs end with `/`), skips .git, target, node_modules |
//! | `grep {pattern, path?, glob?}` | auto | regex search, `file:line: text`, max 200 matches, respects .gitignore |
//! | `edit_file {path, old_string, new_string}` | ask | exact unique replacement |
//! | `write_file {path, content}` | ask | create/overwrite |
//! | `bash {command, timeout_secs?}` | ask | `sh -c` in the workspace root, output capped at 30 KB, default timeout 120 s |
//!
//! All paths are relative to the workspace root; absolute paths or `..` escaping the root
//! are rejected. Blocking — run on a background thread.

mod bash;
mod context;
mod diff;
mod paths;
mod specs;
mod tools;
#[cfg(test)]
mod tests;

pub mod inbox;
pub mod agents;
pub mod skills;
pub mod workdir;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tools::error;

pub struct Workspace {
    pub root: PathBuf,
    /// Extra environment for `bash` (e.g. a shared `CARGO_TARGET_DIR` for pipeline worktrees).
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Safe, read-only: run without asking.
    Auto,
    /// Mutates files or runs commands: ask the user first.
    Ask,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    /// 1-based line number in the new file (old file for `Del`).
    pub line_no: usize,
    pub text: String,
}

/// Structured view of a tool call for the UI (tool-call rows, diff blocks, bash output).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolDisplay {
    Read { path: String, lines: usize },
    List { path: String, entries: usize },
    Grep { pattern: String, matches: usize, files: usize },
    /// For edit_file / write_file: unified diff hunks with ~3 context lines.
    Diff { path: String, added: usize, removed: usize, lines: Vec<DiffLine>, created: bool },
    Bash { command: String, output: String, exit_code: Option<i32>, timed_out: bool },
    /// Invalid input / failure — `message` is shown in danger color.
    Error { message: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    /// Text returned to the model as the tool result.
    pub content: String,
    pub is_error: bool,
    pub display: ToolDisplay,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            env: Vec::new(),
        }
    }

    /// Same workspace, with `env` set for every `bash` command.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Specs for all tools, to put in `llm::ChatRequest::tools`.
    pub fn tool_specs(&self) -> Vec<llm::ToolSpec> {
        specs::tool_specs()
    }

    pub fn permission(&self, tool: &str) -> Permission {
        match tool {
            "read_file" | "list_dir" | "grep" => Permission::Auto,
            _ => Permission::Ask,
        }
    }

    /// What the call WOULD do, for the permission prompt (e.g. the diff of an edit, the
    /// bash command). Must not modify anything. `Error` display if the input is invalid.
    pub fn preview(&self, tool: &str, input: &serde_json::Value) -> ToolDisplay {
        let input = normalize_input(input);
        let input = &input;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.preview_inner(tool, input)));
        result.unwrap_or_else(|_| ToolDisplay::Error { message: format!("{tool}: internal error while previewing") })
    }

    /// Execute the call. Never panics; failures come back as `is_error` outputs.
    /// `cancel` aborts long-running `bash` (kill the child) and `grep`.
    pub fn execute(&self, tool: &str, input: &serde_json::Value, cancel: Arc<AtomicBool>) -> ToolOutput {
        let input = normalize_input(input);
        let input = &input;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.execute_inner(tool, input, &cancel)
        }));
        result.unwrap_or_else(|_| error(format!("{tool}: internal error while executing")))
    }

    /// Short project context for the system prompt: root path, git branch (if any),
    /// top-level entries, and the contents of AGENTS.md / CLAUDE.md if present (truncated).
    pub fn context_summary(&self) -> String {
        context::context_summary(&self.root)
    }

    /// Workspace-relative file paths for @-mention completion (respects .gitignore,
    /// skips hidden/target/node_modules), sorted, at most `limit`.
    pub fn files(&self, limit: usize) -> Vec<String> {
        context::files(&self.root, limit)
    }

    /// `(repo name, git branch)` for the chat header pill, e.g. `("pipeliner", Some("main"))`.
    pub fn repo_label(&self) -> (String, Option<String>) {
        context::repo_label(&self.root)
    }

    fn preview_inner(&self, tool: &str, input: &serde_json::Value) -> ToolDisplay {
        let root = &self.root;
        let as_err = |message: String| ToolDisplay::Error { message };
        match tool {
            "read_file" => match tools::read_args(input) {
                Ok((path, _, _)) => match paths::resolve(root, path) {
                    Ok(r) => ToolDisplay::Read { path: r.rel, lines: 0 },
                    Err(e) => as_err(e),
                },
                Err(e) => as_err(e),
            },
            "list_dir" => match tools::opt_str(input, "path", "list_dir") {
                Ok(p) => match paths::resolve(root, p.unwrap_or(".")) {
                    Ok(r) => ToolDisplay::List { path: r.rel, entries: 0 },
                    Err(e) => as_err(e),
                },
                Err(e) => as_err(e),
            },
            "grep" => match tools::grep_args(root, input) {
                Ok(a) => ToolDisplay::Grep { pattern: a.pattern.to_string(), matches: 0, files: 0 },
                Err(e) => as_err(e),
            },
            "edit_file" => tools::plan_edit(root, input).map(|p| p.display).unwrap_or_else(as_err),
            "write_file" => tools::plan_write(root, input).map(|p| p.display).unwrap_or_else(as_err),
            "bash" => match bash_args(input) {
                Ok((command, _)) => ToolDisplay::Bash {
                    command: command.to_string(),
                    output: String::new(),
                    exit_code: None,
                    timed_out: false,
                },
                Err(e) => as_err(e),
            },
            _ => as_err(unknown_tool(tool)),
        }
    }

    fn execute_inner(&self, tool: &str, input: &serde_json::Value, cancel: &AtomicBool) -> ToolOutput {
        let root = &self.root;
        match tool {
            "read_file" => tools::read_file(root, input),
            "list_dir" => tools::list_dir(root, input),
            "grep" => tools::grep(root, input, cancel),
            "edit_file" => match tools::plan_edit(root, input) {
                Ok(plan) => tools::apply(plan, "edit_file"),
                Err(e) => error(e),
            },
            "write_file" => match tools::plan_write(root, input) {
                Ok(plan) => tools::apply(plan, "write_file"),
                Err(e) => error(e),
            },
            "bash" => self.run_bash(input, cancel),
            _ => error(unknown_tool(tool)),
        }
    }

    fn run_bash(&self, input: &serde_json::Value, cancel: &AtomicBool) -> ToolOutput {
        let (command, timeout) = match bash_args(input) {
            Ok(a) => a,
            Err(e) => return error(e),
        };
        let root = paths::canonical_root(&self.root);
        let res = match bash::run(&root, command, &self.env, Duration::from_secs(timeout), cancel) {
            Ok(r) => r,
            Err(e) => return error(format!("bash: {e}")),
        };
        let mut content = if res.output.trim().is_empty() { "(no output)".to_string() } else { res.output.clone() };
        if !content.ends_with('\n') {
            content.push('\n');
        }
        if res.cancelled {
            content.push_str("[cancelled by the user; the process was killed]");
        } else if res.timed_out {
            content.push_str(&format!("[timed out after {timeout} s; the process was killed]"));
        } else {
            match res.exit_code {
                Some(code) => content.push_str(&format!("[exit code: {code}]")),
                None => content.push_str("[terminated by a signal]"),
            }
        }
        let is_error = res.cancelled || res.timed_out || res.exit_code != Some(0);
        ToolOutput {
            content,
            is_error,
            display: ToolDisplay::Bash {
                command: command.to_string(),
                output: res.output,
                exit_code: res.exit_code,
                timed_out: res.timed_out,
            },
        }
    }
}

/// Some providers deliver tool input as a JSON-encoded string; accept that too.
fn normalize_input(input: &serde_json::Value) -> serde_json::Value {
    match input {
        serde_json::Value::String(s) => match serde_json::from_str::<serde_json::Value>(s) {
            Ok(v @ serde_json::Value::Object(_)) => v,
            _ => input.clone(),
        },
        _ => input.clone(),
    }
}

fn bash_args(input: &serde_json::Value) -> Result<(&str, u64), String> {
    let command = tools::req_str(input, "command", "bash")?;
    if command.trim().is_empty() {
        return Err("bash: `command` must not be empty".into());
    }
    let timeout = tools::opt_uint(input, "timeout_secs", "bash")?
        .unwrap_or(bash::DEFAULT_TIMEOUT_SECS)
        .clamp(1, bash::MAX_TIMEOUT_SECS);
    Ok((command, timeout))
}

fn unknown_tool(tool: &str) -> String {
    format!("Unknown tool `{tool}`. Available tools: read_file, list_dir, grep, edit_file, write_file, bash.")
}
