//! The tools that act on the session itself: their specs, how their input is read and what
//! the model is told back. They hold no state; the loop applies what they parse.

use std::borrow::Cow;

use model::ToolDisplay;
use serde_json::{Map, Value};
use tools::ToolOutput;

pub(crate) mod ask;
pub(crate) mod finish;
pub(crate) mod report;
pub(crate) mod search;
pub(crate) mod skill;
pub(crate) mod spawn;
pub(crate) mod todo;

/// Names of the session tools, the one place they are spelled out.
pub mod names {
    pub const TODO_WRITE: &str = "todo_write";
    pub const SKILL: &str = "skill";
    pub const SPAWN_AGENT: &str = "spawn_agent";
    pub const TOOL_SEARCH: &str = "tool_search";
    pub const REPORT_TO_INBOX: &str = "report_to_inbox";
    pub const ASK_USER: &str = "ask_user";
}

/// Which session tool a name stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionTool {
    Todo,
    Skill,
    Spawn,
    ToolSearch,
    Report,
    AskUser,
    /// The finish tool, under the name the spec gives it.
    Finish,
}

/// A session tool call that worked: `content` for the model and for the transcript.
pub(crate) fn ok(title: &str, content: impl Into<String>) -> ToolOutput {
    let content = content.into();
    ToolOutput { display: text_display(title, &content), content, is_error: false, files: Vec::new() }
}

pub(crate) fn text_display(title: &str, body: &str) -> ToolDisplay {
    ToolDisplay::Text { title: title.to_string(), body: body.to_string() }
}

/// The input as a JSON object; a string holding one is accepted (some gateways send the
/// arguments encoded).
pub(crate) fn object<'v>(tool: &str, input: &'v Value) -> Result<Cow<'v, Map<String, Value>>, String> {
    match input {
        Value::Object(map) => Ok(Cow::Borrowed(map)),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => Ok(Cow::Owned(map)),
            _ => Err(format!("{tool}: the input must be a JSON object.")),
        },
        Value::Null => Ok(Cow::Owned(Map::new())),
        _ => Err(format!("{tool}: the input must be a JSON object.")),
    }
}

/// A required, non-blank string field, trimmed.
pub(crate) fn required_str<'m>(tool: &str, map: &'m Map<String, Value>, key: &str) -> Result<&'m str, String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{tool}: `{key}` is required and must be a non-empty string."))
}

/// An optional string field, trimmed; blank counts as missing.
pub(crate) fn optional_str<'m>(map: &'m Map<String, Value>, key: &str) -> Option<&'m str> {
    map.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// The input as indented JSON, for showing a call nobody wrote a preview for.
pub(crate) fn pretty(input: &Value) -> String {
    serde_json::to_string_pretty(input).unwrap_or_else(|_| input.to_string())
}
