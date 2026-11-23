//! `edit_file` and `write_file`. Both plan the change first (the preview shows that plan as a
//! diff) and write only in `run`.

use std::io::ErrorKind;

use base::fmt::plural;
use model::{FileChange, FileDiff, ToolClass, ToolDisplay, ToolSpec};
use serde_json::{Value, json};

use crate::api::{Tool, ToolContext, ToolOutput};
use crate::args::{Args, peek_str};
use crate::builtin::names::{EDIT_FILE, WRITE_FILE};
use crate::diff::file_diff;
use crate::sandbox::{Resolved, normalize_subject, resolve};
use crate::text::{is_binary, preview_error};

/// Files larger than this are not edited or diffed.
const EDIT_MAX_BYTES: u64 = 10 * 1024 * 1024;

const EDIT_USAGE: &str = r#"{"path": "src/lib.rs", "old_string": "let x = 1;", "new_string": "let x = 2;"}"#;
const WRITE_USAGE: &str = r#"{"path": "src/new.rs", "content": "fn main() {}\n"}"#;

/// A change worked out but not yet written.
struct Plan {
    target: Resolved,
    content: String,
    diff: FileDiff,
    summary: String,
}

impl Plan {
    /// Writes the planned content, creating parent directories.
    fn apply(self, tool: &str) -> ToolOutput {
        let abs = &self.target.abs;
        if let Some(parent) = abs.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return ToolOutput::error(tool, format!("Cannot create the folder for {}: {e}", self.target.rel));
        }
        if let Err(e) = std::fs::write(abs, &self.content) {
            return ToolOutput::error(tool, format!("Cannot write {}: {e}", self.target.rel));
        }
        let files = if self.diff.added + self.diff.removed > 0 || self.diff.created {
            vec![FileChange { path: self.target.rel.clone(), added: self.diff.added, removed: self.diff.removed }]
        } else {
            Vec::new()
        };
        ToolOutput { content: self.summary, is_error: false, display: ToolDisplay::Diff(self.diff), files }
    }
}

/// The current text of a file, `None` when it does not exist. Directories, binary files,
/// non-UTF-8 text and huge files are errors.
fn read_existing(r: &Resolved) -> Result<Option<String>, String> {
    match std::fs::metadata(&r.abs) {
        Ok(m) if m.is_dir() => return Err(format!("{} is a directory, not a file", r.rel)),
        Ok(m) if m.len() > EDIT_MAX_BYTES => {
            return Err(format!("{} is too large to edit ({})", r.rel, base::fmt::bytes(m.len())));
        }
        Ok(_) => {}
        Err(e) if matches!(e.kind(), ErrorKind::NotFound) => return Ok(None),
        Err(e) => return Err(format!("Cannot read {}: {e}", r.rel)),
    }
    let bytes = std::fs::read(&r.abs).map_err(|e| format!("Cannot read {}: {e}", r.rel))?;
    if is_binary(&bytes) {
        return Err(format!("{} is a binary file; it cannot be edited as text", r.rel));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| format!("{} is not UTF-8 text; it cannot be edited", r.rel))
}

/// Replaces exact text in an existing file.
pub struct EditFile;

impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: EDIT_FILE.into(),
            description: "Edit an existing file by replacing exact text. `old_string` must match the file \
                exactly, whitespace and indentation included (without the line-number prefix read_file \
                shows), and must occur exactly once: include enough surrounding lines to make it unique, or \
                set `replace_all` to replace every occurrence. An empty `new_string` deletes the text. Read \
                the file first. Use write_file to create files. The person may be asked to approve the change."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace root."},
                    "old_string": {"type": "string", "description": "Exact text to replace."},
                    "new_string": {"type": "string", "description": "Text to put in its place."},
                    "replace_all": {"type": "boolean", "description": "Replace every occurrence instead of exactly one (default false)."}
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Write
    }

    fn subject(&self, input: &Value) -> String {
        peek_str(input, "path").map(|p| normalize_subject(&p)).unwrap_or_default()
    }

    fn preview(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        match plan_edit(ctx, input) {
            Ok(plan) => ToolDisplay::Diff(plan.diff),
            Err(e) => preview_error(EDIT_FILE, e),
        }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        match plan_edit(ctx, input) {
            Ok(plan) => plan.apply(EDIT_FILE),
            Err(e) => ToolOutput::error(EDIT_FILE, e),
        }
    }
}

