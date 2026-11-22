//! The transcript model, conversion to API history (and back, for restoring), and
//! JSON persistence under `$XDG_DATA_HOME/harness/sessions/<id>.json`.

use std::path::{Path, PathBuf};

use agent_tools::ToolDisplay;
use llm::{ContentBlock, Message, Role, StopReason, StreamEvent, Usage};
use serde_json::{Value, json};

use crate::data::Mode;
use crate::markdown;

// ── Transcript items ────────────────────────────────────────────────────

pub enum Item {
    User(UserMsg),
    /// `!command` run by the user; its output goes into the history as context.
    Shell(ShellRun),
    /// One model response (one `stream_chat` call) and its tool calls.
    Turn(Turn),
    Error(String),
    Notice(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub path: String,
    /// `None` until read (or when reading failed).
    pub content: Option<String>,
    pub lines: usize,
    pub error: Option<String>,
}

pub struct UserMsg {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

pub struct ShellRun {
    pub command: String,
    /// `None` while running.
    pub output: Option<ToolResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    /// `None` for results restored from disk.
    pub display: Option<ToolDisplay>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnState {
    Streaming,
    Done,
    Interrupted,
    Failed,
}

pub struct Turn {
    pub model: String,
    pub mode: Mode,
    /// Custom agent (from `.claude/agents`) that produced this turn, if any.
    pub agent: Option<String>,
    pub blocks: Vec<TurnBlock>,
    pub thinking: String,
    pub thinking_open: bool,
    pub state: TurnState,
    pub usage: Usage,
    pub stop: Option<StopReason>,
}

// A turn holds a handful of blocks; boxing ToolCall would only add indirection.
#[allow(clippy::large_enum_variant)]
pub enum TurnBlock {
    Text(TextBlock),
    Tool(ToolCall),
}

pub struct TextBlock {
    pub text: String,
    pub md: Vec<markdown::Block>,
    parsed_len: usize,
}

impl TextBlock {
    pub fn new(text: String) -> Self {
        let md = markdown::parse(&text);
        let parsed_len = text.len();
        Self {
            text,
            md,
            parsed_len,
        }
    }

