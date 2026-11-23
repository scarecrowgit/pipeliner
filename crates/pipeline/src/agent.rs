//! A headless agent loop for pipeline stages: stream a turn, run its tool calls without
//! asking (workspace tools filtered per role, plus stage tools such as `delegate` or a
//! `*_done` tool that ends the loop), repeat until the model stops or calls a finish tool.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent_tools::Workspace;
use anyhow::{Context as _, anyhow, bail};
use llm::{ChatRequest, ContentBlock, Message, Role, StopReason, StreamEvent, ToolSpec, Usage};
use serde_json::Value;

use crate::model::{LogKind, LogLine, now};

/// What a stage tool returns to the model.
pub struct Reply {
    pub content: String,
    pub is_error: bool,
    /// End the loop after this call (a `*_done` tool).
    pub finish: bool,
}

impl Reply {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, finish: false }
    }
    pub fn err(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, finish: false }
    }
    pub fn finish(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, finish: true }
    }
}

/// A tool implemented by the stage (closure over the stage's state).
pub struct StageTool<'a> {
    pub spec: ToolSpec,
    pub call: Box<dyn FnMut(&Value) -> Reply + 'a>,
}

pub struct AgentSpec<'a> {
    /// Shown in the run log (`Designer`, `Coder 2`).
    pub name: String,
    /// `provider/model`.
    pub model: String,
    pub system: String,
    /// Workspace tools allowed for this agent (by name) and their workspace.
    pub workspace: Option<(Workspace, Vec<&'static str>)>,
    pub tools: Vec<StageTool<'a>>,
    pub max_steps: usize,
    pub max_tokens: u32,
}

pub struct Outcome {
    /// Last reply text.
    pub text: String,
    /// A finish tool was called.
    pub finished: bool,
    pub steps: usize,
    pub usage: Usage,
}

pub const READ_TOOLS: [&str; 3] = ["read_file", "list_dir", "grep"];
pub const ALL_TOOLS: [&str; 6] = ["read_file", "list_dir", "grep", "edit_file", "write_file", "bash"];

/// Run `spec` on `task` (the first user message). `log` receives progress lines.
pub fn run(
    spec: &mut AgentSpec,
    task: &str,
    log: &dyn Fn(LogLine),
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Outcome> {
    let config = llm::Config::load().context("loading ~/.config/harness/config.toml")?;
    let (provider_id, model) = spec
        .model
        .split_once('/')
        .ok_or_else(|| anyhow!("model `{}` must be `provider/model`", spec.model))?;
    let provider = config
        .provider(provider_id)
        .ok_or_else(|| anyhow!("provider `{provider_id}` is not configured"))?
        .clone();

    let mut tools: Vec<ToolSpec> = Vec::new();
    if let Some((ws, allowed)) = &spec.workspace {
        tools.extend(
            ws.tool_specs()
                .into_iter()
                .filter(|t| allowed.contains(&t.name.as_str())),
        );
    }
    tools.extend(spec.tools.iter().map(|t| t.spec.clone()));

    let say = |kind: LogKind, text: String| {
        log(LogLine { at: now(), who: spec.name.clone(), kind, text })
    };
    let mut messages = vec![Message::user_text(task)];
    let mut usage = Usage::default();
    let mut last_text = String::new();

    for step in 1..=spec.max_steps {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        let request = ChatRequest {
            model: model.to_string(),
            system: Some(spec.system.clone()),
            messages: messages.clone(),
            tools: tools.clone(),
            max_tokens: spec.max_tokens,
            temperature: None,
        };
        let turn = stream_with_retry(&provider, &request, cancel, &say)?;
        usage.input_tokens += turn.usage.input_tokens;
        usage.output_tokens += turn.usage.output_tokens;
        usage.cache_read_tokens += turn.usage.cache_read_tokens;
        usage.cache_write_tokens += turn.usage.cache_write_tokens;

        let mut content = Vec::new();
        if !turn.text.trim().is_empty() {
            say(LogKind::Text, clip(turn.text.trim(), 600));
            last_text = turn.text.clone();
            content.push(ContentBlock::Text(turn.text.clone()));
        }
        for (id, name, input) in &turn.calls {
            content.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
        }
        if content.is_empty() {
            content.push(ContentBlock::Text("(no reply)".into()));
        }
        messages.push(Message { role: Role::Assistant, content });

        if turn.calls.is_empty() {
            if turn.stop == Some(StopReason::MaxTokens) {
                messages.push(Message::user_text(
                    "Your reply hit the output limit. Continue, using tools for long content.",
                ));
                continue;
            }
            return Ok(Outcome { text: last_text, finished: false, steps: step, usage });
        }

        let mut results = Vec::new();
        let mut finished = false;
        for (id, name, input) in &turn.calls {
            say(LogKind::Tool, format!("{name} {}", arg_summary(name, input)));
            let (text, is_error) = if let Some(t) = spec.tools.iter_mut().find(|t| t.spec.name == *name) {
                let r = (t.call)(input);
                finished |= r.finish;
                (r.content, r.is_error)
            } else if let Some((ws, allowed)) = &spec.workspace
                && allowed.contains(&name.as_str())
            {
                let out = ws.execute(name, input, cancel.clone());
                (out.content, out.is_error)
            } else {
                (format!("Tool `{name}` is not available to this agent."), true)
            };
            if is_error {
                say(LogKind::Error, clip(&format!("{name}: {text}"), 300));
            }
            results.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: text,
                is_error,
            });
        }
        messages.push(Message { role: Role::User, content: results });
        if finished {
            return Ok(Outcome { text: last_text, finished: true, steps: step, usage });
        }
    }
    bail!("{} stopped after {} steps without finishing", spec.name, spec.max_steps)
}

