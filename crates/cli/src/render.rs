//! How agent events look on a terminal: streamed text, and one compact line per tool call,
//! result, sub-agent and notice.

use std::io::Write;

use engine::api::{AgentEvent, PermissionRequest, ToolDisplay};

/// Longest tool line, in characters.
const LINE_MAX: usize = 120;

/// Prints the events of one chat. Text of the chat itself streams to `out`; sub-agents show
/// as one line when they start and one when they finish.
pub struct Printer {
    /// The last thing printed to `out` was streamed text without a newline.
    mid_line: bool,
}

impl Printer {
    pub fn new() -> Self {
        Self { mid_line: false }
    }

    /// Ends a streamed line, so the next line starts at the margin.
    pub fn break_line(&mut self, out: &mut dyn Write) {
        if self.mid_line {
            let _ = writeln!(out);
            self.mid_line = false;
        }
    }

    /// Prints one event of the chat (`own` is false for a sub-agent's events).
    pub fn event(&mut self, out: &mut dyn Write, own: bool, event: &AgentEvent) {
        match event {
            AgentEvent::TextDelta { text } if own => {
                let _ = write!(out, "{text}");
                self.mid_line = !text.ends_with('\n');
            }
            AgentEvent::ToolCallReady { name, display, .. } if own => {
                self.line(out, &format!("  → {}", call_line(name, display)));
            }
            AgentEvent::ToolFinished { display, is_error, .. } if own => {
                let mark = if *is_error { "✗" } else { "✓" };
                self.line(out, &format!("    {mark} {}", result_line(display)));
            }
            AgentEvent::SubagentStarted { name, task, .. } => {
                self.line(out, &format!("  ↳ {name}: {}", base::fmt::first_line(task)));
            }
            AgentEvent::SubagentFinished { summary, ok, cost_usd, .. } => {
                let mark = if *ok { "✓" } else { "✗" };
                let cost = base::fmt::cost(*cost_usd);
                self.line(out, &format!("  ↳ {mark} {} ({cost})", base::fmt::first_line(summary)));
            }
            AgentEvent::Retrying { attempt, reason } if own => {
                self.line(out, &format!("  … retrying ({attempt}): {reason}"));
            }
            AgentEvent::Compacted { summarised, .. } if own && *summarised > 0 => {
                self.line(out, &format!("  … summarised {summarised} older messages"));
            }
            AgentEvent::Notice { text } if own => self.line(out, &format!("  note: {text}")),
            AgentEvent::PrDetected { url } if own => self.line(out, &format!("  pull request: {url}")),
            AgentEvent::Failed { error } if own => self.line(out, &format!("error: {error}")),
            _ => {}
        }
        let _ = out.flush();
    }

    fn line(&mut self, out: &mut dyn Write, text: &str) {
        self.break_line(out);
        let _ = writeln!(out, "{}", base::fmt::truncate_chars(text, LINE_MAX));
    }
}

/// What a call will do: `read src/a.rs`, `$ cargo test`, `edit src/a.rs`.
fn call_line(name: &str, display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::Line { verb, target, .. } => format!("{verb} {target}"),
        ToolDisplay::Command { command, .. } => format!("$ {}", base::fmt::first_line(command)),
        ToolDisplay::Diff(diff) => format!("edit {} (+{} −{})", diff.path, diff.added, diff.removed),
        ToolDisplay::Text { title, .. } if !title.is_empty() => format!("{name}: {title}"),
        ToolDisplay::Text { .. } => name.to_string(),
    }
}

/// What a call came back with, in one line.
fn result_line(display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::Line { result, .. } => result.clone(),
        ToolDisplay::Command { output, exit_code, .. } => {
            let code = exit_code.map_or_else(|| "stopped".to_string(), |c| format!("exit {c}"));
            let last = output.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
            if last.is_empty() { code } else { format!("{code} · {}", last.trim()) }
        }
        ToolDisplay::Diff(diff) => format!("{} (+{} −{})", diff.path, diff.added, diff.removed),
        ToolDisplay::Text { title, body } => {
            let first = base::fmt::first_line(body.trim());
            if first.is_empty() { title.clone() } else { first.to_string() }
        }
    }
}

/// The question for a permission request.
pub fn question(request: &PermissionRequest) -> String {
    format!(
        "{} wants to {} ({}). Allow? [y]es / [n]o / [a]lways: ",
        request.agent,
        call_line(&request.tool, &request.display),
        request.policy
    )
}
