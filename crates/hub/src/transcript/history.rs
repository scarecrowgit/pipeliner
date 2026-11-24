//! Blocks from stored messages: the initial page, older pages, and messages committed live.
//!
//! Stored messages carry raw tool inputs, not the [`ToolDisplay`] the live stream sends, so tool
//! rows of history get a display derived from common input shapes (`command`, `path`, edits).

use crate::api::{
    Attachment, ContentBlock, DiffHunk, DiffLine, DiffLineKind, FileDiff, Role, Session, StoredMessage, TodoItem,
    TodoStatus, ToolDisplay,
};

use super::{
    BlockKind, CompactionBlock, Splice, SubagentBlock, SubagentState, TextBlock, ToolBlock, ToolState, Transcript,
    UserBlock,
};

/// How the transcript shows a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolRole {
    /// A todo list update: shown as the list.
    Todo,
    /// Starting a sub-agent: shown as a sub-agent row.
    Spawn,
    Plain,
}

pub(crate) fn tool_role(name: &str) -> ToolRole {
    match name {
        "todowrite" | "todo_write" | "update_todos" => ToolRole::Todo,
        "spawn_agent" | "task" | "subagent" => ToolRole::Spawn,
        _ => ToolRole::Plain,
    }
}

impl Transcript {
    /// Appends stored messages, oldest first. Messages already applied are skipped.
    pub(crate) fn append_history(&mut self, messages: &[StoredMessage]) {
        for message in messages {
            self.push_stored(message);
        }
    }

    /// Puts an older page in front of what is loaded.
    pub(crate) fn prepend_history(&mut self, messages: &[StoredMessage]) -> Splice {
        let mut older = Transcript { orphans: std::mem::take(&mut self.orphans), ..Transcript::default() };
        for message in messages {
            older.push_stored(message);
        }
        // A page ending in compacted messages above a loaded part that starts after the summary.
        if older.compacted_run > 0 && self.oldest_compacted == Some(false) {
            older.push(BlockKind::Compaction(CompactionBlock { summarised: older.compacted_run, running: false }));
        }
        older.dirty = None;
        self.prepend(older)
    }

    /// Applies one stored message as history.
    pub(crate) fn push_stored(&mut self, stored: &StoredMessage) {
        if self.newest_seq.is_some_and(|seq| stored.seq <= seq) {
            return;
        }
        self.newest_seq = Some(stored.seq);
        self.oldest_compacted.get_or_insert(stored.compacted);
        if stored.compacted {
            self.compacted_run += 1;
        } else if self.compacted_run > 0 {
            let summarised = std::mem::take(&mut self.compacted_run);
            self.push(BlockKind::Compaction(CompactionBlock { summarised, running: false }));
        }
        match stored.message.role {
            Role::User => self.push_user(stored),
            Role::Assistant => {
                for block in &stored.message.content {
                    match block {
                        ContentBlock::Thinking { text, .. } if !text.is_empty() => {
                            self.push(BlockKind::Thinking(text_block(stored, text.clone())));
                        }
                        ContentBlock::Text { text } if !text.is_empty() => self.push_assistant_text(stored, text),
                        ContentBlock::ToolUse { id, name, .. } => self.tool_use(id, name, block),
                        _ => {}
                    }
                }
            }
        }
    }