    fn reparse(&mut self) {
        if self.parsed_len != self.text.len() {
            self.md = markdown::parse(&self.text);
            self.parsed_len = self.text.len();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolStatus {
    /// Arguments still streaming.
    Streaming,
    Queued,
    /// Waiting for the user's permission.
    Asking,
    Running,
    Done(ToolResult),
    Rejected(String),
    Cancelled,
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Option<Value>,
    /// Bytes of argument JSON received while streaming.
    pub partial: usize,
    pub status: ToolStatus,
    /// What the call would do, computed for the permission prompt.
    pub preview: Option<ToolDisplay>,
    /// Diff / output expanded.
    pub open: bool,
}

pub const REJECTED: &str = "User rejected this action";
pub const INTERRUPTED: &str = "Interrupted by the user before the tool finished";

impl ToolCall {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            input: None,
            partial: 0,
            status: ToolStatus::Streaming,
            preview: None,
            open: true,
        }
    }

    pub fn result(&self) -> Option<&ToolResult> {
        match &self.status {
            ToolStatus::Done(r) => Some(r),
            _ => None,
        }
    }

    /// The `tool_result` sent back for this call.
    pub fn result_block(&self) -> ContentBlock {
        let (content, is_error) = match &self.status {
            ToolStatus::Done(r) => (r.content.clone(), r.is_error),
            ToolStatus::Rejected(note) if note.trim().is_empty() => (REJECTED.to_string(), true),
            ToolStatus::Rejected(note) => (format!("{REJECTED}. Note: {}", note.trim()), true),
            _ => (INTERRUPTED.to_string(), true),
        };
        ContentBlock::ToolResult {
            tool_use_id: self.id.clone(),
            content,
            is_error,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(
            self.status,
            ToolStatus::Done(_) | ToolStatus::Rejected(_) | ToolStatus::Cancelled
        )
    }
}

impl Turn {
    pub fn new(model: String, mode: Mode) -> Self {
        Self {
            model,
            mode,
            agent: None,
            blocks: Vec::new(),
            thinking: String::new(),
            thinking_open: false,
            state: TurnState::Streaming,
            usage: Usage::default(),
            stop: None,
        }
    }

    pub fn tools(&self) -> impl Iterator<Item = &ToolCall> {
        self.blocks.iter().filter_map(|b| match b {
            TurnBlock::Tool(t) => Some(t),
            _ => None,
        })
    }

    pub fn tools_mut(&mut self) -> impl Iterator<Item = &mut ToolCall> {
        self.blocks.iter_mut().filter_map(|b| match b {
            TurnBlock::Tool(t) => Some(t),
            _ => None,
        })
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCall> {
        self.tools_mut().find(|t| t.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.thinking.is_empty()
            && self.blocks.iter().all(|b| match b {
                TurnBlock::Text(t) => t.text.trim().is_empty(),
                TurnBlock::Tool(_) => false,
            })
    }

    /// Apply one stream event. Markdown is re-parsed by [`Turn::reparse`] once per batch.
    pub fn apply(&mut self, ev: StreamEvent) {
        match ev {
            StreamEvent::TextDelta(t) => match self.blocks.last_mut() {
                Some(TurnBlock::Text(b)) => b.text.push_str(&t),
                _ => self.blocks.push(TurnBlock::Text(TextBlock {
                    text: t,
                    md: Vec::new(),
                    parsed_len: usize::MAX,
                })),
            },
            StreamEvent::ThinkingDelta(t) => self.thinking.push_str(&t),
            StreamEvent::ToolUseStart { id, name } => {
                if self.tool_mut(&id).is_none() {
                    self.blocks.push(TurnBlock::Tool(ToolCall::new(id, name)));
                }
            }
            StreamEvent::ToolUseInputDelta { id, partial_json } => {
                if let Some(t) = self.tool_mut(&id) {
                    t.partial += partial_json.len();
                }
            }
            StreamEvent::ToolUse { id, name, input } => {
                if self.tool_mut(&id).is_none() {
                    self.blocks
                        .push(TurnBlock::Tool(ToolCall::new(id.clone(), name)));
                }
                if let Some(t) = self.tool_mut(&id) {
                    t.input = Some(input);
                    t.status = ToolStatus::Queued;
                }
            }
            StreamEvent::Usage(u) => self.usage = u,
            StreamEvent::Stop(r) => self.stop = Some(r),
        }
    }

    pub fn reparse(&mut self) {
        for b in &mut self.blocks {
            if let TurnBlock::Text(t) = b {
                t.reparse();
            }
        }
    }

    /// Stop everything in flight (Esc): partial text stays.
    pub fn interrupt(&mut self) {
        self.state = TurnState::Interrupted;
        for t in self.tools_mut() {
            if !t.finished() {
                t.status = ToolStatus::Cancelled;
            }
        }
    }
}

// ── Context encoding (user message blocks) ──────────────────────────────

const FILE_OPEN: &str = "<file path=\"";
const SHELL_OPEN: &str = "<shell_output command=\"";

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

fn unescape_attr(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&amp;", "&")
}

pub fn encode_file(path: &str, content: &str) -> String {
    format!("{FILE_OPEN}{}\">\n{content}\n</file>", escape_attr(path))
}

/// `(path, content)` from an [`encode_file`] block.
pub fn decode_file(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix(FILE_OPEN)?;
    let (path, body) = rest.split_once("\">\n")?;
    let body = body.strip_suffix("\n</file>")?;
    Some((unescape_attr(path), body.to_string()))
}

pub fn encode_shell(command: &str, output: &str, is_error: bool) -> String {
    format!(
        "{SHELL_OPEN}{}\" failed=\"{is_error}\">\nThe user ran this command in the workspace shell; the output is context for the next request.\n{output}\n</shell_output>",
        escape_attr(command)
    )
}

const SHELL_PREFACE: &str = "The user ran this command in the workspace shell; the output is context for the next request.\n";

/// `(command, output, is_error)` from an [`encode_shell`] block.
pub fn decode_shell(s: &str) -> Option<(String, String, bool)> {
    let rest = s.strip_prefix(SHELL_OPEN)?;
    let (cmd, rest) = rest.split_once("\" failed=\"")?;
    let (failed, body) = rest.split_once("\">\n")?;
    let body = body.strip_suffix("\n</shell_output>")?;
    let body = body.strip_prefix(SHELL_PREFACE).unwrap_or(body);
    Some((unescape_attr(cmd), body.to_string(), failed == "true"))
}

// ── History ─────────────────────────────────────────────────────────────

fn push(out: &mut Vec<Message>, role: Role, blocks: Vec<ContentBlock>) {
    if blocks.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.role == role => last.content.extend(blocks),
        _ => out.push(Message {
            role,
            content: blocks,
        }),
    }
}

/// API history from the transcript. Failed / still-streaming turns are skipped, every
/// complete tool call gets a result (rejected / interrupted ones an error), calls whose
/// arguments never finished streaming are dropped, and adjacent same-role messages merge.
pub fn build_history(items: &[Item]) -> Vec<Message> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Item::User(u) => {
                let mut blocks = vec![ContentBlock::Text(u.text.clone())];
                for a in &u.attachments {
                    if let Some(c) = &a.content {
                        blocks.push(ContentBlock::Text(encode_file(&a.path, c)));
                    }
                }
                push(&mut out, Role::User, blocks);
            }
            Item::Shell(s) => {
                if let Some(r) = &s.output {
                    let text = encode_shell(&s.command, &r.content, r.is_error);
                    push(&mut out, Role::User, vec![ContentBlock::Text(text)]);
                }
            }
            Item::Turn(t) => {
                if matches!(t.state, TurnState::Failed | TurnState::Streaming) {
                    continue;
                }
                let mut assistant = Vec::new();
                let mut results = Vec::new();
                for b in &t.blocks {
                    match b {
                        TurnBlock::Text(tb) if !tb.text.trim().is_empty() => {
                            assistant.push(ContentBlock::Text(tb.text.clone()))
                        }
                        TurnBlock::Text(_) => {}
                        TurnBlock::Tool(tc) => {
                            if let Some(input) = &tc.input {
                                assistant.push(ContentBlock::ToolUse {
                                    id: tc.id.clone(),
                                    name: tc.name.clone(),
                                    input: input.clone(),
                                });
                                results.push(tc.result_block());
                            }
                        }
                    }
                }
                push(&mut out, Role::Assistant, assistant);
                push(&mut out, Role::User, results);
            }
            Item::Error(_) | Item::Notice(_) => {}
        }
    }
    out
}

/// Rebuild a transcript from saved history (tool displays are not persisted).
pub fn items_from_history(messages: &[Message], model: &str) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for m in messages {
        match m.role {
            Role::Assistant => {
                let mut turn = Turn::new(model.to_string(), Mode::Build);
                turn.state = TurnState::Done;
                for b in &m.content {
                    match b {
                        ContentBlock::Text(t) => {
                            turn.blocks.push(TurnBlock::Text(TextBlock::new(t.clone())))
                        }
                        ContentBlock::ToolUse { id, name, input } => {
                            let mut tc = ToolCall::new(id.clone(), name.clone());
                            tc.input = Some(input.clone());
                            tc.status = ToolStatus::Cancelled;
                            tc.open = false;
                            turn.blocks.push(TurnBlock::Tool(tc));
                        }
                        ContentBlock::ToolResult { .. } => {}
                    }
                }
                items.push(Item::Turn(turn));
            }
            Role::User => {
                let mut current_user = false;
                for b in &m.content {
                    match b {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            let tool = items.iter_mut().rev().find_map(|i| match i {
                                Item::Turn(t) => t.tool_mut(tool_use_id),
                                _ => None,
                            });
                            if let Some(tc) = tool {
                                tc.status = if *is_error && content.starts_with(REJECTED) {
                                    let note = content[REJECTED.len()..]
                                        .trim_start_matches(". Note: ")
                                        .to_string();
                                    ToolStatus::Rejected(note)
                                } else if *is_error && content == INTERRUPTED {
                                    ToolStatus::Cancelled
                                } else {
                                    ToolStatus::Done(ToolResult {
                                        content: content.clone(),
                                        is_error: *is_error,
                                        display: None,
                                    })
                                };
                            }
                            current_user = false;
                        }
                        ContentBlock::Text(t) => {
                            if let Some((command, output, is_error)) = decode_shell(t) {
                                items.push(Item::Shell(ShellRun {
                                    command,
                                    output: Some(ToolResult {
                                        content: output,
                                        is_error,
                                        display: None,
                                    }),
                                }));
                                current_user = false;
                            } else if let Some((path, content)) = decode_file(t)
                                && current_user
                                && let Some(Item::User(u)) = items.last_mut()
                            {
                                u.attachments.push(Attachment {
                                    path,
                                    lines: content.lines().count(),
                                    content: Some(content),
                                    error: None,
                                });
                            } else {
                                items.push(Item::User(UserMsg {
                                    text: t.clone(),
                                    attachments: Vec::new(),
                                }));
                                current_user = true;
                            }
                        }
                        ContentBlock::ToolUse { .. } => {}
                    }
                }
            }
        }
    }
    items
}

/// Header title: the first user message, first line, truncated.
pub fn title(items: &[Item]) -> Option<String> {
    items.iter().find_map(|i| match i {
        Item::User(u) => Some(truncate(u.text.lines().next().unwrap_or("").trim(), 60)),
        _ => None,
    })
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

/// `@path` tokens in a message (trailing punctuation stripped, deduped, in order).
pub fn mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(path) = word.strip_prefix('@') else {
            continue;
        };
        let path = path.trim_end_matches([',', '.', ';', ':', ')', '!', '?', '"', '\'']);
        if !path.is_empty() && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

// ── Persistence ─────────────────────────────────────────────────────────

pub struct Saved {
    pub id: String,
    pub model: Option<String>,
    pub messages: Vec<Message>,
    pub cost_usd: f64,
    /// Usage of the last request and its model id (the panel's Context meter).
    pub last_usage: Option<(Usage, String)>,
    pub usage: Usage,
    /// Deferred MCP tools loaded with `tool_search` (stay loaded when reopened).
    pub loaded_tools: Vec<String>,
    /// Build / Plan (a custom agent runs as Build).
    pub mode: Mode,
    /// Selected custom agent (by name), if any.
    pub agent: Option<String>,
    /// Skills forced into the system prompt for this session.
    pub forced_skills: Vec<String>,
}

pub fn sessions_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        });
    base.join("harness").join("sessions")
}

