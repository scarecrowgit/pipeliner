//! The contract of this crate: what a tool is, and how tools are looked up.
//!
//! Signatures here are owned by the lead. Implementations live in the other modules.
//!
//! Tools in this crate work on a folder and hold no session state: files, search, shell.
//! Tools that need the running session (todo list, skills, sub-agents, reports) are defined by
//! the agent loop and registered next to these.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use base::cancel::CancelToken;
use model::{FileChange, ToolClass, ToolDisplay, ToolSpec};

/// Where and how a tool call runs.
pub struct ToolContext<'a> {
    /// The workdir. File tools never read or write outside it, following symlinks.
    pub root: &'a Path,
    /// Extra environment for processes the tool starts.
    pub env: &'a BTreeMap<String, String>,
    pub cancel: &'a CancelToken,
}

/// What a tool call produced.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    /// What the model reads as the tool result.
    pub content: String,
    pub is_error: bool,
    /// What a person sees in the transcript.
    pub display: ToolDisplay,
    /// Files the call created or changed, with line counts.
    pub files: Vec<FileChange>,
}

impl ToolOutput {
    /// A failed call: the same text for the model and for the person.
    pub fn error(title: &str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            content: message.clone(),
            is_error: true,
            display: ToolDisplay::Text { title: title.to_string(), body: message },
            files: Vec::new(),
        }
    }
}

pub trait Tool: Send + Sync {
    /// Name, description and JSON schema as sent to the model.
    fn spec(&self) -> ToolSpec;

    /// What the tool can do to the world; permission defaults are per class.
    fn class(&self) -> ToolClass;

    /// The text permission rules match against: the command for `bash`, the path for file
    /// tools. Empty when a call has nothing to match.
    fn subject(&self, input: &serde_json::Value) -> String;

    /// What the call would do, without doing it: the diff of an edit, the command of a shell
    /// call. Must not change anything and must not panic on malformed input.
    fn preview(&self, ctx: &ToolContext<'_>, input: &serde_json::Value) -> ToolDisplay;

    /// Runs the call. Errors the model can act on (file not found, text not unique, non-zero
    /// exit) come back as `ToolOutput { is_error: true, .. }`, never as a panic.
    fn run(&self, ctx: &ToolContext<'_>, input: &serde_json::Value) -> ToolOutput;
}

/// Tools by name. Cheap to clone; the agent loop builds one per session from the built-ins,
/// its own session tools and MCP tools.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool under the name in its spec, replacing one of the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.spec().name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn remove(&mut self, name: &str) {
        self.tools.remove(name);
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    /// Specs in name order, as sent to the model.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