    fn push_user(&mut self, stored: &StoredMessage) {
        let mut text = String::new();
        let mut images = Vec::new();
        for block in &stored.message.content {
            match block {
                ContentBlock::Text { text: part } => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part);
                }
                ContentBlock::Image { blob, media_type, name, bytes } => images.push(Attachment {
                    blob: blob.clone(),
                    name: name.clone(),
                    media_type: media_type.clone(),
                    bytes: *bytes,
                }),
                ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                    self.tool_result(tool_use_id, content, *is_error);
                }
                _ => {}
            }
        }
        if !text.is_empty() || !images.is_empty() {
            self.push(BlockKind::User(UserBlock { seq: stored.seq, at: stored.at, text, images }));
        }
    }

    /// Text of one assistant message: consecutive text parts join into one block.
    fn push_assistant_text(&mut self, stored: &StoredMessage, text: &str) {
        let last = self.blocks.len().checked_sub(1);
        let same_message = last.is_some_and(|i| {
            matches!(&self.blocks[i].kind, BlockKind::Assistant(block) if block.seq == Some(stored.seq))
        });
        match (last, same_message) {
            (Some(index), true) => self.edit(index, |kind| {
                if let BlockKind::Assistant(block) = kind {
                    block.text.push_str("\n\n");
                    block.text.push_str(text);
                }
            }),
            _ => self.push(BlockKind::Assistant(text_block(stored, text.to_string()))),
        }
    }

    /// A tool call from a stored message, unless the live stream already added it.
    pub(crate) fn tool_use(&mut self, id: &str, name: &str, block: &ContentBlock) {
        match tool_role(name) {
            ToolRole::Todo => self.push(BlockKind::Todos(todo_items(block))),
            ToolRole::Spawn => {
                if self.rfind(|kind| matches!(kind, BlockKind::Subagent(s) if s.call.as_deref() == Some(id))).is_some() {
                    return;
                }
                self.push(BlockKind::Subagent(SubagentBlock {
                    session: None,
                    call: Some(id.to_string()),
                    name: field(block, &["agent", "name", "subagent_type"]).unwrap_or("agent").to_string(),
                    task: field(block, &["task", "description", "prompt"]).unwrap_or_default().to_string(),
                    depth: 1,
                    state: SubagentState::Running,
                }));
                if let Some((content, is_error)) = self.orphans.remove(id) {
                    self.tool_result(id, &content, is_error);
                }
            }
            ToolRole::Plain => {
                if self.tool_index(id).is_some() {
                    return;
                }
                self.push(BlockKind::Tool(ToolBlock {
                    id: id.to_string(),
                    name: name.to_string(),
                    class: None,
                    display: Some(display_for(name, block)),
                    state: ToolState::Ready,
                }));
                if let Some((content, is_error)) = self.orphans.remove(id) {
                    self.tool_result(id, &content, is_error);
                }
            }
        }
    }

    /// A stored tool result: finishes its call, or waits for the call to be loaded.
    fn tool_result(&mut self, id: &str, content: &str, is_error: bool) {
        if let Some(index) = self.tool_index(id) {
            self.edit(index, |kind| {
                if let BlockKind::Tool(tool) = kind
                    && !matches!(tool.state, ToolState::Finished { .. })
                {
                    tool.state = ToolState::Finished { is_error };
                    if let Some(display) = tool.display.as_mut() {
                        finish_display(display, content, is_error);
                    }
                }
            });
            return;
        }
        let spawn = self.rfind(|kind| matches!(kind, BlockKind::Subagent(s) if s.call.as_deref() == Some(id)));
        if let Some(index) = spawn {
            self.edit(index, |kind| {
                if let BlockKind::Subagent(sub) = kind
                    && sub.state == SubagentState::Running
                {
                    sub.state = SubagentState::Finished {
                        ok: !is_error,
                        summary: content.to_string(),
                        tool_calls: 0,
                        secs: 0,
                        cost_usd: 0.0,
                    };
                }
            });
            return;
        }
        self.orphans.insert(id.to_string(), (content.to_string(), is_error));
    }

    /// Gives sub-agent rows from history their sessions: the n-th row is the n-th child.
    pub(crate) fn link_subagents(&mut self, children: &[Session]) {
        let unlinked: Vec<usize> = (0..self.blocks.len())
            .filter(|&i| matches!(&self.blocks[i].kind, BlockKind::Subagent(s) if s.session.is_none()))
            .collect();
        for (index, child) in unlinked.into_iter().zip(children) {
            self.edit(index, |kind| {
                if let BlockKind::Subagent(sub) = kind {
                    sub.session = Some(child.id.clone());
                    sub.depth = child.depth.max(1);
                    if sub.task.is_empty() {
                        sub.task = child.title.clone();
                    }
                }
            });
        }
    }
}

fn text_block(stored: &StoredMessage, text: String) -> TextBlock {
    TextBlock { seq: Some(stored.seq), at: Some(stored.at), model: stored.model.clone(), text, streaming: false }
}

