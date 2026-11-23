//! Conversations: messages as the model sees them, sessions as people see them, tool previews
//! and permission requests.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{BlobId, IssueKey, Millis, NodeId, RequestId, RunId, RunnableRef, SessionId};

// ---------------------------------------------------------------- messages

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// One piece of a message. Provider clients translate these to and from their wire formats.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// Model reasoning. `signature` is the provider's opaque token that must be sent back
    /// unchanged with the block.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// An attached image; the bytes live in the blob store.
    Image {
        blob: BlobId,
        media_type: String,
        name: String,
        bytes: u64,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, content: vec![ContentBlock::text(text)] }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: vec![ContentBlock::text(text)] }
    }

    /// All text blocks joined; what a preview or a search index wants.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let ContentBlock::Text { text } = block {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &serde_json::Value)> {
        self.content.iter().filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// A message as stored: its place in the session and what producing it cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    pub seq: u64,
    pub at: Millis,
    pub message: Message,
    /// Replaced by a summary: still shown above the divider, no longer sent to the model.
    #[serde(default)]
    pub compacted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub cost_usd: f64,
}

/// `provider/model`, split at the first slash: `nr/cc/claude-opus-5-5` is provider `nr`,
/// model `cc/claude-opus-5-5`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), model: model.into() }
    }

    pub fn parse(text: &str) -> Self {
        match text.split_once('/') {
            Some((provider, model)) => Self::new(provider, model),
            None => Self::new("", text),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.model.is_empty()
    }

    /// The model id without any gateway prefix: `cc/claude-opus-5-5` reads `claude-opus-5-5`.
    pub fn short(&self) -> &str {
        self.model.rsplit('/').next().unwrap_or(&self.model)
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.provider.is_empty() {
            f.write_str(&self.model)
        } else {
            write!(f, "{}/{}", self.provider, self.model)
        }
    }
}

impl From<ModelRef> for String {
    fn from(value: ModelRef) -> Self {
        value.to_string()
    }
}

impl From<String> for ModelRef {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

/// Token counts of one model call, or a sum of several.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// How much of the context window the request filled.
    pub fn context_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    pub fn total(&self) -> u64 {
        self.context_tokens() + self.output
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Other,
}

// ---------------------------------------------------------------- sessions

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Nothing is happening and nothing is owed.
    #[default]
    Idle,
    Running,
    /// Waiting for a person: a permission or a question.
    Waiting,
    Failed,
    /// The agent reported its work as finished.
    Done,
}

/// The two built-in ways to answer in chat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMode {
    /// Full access; writes and commands ask first by policy.
    #[default]
    Build,
    /// Read-only: proposes a plan, edits nothing.
    Plan,
}

/// Who started a session.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum SessionOrigin {
    #[default]
    Me,
    /// Another agent spawned it as a sub-agent.
    Agent { parent: SessionId },
    /// A pipeline node.
    Run { run: RunId, node: NodeId, variant: u32 },
    /// Brought over from the old app's session files.
    Imported,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<SessionId>,
    /// 0 for a top-level session, 1 for its sub-agents, and so on.
    #[serde(default)]
    pub depth: u32,
    /// Who answers in this thread.
    pub runnable: RunnableRef,
    #[serde(default)]
    pub mode: ChatMode,
    pub model: ModelRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<Workdir>,
    #[serde(default)]
    pub state: SessionState,
    #[serde(default)]
    pub origin: SessionOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueKey>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    pub created: Millis,
    pub updated: Millis,
    #[serde(default)]
    pub message_count: u32,
    #[serde(default)]
    pub cost_usd: f64,
    /// Sum over every model call of the session.
    #[serde(default)]
    pub usage: Usage,
    /// Context filled by the latest request, against `context_window`.
    #[serde(default)]
    pub context_tokens: u64,
    #[serde(default)]
    pub context_window: u64,
    /// Skills whose full text is in every request of the session.
    #[serde(default)]
    pub forced_skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkMode {
    /// Own branch and folder; other agents in the repo are not affected.
    #[default]
    Worktree,
    /// Edits land straight in the checkout.
    InPlace,
    /// Nothing is written.
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoHost {
    GitHub,
    GitLab,
    /// No remote, or one we do not recognise.
    #[default]
    Local,
}

/// Where a session or a pipeline node works.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workdir {
    pub path: PathBuf,
    /// Repo name as shown: `payments-api`.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub host: RepoHost,
    #[serde(default)]
    pub mode: WorkMode,
    /// The checkout a worktree was cut from; `None` when `path` is the checkout itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<PathBuf>,
    /// Runtime name; only the local machine exists for now.
    #[serde(default)]
    pub runtime: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub text: String,
    #[serde(default)]
    pub status: TodoStatus,
}