/// Time-sortable id, e.g. `1763560000123-4821`.
pub fn new_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:04}", now.as_millis(), now.subsec_nanos() % 10_000)
}

pub fn to_json(s: &Saved, title: Option<&str>) -> Value {
    json!({
        "id": s.id,
        "title": title,
        "model": s.model,
        "cost_usd": s.cost_usd,
        "last_usage": s.last_usage.as_ref().map(|(u, model)| json!({
            "model": model,
            "input_tokens": u.input_tokens,
            "output_tokens": u.output_tokens,
            "cache_read_tokens": u.cache_read_tokens,
            "cache_write_tokens": u.cache_write_tokens,
        })),
        "usage": {
            "input_tokens": s.usage.input_tokens,
            "output_tokens": s.usage.output_tokens,
            "cache_read_tokens": s.usage.cache_read_tokens,
            "cache_write_tokens": s.usage.cache_write_tokens,
        },
        "messages": s.messages,
        "loaded_tools": s.loaded_tools,
        "mode": match s.mode {
            Mode::Build => "build",
            Mode::Plan => "plan",
        },
        "agent": s.agent,
        "forced_skills": s.forced_skills,
    })
}

pub fn from_json(mut v: Value) -> Option<Saved> {
    let messages: Vec<Message> = serde_json::from_value(v.get_mut("messages")?.take()).ok()?;
    let u = |k: &str| v["usage"][k].as_u64().unwrap_or(0);
    Some(Saved {
        id: v["id"].as_str()?.to_string(),
        model: v["model"].as_str().map(str::to_string),
        cost_usd: v["cost_usd"].as_f64().unwrap_or(0.),
        last_usage: v["last_usage"]["model"].as_str().map(|model| {
            let n = |k: &str| v["last_usage"][k].as_u64().unwrap_or(0);
            (
                Usage {
                    input_tokens: n("input_tokens"),
                    output_tokens: n("output_tokens"),
                    cache_read_tokens: n("cache_read_tokens"),
                    cache_write_tokens: n("cache_write_tokens"),
                },
                model.to_string(),
            )
        }),
        usage: Usage {
            input_tokens: u("input_tokens"),
            output_tokens: u("output_tokens"),
            cache_read_tokens: u("cache_read_tokens"),
            cache_write_tokens: u("cache_write_tokens"),
        },
        messages,
        loaded_tools: strings(&v["loaded_tools"]),
        mode: match v["mode"].as_str() {
            Some("plan") => Mode::Plan,
            _ => Mode::Build,
        },
        agent: v["agent"]
            .as_str()
            .filter(|a| !a.is_empty())
            .map(str::to_string),
        forced_skills: strings(&v["forced_skills"]),
    })
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn save(dir: &Path, saved: &Saved, title: Option<&str>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", saved.id));
    let tmp = dir.join(format!(".{}.json.tmp", saved.id));
    let data = serde_json::to_vec_pretty(&to_json(saved, title))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, path)
}