/// The first of `keys` that is a string field of a tool call's input.
fn field<'a>(block: &'a ContentBlock, keys: &[&str]) -> Option<&'a str> {
    let ContentBlock::ToolUse { input, .. } = block else { return None };
    keys.iter().find_map(|key| input.get(*key).and_then(|value| value.as_str()))
}

fn todo_items(block: &ContentBlock) -> Vec<TodoItem> {
    let ContentBlock::ToolUse { input, .. } = block else { return Vec::new() };
    let Some(items) = ["items", "todos"].iter().find_map(|key| input.get(*key).and_then(|v| v.as_array())) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = ["text", "content"].iter().find_map(|key| item.get(*key).and_then(|v| v.as_str()))?;
            let status = match item.get("status").and_then(|v| v.as_str()) {
                Some("in_progress") => TodoStatus::InProgress,
                Some("done" | "completed") => TodoStatus::Done,
                _ => TodoStatus::Pending,
            };
            Some(TodoItem { text: text.to_string(), status })
        })
        .collect()
}

/// A display for a stored tool call, from the shape of its input.
pub(crate) fn display_for(name: &str, block: &ContentBlock) -> ToolDisplay {
    if let Some(command) = field(block, &["command", "cmd"]) {
        return ToolDisplay::Command { command: command.to_string(), output: String::new(), exit_code: None };
    }
    let path = field(block, &["path", "file_path", "file"]);
    let old = field(block, &["old_string", "old"]);
    let new = field(block, &["new_string", "new", "content"]);
    if let (Some(path), Some(new)) = (path, new)
        && (name.contains("edit") || name.contains("write"))
    {
        return ToolDisplay::Diff(naive_diff(path, old.unwrap_or_default(), new));
    }
    if let Some(path) = path {
        return ToolDisplay::Line { verb: verb(name).to_string(), target: path.to_string(), result: String::new() };
    }
    if let Some(pattern) = field(block, &["pattern", "query"]) {
        return ToolDisplay::Line { verb: verb(name).to_string(), target: pattern.to_string(), result: String::new() };
    }
    let body = match block {
        ContentBlock::ToolUse { input, .. } => input.to_string(),
        _ => String::new(),
    };
    ToolDisplay::Text { title: name.to_string(), body }
}

fn verb(name: &str) -> &str {
    match name {
        "read_file" | "read" | "view" => "read",
        "list_dir" | "ls" => "list",
        "grep" | "search" | "rg" => "grep",
        "glob" | "find" => "glob",
        other => other,
    }
}

/// Every old line removed, every new line added: enough to draw an edit from history.
fn naive_diff(path: &str, old: &str, new: &str) -> FileDiff {
    let line = |kind, text: &str| DiffLine { kind, old_no: None, new_no: None, text: text.to_string(), no_newline: false };
    let mut lines: Vec<DiffLine> = old.lines().map(|text| line(DiffLineKind::Del, text)).collect();
    lines.extend(new.lines().map(|text| line(DiffLineKind::Add, text)));
    FileDiff {
        path: path.to_string(),
        added: new.lines().count() as u32,
        removed: old.lines().count() as u32,
        created: old.is_empty(),
        hunks: vec![DiffHunk { lines }],
        ..FileDiff::default()
    }
}

/// Fills a derived display with the tool's output.
fn finish_display(display: &mut ToolDisplay, content: &str, is_error: bool) {
    match display {
        ToolDisplay::Line { verb, result, .. } => {
            *result = if is_error {
                content.lines().next().unwrap_or("failed").to_string()
            } else {
                summarise(verb, content)
            };
        }
        ToolDisplay::Command { output, exit_code, .. } => {
            *output = content.to_string();
            *exit_code = Some(i32::from(is_error));
        }
        ToolDisplay::Text { body, .. } => {
            if body.is_empty() {
                *body = content.to_string();
            }
        }
        ToolDisplay::Diff(_) => {}
    }
}

fn summarise(verb: &str, content: &str) -> String {
    let lines = content.lines().filter(|line| !line.trim().is_empty()).count();
    match verb {
        "read" => format!("{} lines", content.lines().count()),
        "grep" | "glob" | "list" => match lines {
            1 => "1 match".to_string(),
            n => format!("{n} matches"),
        },
        _ => {
            let first = content.lines().next().unwrap_or_default();
            first.chars().take(60).collect()
        }
    }
}