struct Turn {
    text: String,
    calls: Vec<(String, String, Value)>,
    usage: Usage,
    stop: Option<StopReason>,
}

fn stream_with_retry(
    provider: &llm::ProviderConfig,
    request: &ChatRequest,
    cancel: &Arc<AtomicBool>,
    say: &dyn Fn(LogKind, String),
) -> anyhow::Result<Turn> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let mut turn = Turn { text: String::new(), calls: Vec::new(), usage: Usage::default(), stop: None };
        let res = llm::stream_chat(provider, request, cancel.clone(), &mut |ev| match ev {
            StreamEvent::TextDelta(t) => turn.text.push_str(&t),
            StreamEvent::ToolUse { id, name, input } => turn.calls.push((id, name, input)),
            StreamEvent::Usage(u) => turn.usage = u,
            StreamEvent::Stop(s) => turn.stop = Some(s),
            _ => {}
        });
        if cancel.load(Ordering::Relaxed) || turn.stop == Some(StopReason::Cancelled) {
            bail!("cancelled");
        }
        match res {
            Ok(()) => return Ok(turn),
            Err(e) if attempt < 3 => {
                say(LogKind::Error, format!("request failed ({e}); retrying"));
                std::thread::sleep(Duration::from_secs(3 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}

fn arg_summary(name: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let text = match name {
        "read_file" | "list_dir" | "edit_file" | "write_file" => s("path").to_string(),
        "grep" => s("pattern").to_string(),
        "bash" => s("command").lines().next().unwrap_or_default().to_string(),
        "delegate" => s("title").to_string(),
        "read_mockup" | "load_skill" => s("screen").to_string() + s("name"),
        _ => String::new(),
    };
    clip(&text, 160)
}

pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── Skills ──────────────────────────────────────────────────────────────

/// Folders searched for `<name>/SKILL.md`: `~/.claude/skills`, its synced sets
/// (`~/.claude/skills/synced/*`), `~/.config/harness/skills`.
fn skill_roots() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut roots = vec![home.join(".claude/skills")];
    if let Ok(entries) = std::fs::read_dir(home.join(".claude/skills/synced")) {
        roots.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    roots.push(home.join(".config/harness/skills"));
    roots
}

/// `(description, body)` of skill `name` (frontmatter stripped).
pub fn find_skill(name: &str) -> Option<(String, String)> {
    if name.contains('/') || name.contains("..") || name.is_empty() {
        return None;
    }
    let text = skill_roots()
        .into_iter()
        .find_map(|r| std::fs::read_to_string(r.join(name).join("SKILL.md")).ok())?;
    Some(split_frontmatter(&text))
}

fn split_frontmatter(text: &str) -> (String, String) {
    let Some(rest) = text.strip_prefix("---") else {
        return (String::new(), text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (String::new(), text.to_string());
    };
    let front = &rest[..end];
    let body = rest[end + 4..].trim_start_matches(['\r', '\n']).to_string();
    let desc = front
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().trim_matches('"').to_string())
        .unwrap_or_default();
    (desc, body)
}

/// System-prompt section with the full text of `names` (missing skills are noted).
pub fn forced_skills(names: &[String]) -> String {
    let mut out = String::new();
    for n in names {
        match find_skill(n) {
            Some((_, body)) => {
                out.push_str(&format!("\n\n<skill name=\"{n}\">\n{body}\n</skill>"))
            }
            None => out.push_str(&format!("\n\n(Skill `{n}` was not found on this machine.)")),
        }
    }
    out
}

/// `load_skill`: the full text of one of `names`.
pub fn load_skill_tool<'a>(names: Vec<String>) -> StageTool<'a> {
    let listing = names
        .iter()
        .map(|n| match find_skill(n) {
            Some((d, _)) => format!("- {n}: {d}"),
            None => format!("- {n}"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    StageTool {
        spec: ToolSpec {
            name: "load_skill".into(),
            description: format!(
                "Load the full instructions of a skill before working in its area. Available:\n{listing}"
            ),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        },
        call: Box::new(move |input| {
            let name = input.get("name").and_then(Value::as_str).unwrap_or_default();
            if !names.iter().any(|n| n == name) {
                return Reply::err(format!("Unknown skill `{name}`."));
            }
            match find_skill(name) {
                Some((_, body)) => Reply::ok(body),
                None => Reply::err(format!("Skill `{name}` is not installed.")),
            }
        }),
    }
}

/// Project instructions for agents (`AGENTS.md`, else `CLAUDE.md`) in `root`.
pub fn project_notes(root: &Path) -> String {
    ["AGENTS.md", "CLAUDE.md"]
        .iter()
        .find_map(|f| std::fs::read_to_string(root.join(f)).ok())
        .map(|t| format!("\n\nProject instructions ({}):\n{}", root.display(), clip(&t, 20_000)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter() {
        let (d, b) = split_frontmatter("---\nname: x\ndescription: \"Does y\"\n---\n\n# Body\n");
        assert_eq!(d, "Does y");
        assert_eq!(b, "# Body\n");
    }
}