/// A file the session changed, with line counts against the state before the session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

// ---------------------------------------------------------------- tools

/// A tool as the model sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// What a tool can do to the world; permission rules are written against this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// Reads inside the workdir.
    Read,
    /// Changes files inside the workdir.
    Write,
    /// Runs a process.
    Execute,
    /// Leaves the machine or the harness: MCP tools, pull requests.
    External,
    /// Affects only the session itself: todo list, loading a skill, reporting.
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_no: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_no: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub lines: Vec<DiffLine>,
}

/// One file's change, ready to draw.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub added: u32,
    pub removed: u32,
    pub hunks: Vec<DiffHunk>,
    /// The file did not exist before.
    #[serde(default)]
    pub created: bool,
}

/// How a tool call is shown before it runs and after it finished.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolDisplay {
    /// `read src/charges/retry.ts` with a one-line result such as "58 lines".
    Line { verb: String, target: String, result: String },
    /// A shell command and, once it ran, its output.
    Command { command: String, output: String, exit_code: Option<i32> },
    /// An edit or a new file.
    Diff(FileDiff),
    /// Anything else: a title and a body of text.
    Text { title: String, body: String },
}

// ---------------------------------------------------------------- permissions

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RuleScope {
    Global,
    /// One repo, by its checkout path.
    Repo { path: PathBuf },
    Session { session: SessionId },
}

/// "Always allow `bun test`": a tool name plus a prefix its input must start with.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool: String,
    /// For `bash`, a command prefix; for file tools, a path prefix; empty matches every call.
    #[serde(default)]
    pub pattern: String,
    pub decision: RuleDecision,
    pub scope: RuleScope,
}

/// A tool call waiting for a person.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: RequestId,
    pub session: SessionId,
    /// Display name of who asks: "Build".
    pub agent: String,
    pub tool: String,
    pub class: ToolClass,
    pub display: ToolDisplay,
    /// Why it asks, in policy words: "bash = ask".
    pub policy: String,
    /// The rule "Always allow" would add, when one makes sense.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_rule: Option<PermissionRule>,
    pub at: Millis,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways { rule: PermissionRule },
    Reject { note: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_split_at_the_first_slash() {
        let m = ModelRef::parse("nr/cc/claude-opus-5-5");
        assert_eq!(m.provider, "nr");
        assert_eq!(m.model, "cc/claude-opus-5-5");
        assert_eq!(m.short(), "claude-opus-5-5");
        assert_eq!(m.to_string(), "nr/cc/claude-opus-5-5");
        assert_eq!(serde_json::to_string(&m).unwrap(), "\"nr/cc/claude-opus-5-5\"");
        assert_eq!(ModelRef::parse("bare").provider, "");
    }

    #[test]
    fn content_blocks_are_tagged_by_type() {
        let block = ContentBlock::ToolResult { tool_use_id: "t1".into(), content: "ok".into(), is_error: false };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(serde_json::from_value::<ContentBlock>(json).unwrap(), block);
    }

    #[test]
    fn usage_adds_up() {
        let mut u = Usage { input: 10, output: 5, cache_read: 100, cache_write: 20 };
        u.add(Usage { input: 1, output: 1, cache_read: 0, cache_write: 0 });
        assert_eq!(u.context_tokens(), 131);
        assert_eq!(u.total(), 137);
    }

    #[test]
    fn plain_text_skips_tool_blocks() {
        let m = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("first"),
                ContentBlock::ToolUse { id: "1".into(), name: "read_file".into(), input: serde_json::json!({}) },
                ContentBlock::text("second"),
            ],
        };
        assert_eq!(m.plain_text(), "first\nsecond");
        assert_eq!(m.tool_uses().count(), 1);
    }
}