/// A saved session as listed in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexEntry {
    pub id: String,
    pub title: String,
}

/// Saved sessions in `dir`, newest first (by file mtime), at most `limit`.
pub fn list(dir: &Path, limit: usize) -> Vec<IndexEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".json") && !name.starts_with('.')
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .take(limit)
        .filter_map(|(_, path)| {
            let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            Some(IndexEntry {
                id: v["id"].as_str()?.to_string(),
                title: v["title"]
                    .as_str()
                    .unwrap_or("Untitled session")
                    .to_string(),
            })
        })
        .collect()
}

/// Load one saved session by id.
pub fn load(dir: &Path, id: &str) -> Option<Saved> {
    if id.contains('/') || id.contains("..") {
        return None;
    }
    let v: Value =
        serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).ok()?).ok()?;
    from_json(v)
}

/// The most recently written session in `dir`.
pub fn load_latest(dir: &Path) -> Option<Saved> {
    let latest = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".json") && !name.starts_with('.')
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)?
        .1;
    let v: Value = serde_json::from_slice(&std::fs::read(latest).ok()?).ok()?;
    from_json(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Item {
        Item::User(UserMsg {
            text: text.into(),
            attachments: Vec::new(),
        })
    }

    fn turn_with(events: Vec<StreamEvent>) -> Turn {
        let mut t = Turn::new("p/m".into(), Mode::Build);
        for e in events {
            t.apply(e);
        }
        t.reparse();
        t.state = TurnState::Done;
        t
    }

    fn tool_use(id: &str, name: &str) -> StreamEvent {
        StreamEvent::ToolUse {
            id: id.into(),
            name: name.into(),
            input: json!({"path": "a.rs"}),
        }
    }

    #[test]
    fn stream_events_build_blocks() {
        let t = turn_with(vec![
            StreamEvent::TextDelta("Hel".into()),
            StreamEvent::TextDelta("lo **x**".into()),
            StreamEvent::ToolUseStart {
                id: "1".into(),
                name: "read_file".into(),
            },
            StreamEvent::ToolUseInputDelta {
                id: "1".into(),
                partial_json: "{\"pa".into(),
            },
            tool_use("1", "read_file"),
            StreamEvent::ToolUseStart {
                id: "2".into(),
                name: "grep".into(),
            },
            StreamEvent::Usage(Usage {
                input_tokens: 5,
                ..Default::default()
            }),
            StreamEvent::Stop(StopReason::ToolUse),
        ]);
        assert_eq!(t.blocks.len(), 3);
        let TurnBlock::Text(tb) = &t.blocks[0] else {
            panic!()
        };
        assert_eq!(tb.text, "Hello **x**");
        assert_eq!(tb.md.len(), 1);
        let tools: Vec<_> = t.tools().collect();
        assert_eq!(tools[0].status, ToolStatus::Queued);
        assert_eq!(tools[0].partial, 4);
        assert_eq!(tools[1].status, ToolStatus::Streaming);
        assert!(tools[1].input.is_none());
        assert_eq!(t.usage.input_tokens, 5);
        assert_eq!(t.stop, Some(StopReason::ToolUse));
    }

    #[test]
    fn history_pairs_tool_calls_and_merges_roles() {
        let mut t = turn_with(vec![
            StreamEvent::TextDelta("Reading.".into()),
            tool_use("1", "read_file"),
            tool_use("2", "edit_file"),
            tool_use("3", "bash"),
            StreamEvent::ToolUseStart {
                id: "4".into(),
                name: "write_file".into(),
            },
        ]);
        let mut tools = t.tools_mut();
        tools.next().unwrap().status = ToolStatus::Done(ToolResult {
            content: "1: fn x".into(),
            is_error: false,
            display: None,
        });
        tools.next().unwrap().status = ToolStatus::Rejected("use a test".into());
        drop(tools);
        t.interrupt(); // bash (queued) and write (streaming) are cancelled

        let failed = {
            let mut f = turn_with(vec![StreamEvent::TextDelta("partial".into())]);
            f.state = TurnState::Failed;
            f
        };
        let items = vec![
            user("fix it"),
            Item::Turn(t),
            Item::Error("boom".into()),
            Item::Turn(failed),
            Item::Shell(ShellRun {
                command: "ls".into(),
                output: Some(ToolResult {
                    content: "a.rs".into(),
                    is_error: false,
                    display: None,
                }),
            }),
            user("again"),
        ];
        let h = build_history(&items);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0], Message::user_text("fix it"));
        assert_eq!(h[1].role, Role::Assistant);
        assert_eq!(h[1].content.len(), 4); // text + 3 complete tool uses; #4 dropped
        assert_eq!(h[2].role, Role::User);
        // 3 results, then shell context and the next user text merged in.
        assert_eq!(h[2].content.len(), 5);
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = &h[2].content[1]
        else {
            panic!()
        };
        assert!(is_error);
        assert_eq!(content, "User rejected this action. Note: use a test");
        let ContentBlock::ToolResult { content, .. } = &h[2].content[2] else {
            panic!()
        };
        assert_eq!(content, INTERRUPTED);
        let ContentBlock::Text(shell) = &h[2].content[3] else {
            panic!()
        };
        assert_eq!(
            decode_shell(shell),
            Some(("ls".into(), "a.rs".into(), false))
        );
        assert_eq!(h[2].content[4], ContentBlock::Text("again".into()));
    }

    #[test]
    fn attachments_become_file_blocks() {
        let items = vec![Item::User(UserMsg {
            text: "look at @src/a.rs".into(),
            attachments: vec![
                Attachment {
                    path: "src/a.rs".into(),
                    content: Some("1: fn a() {}".into()),
                    lines: 1,
                    error: None,
                },
                Attachment {
                    path: "missing.rs".into(),
                    content: None,
                    lines: 0,
                    error: Some("not found".into()),
                },
            ],
        })];
        let h = build_history(&items);
        assert_eq!(h[0].content.len(), 2);
        let ContentBlock::Text(f) = &h[0].content[1] else {
            panic!()
        };
        assert_eq!(
            decode_file(f),
            Some(("src/a.rs".into(), "1: fn a() {}".into()))
        );
    }

    #[test]
    fn restore_round_trips() {
        let mut t = turn_with(vec![
            StreamEvent::TextDelta("ok".into()),
            tool_use("1", "read_file"),
            tool_use("2", "bash"),
        ]);
        let mut tools = t.tools_mut();
        tools.next().unwrap().status = ToolStatus::Done(ToolResult {
            content: "x".into(),
            is_error: false,
            display: None,
        });
        tools.next().unwrap().status = ToolStatus::Rejected(String::new());
        drop(tools);
        let items = vec![
            Item::User(UserMsg {
                text: "hi \"there\"".into(),
                attachments: vec![Attachment {
                    path: "a\"b.rs".into(),
                    content: Some("line".into()),
                    lines: 1,
                    error: None,
                }],
            }),
            Item::Turn(t),
            Item::Shell(ShellRun {
                command: "echo \"x\" && ls".into(),
                output: Some(ToolResult {
                    content: "x\n".into(),
                    is_error: true,
                    display: None,
                }),
            }),
            user("next"),
        ];
        let h = build_history(&items);
        let restored = items_from_history(&h, "p/m");
        assert_eq!(build_history(&restored), h);
        assert_eq!(title(&restored).as_deref(), Some("hi \"there\""));
    }

    #[test]
    fn mcp_and_skill_calls_round_trip() {
        let mut t = turn_with(vec![
            StreamEvent::ToolUse {
                id: "s1".into(),
                name: "skill".into(),
                input: json!({"name": "gpui-elements"}),
            },
            StreamEvent::ToolUse {
                id: "m1".into(),
                name: "mcp__gh__search".into(),
                input: json!({"q": "bug"}),
            },
            StreamEvent::ToolUse {
                id: "m2".into(),
                name: "mcp__gh__close".into(),
                input: json!({"n": 1}),
            },
        ]);
        let mut tools = t.tools_mut();
        tools.next().unwrap().status = ToolStatus::Done(ToolResult {
            content: "Base directory for this skill: /x\n\n# GPUI".into(),
            is_error: false,
            display: None,
        });
        tools.next().unwrap().status = ToolStatus::Done(ToolResult {
            content: "MCP server `gh` stopped responding".into(),
            is_error: true,
            display: None,
        });
        tools.next().unwrap().status = ToolStatus::Rejected("no".into());
        drop(tools);
        let items = vec![user("find bugs"), Item::Turn(t)];
        let h = build_history(&items);
        let saved = Saved {
            id: "x".into(),
            model: Some("p/m".into()),
            messages: h.clone(),
            cost_usd: 0.,
            last_usage: None,
            usage: Usage::default(),
            loaded_tools: Vec::new(),
            mode: Mode::Build,
            agent: None,
            forced_skills: Vec::new(),
        };
        let back = from_json(to_json(&saved, None)).unwrap();
        let restored = items_from_history(&back.messages, "p/m");
        assert_eq!(build_history(&restored), h);
        let Item::Turn(rt) = &restored[1] else {
            panic!()
        };
        let calls: Vec<_> = rt.tools().collect();
        assert_eq!(calls[0].name, "skill");
        assert!(calls[0].result().is_some_and(|r| !r.is_error));
        assert!(calls[1].result().is_some_and(|r| r.is_error));
        assert_eq!(calls[2].status, ToolStatus::Rejected("no".into()));
    }

    #[test]
    fn json_round_trip() {
        let saved = Saved {
            id: "1-2".into(),
            model: Some("a/b".into()),
            messages: vec![Message::user_text("x"), Message::assistant_text("y")],
            cost_usd: 0.5,
            last_usage: Some((Usage { input_tokens: 7, ..Default::default() }, "m".into())),
            usage: Usage {
                input_tokens: 3,
                output_tokens: 4,
                ..Default::default()
            },
            loaded_tools: vec!["mcp__g__a".into()],
            mode: Mode::Plan,
            agent: Some("ext-reviewer".into()),
            forced_skills: vec!["gpui-elements".into()],
        };
        let back = from_json(to_json(&saved, Some("x"))).unwrap();
        assert_eq!(back.id, "1-2");
        assert_eq!(back.mode, Mode::Plan);
        assert_eq!(back.agent.as_deref(), Some("ext-reviewer"));
        assert_eq!(back.forced_skills, ["gpui-elements"]);
        assert_eq!(back.last_usage.as_ref().map(|(u, m)| (u.input_tokens, m.as_str())), Some((7, "m")));
        // Files written before these fields existed still load.
        let mut old = to_json(&saved, None);
        for k in ["mode", "agent", "forced_skills"] {
            old.as_object_mut().unwrap().remove(k);
        }
        let old = from_json(old).unwrap();
        assert_eq!(old.mode, Mode::Build);
        assert_eq!(old.agent, None);
        assert!(old.forced_skills.is_empty());
        assert_eq!(back.loaded_tools, ["mcp__g__a"]);
        assert_eq!(back.messages, saved.messages);
        assert_eq!(back.usage, saved.usage);
        assert_eq!(back.model.as_deref(), Some("a/b"));
    }

    #[test]
    fn save_and_load_latest() {
        let dir = std::env::temp_dir().join(format!("screen_chat_test_{}", new_id()));
        let mk = |id: &str, text: &str| Saved {
            id: id.into(),
            model: None,
            messages: vec![Message::user_text(text)],
            cost_usd: 0.,
            last_usage: None,
            usage: Usage::default(),
            loaded_tools: Vec::new(),
            mode: Mode::Build,
            agent: None,
            forced_skills: Vec::new(),
        };
        save(&dir, &mk("a", "first"), None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        save(&dir, &mk("b", "second"), None).unwrap();
        let latest = load_latest(&dir).unwrap();
        assert_eq!(latest.id, "b");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mentions_and_titles() {
        assert_eq!(
            mentions("see @src/a.rs, and @b.md. also @src/a.rs @"),
            vec!["src/a.rs".to_string(), "b.md".to_string()]
        );
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(title(&[]), None);
    }
}