fn plan_edit(ctx: &ToolContext<'_>, input: &Value) -> Result<Plan, String> {
    let args = Args::new(EDIT_FILE, EDIT_USAGE, input)?;
    let path = args.str("path")?;
    let old = args.str("old_string")?;
    let new = args.str("new_string")?;
    let replace_all = args.opt_bool("replace_all")?.unwrap_or(false);
    if old.is_empty() {
        return Err(format!("{EDIT_FILE}: `old_string` must not be empty; use write_file to create or overwrite a file"));
    }
    if old == new {
        return Err(format!("{EDIT_FILE}: `old_string` and `new_string` are the same; there is nothing to change"));
    }
    let target = resolve(ctx.root, path).map_err(|e| e.to_string())?;
    let Some(current) = read_existing(&target)? else {
        return Err(format!("File not found: {}. Use write_file to create a new file", target.rel));
    };

    // A CRLF file edited with LF text: match and replace in the file's own line endings.
    // A file with mixed endings falls back to the text as given.
    let converted = (current.contains("\r\n") && !old.contains("\r\n") && !new.contains("\r\n"))
        .then(|| (to_crlf(old), to_crlf(new)));
    let (old, new) = match &converted {
        Some((o, n)) if current.contains(o.as_str()) => (o.as_str(), n.as_str()),
        _ => (old, new),
    };
    let found = current.matches(old).count();
    if found == 0 {
        return Err(not_found(&target.rel, &current, old));
    }
    if found > 1 && !replace_all {
        return Err(format!(
            "`old_string` occurs {found} times in {}. Include more surrounding lines so it matches exactly \
             once, or set `replace_all` to true to replace all of them",
            target.rel
        ));
    }
    let content = if replace_all { current.replace(old, new) } else { current.replacen(old, new, 1) };
    let diff = file_diff(&target.rel, &current, &content, false);
    let summary = format!(
        "Edited {}: replaced {} (+{} -{} lines).",
        target.rel,
        plural(found as u64, "occurrence", "occurrences"),
        diff.added,
        diff.removed
    );
    Ok(Plan { target, content, diff, summary })
}

/// The error for text that is not in the file, with a hint where a near match is.
fn not_found(rel: &str, content: &str, old: &str) -> String {
    let mut message = format!(
        "`old_string` was not found in {rel}. It must match the file exactly, including whitespace and \
         indentation; read_file the current content and copy the text from there"
    );
    let probe = old.lines().map(str::trim).find(|l| !l.is_empty());
    if let Some(probe) = probe {
        let lines: Vec<usize> = content
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(probe))
            .map(|(i, _)| i + 1)
            .take(5)
            .collect();
        if !lines.is_empty() {
            let list = lines.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
            message.push_str(&format!(". Its first line appears (ignoring indentation) at line {list}"));
        }
    }
    message
}

fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// Creates a file or replaces its whole content.
pub struct WriteFile;

impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: WRITE_FILE.into(),
            description: "Create a file, or overwrite it completely, with `content`. `path` is relative to the \
                workspace root; missing parent folders are created. Prefer edit_file for changes to an \
                existing file, and read a file before overwriting it. The person may be asked to approve the \
                change."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace root."},
                    "content": {"type": "string", "description": "The complete new content of the file."}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Write
    }

    fn subject(&self, input: &Value) -> String {
        peek_str(input, "path").map(|p| normalize_subject(&p)).unwrap_or_default()
    }

    fn preview(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        match plan_write(ctx, input) {
            Ok(plan) => ToolDisplay::Diff(plan.diff),
            Err(e) => preview_error(WRITE_FILE, e),
        }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        match plan_write(ctx, input) {
            Ok(plan) => plan.apply(WRITE_FILE),
            Err(e) => ToolOutput::error(WRITE_FILE, e),
        }
    }
}

fn plan_write(ctx: &ToolContext<'_>, input: &Value) -> Result<Plan, String> {
    let args = Args::new(WRITE_FILE, WRITE_USAGE, input)?;
    let path = args.str("path")?;
    let content = args.str("content")?;
    let target = resolve(ctx.root, path).map_err(|e| e.to_string())?;
    if target.is_root() {
        return Err(format!("{WRITE_FILE}: `path` must name a file, not the workspace root"));
    }
    let previous = match read_existing(&target) {
        Ok(p) => p,
        // Replacing a binary or non-UTF-8 file: there is no text to diff against.
        Err(_) if target.abs.is_file() => Some(String::new()),
        Err(e) => return Err(e),
    };
    let created = previous.is_none();
    let diff = file_diff(&target.rel, previous.as_deref().unwrap_or(""), content, created);
    let lines = plural(content.lines().count() as u64, "line", "lines");
    let summary = if created {
        format!("Created {} ({lines}).", target.rel)
    } else {
        format!("Wrote {} ({lines}; +{} -{} lines).", target.rel, diff.added, diff.removed)
    };
    Ok(Plan { target, content: content.to_string(), diff, summary })
}
