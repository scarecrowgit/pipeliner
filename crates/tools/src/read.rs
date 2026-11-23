//! `read_file` and `list_dir`.

use std::io::{BufRead, BufReader, Read};

use base::fmt::{count, plural, truncate_chars};
use model::{ToolClass, ToolDisplay, ToolSpec};
use serde_json::{Value, json};

use crate::api::{Tool, ToolContext, ToolOutput};
use crate::args::{Args, peek_str};
use crate::builtin::names::{LIST_DIR, READ_FILE};
use crate::sandbox::{OpenError, normalize_subject, open_regular, resolve};
use crate::text::{is_binary, strip_eol, walker};

/// Lines returned by one call when no limit is given, and at most.
pub const READ_MAX_LINES: usize = 2000;
/// Bytes of output one call returns at most.
pub const READ_MAX_BYTES: usize = 256 * 1024;
/// Characters kept of a single very long line.
const READ_MAX_LINE_CHARS: usize = 2000;
/// Past this many bytes the line count of the rest of the file is not worth the time.
const READ_COUNT_BYTES: u64 = 64 * 1024 * 1024;
/// Entries one `list_dir` call shows at most.
pub const LIST_MAX_ENTRIES: usize = 500;

const READ_USAGE: &str = r#"{"path": "src/main.rs", "offset": 1, "limit": 200}"#;
const LIST_USAGE: &str = r#"{"path": "src"}"#;

/// The subject of a file tool call: the resolved path relative to the root, empty when the
/// path is missing or the sandbox refuses it. `default` is used when `path` is not given.
pub(crate) fn path_subject(ctx: &ToolContext<'_>, input: &Value, default: Option<&str>) -> String {
    let path = peek_str(input, "path").filter(|p| !p.trim().is_empty());
    let Some(path) = path.as_deref().or(default) else { return String::new() };
    resolve(ctx.root, path).map(|r| r.rel).unwrap_or_default()
}

/// The path a preview shows: the resolved one, or what the model wrote when it cannot be
/// resolved (the call will then fail with the reason).
pub(crate) fn shown_path(ctx: &ToolContext<'_>, input: &Value, default: Option<&str>) -> String {
    let subject = path_subject(ctx, input, default);
    if !subject.is_empty() {
        return subject;
    }
    peek_str(input, "path").map(|p| normalize_subject(&p)).unwrap_or_default()
}

/// Reads a text file with line numbers, a window at a time.
pub struct ReadFile;

impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: READ_FILE.into(),
            description: "Read a text file in the workspace. `path` is relative to the workspace root. Lines \
                come back prefixed with their 1-based number and a tab (`    12\tcode`); the prefix is not \
                part of the file. At most 2000 lines / 256 KB per call: use `offset` (first line, 1-based) \
                and `limit` (number of lines) to page through larger files. Binary files are refused. Read \
                a file before editing it."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path relative to the workspace root, e.g. `src/main.rs`."},
                    "offset": {"type": "integer", "minimum": 1, "description": "First line to read, 1-based (default 1)."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": READ_MAX_LINES, "description": "Number of lines to read (default and maximum 2000)."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Read
    }

    fn subject(&self, ctx: &ToolContext<'_>, input: &Value) -> String {
        path_subject(ctx, input, None)
    }

    fn preview(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        ToolDisplay::Line { verb: "read".into(), target: shown_path(ctx, input, None), result: String::new() }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        read_file(ctx, input).unwrap_or_else(|e| ToolOutput::error(READ_FILE, e))
    }
}

fn read_file(ctx: &ToolContext<'_>, input: &Value) -> Result<ToolOutput, String> {
    let args = Args::new(READ_FILE, READ_USAGE, input)?;
    let path = args.str("path")?;
    let offset = args.opt_u64("offset")?.unwrap_or(1).max(1);
    let limit = args.opt_u64("limit")?.unwrap_or(READ_MAX_LINES as u64);
    if limit == 0 {
        return Err(format!("{READ_FILE}: `limit` must be at least 1. Example input: {READ_USAGE}"));
    }
    let offset = usize::try_from(offset).unwrap_or(usize::MAX);
    let limit = usize::try_from(limit).unwrap_or(usize::MAX).min(READ_MAX_LINES);

    let r = resolve(ctx.root, path).map_err(|e| e.to_string())?;
    let mut file = match open_regular(&r) {
        Ok(f) => f,
        Err(OpenError::IsDir) => {
            return Err(format!("{} is a directory; use list_dir to see what it holds", r.rel));
        }
        Err(OpenError::NotFound) => {
            return Err(format!("File not found: {}. Use list_dir or grep to find the right path", r.rel));
        }
        Err(e) => return Err(e.message(&r.rel)),
    };
    let mut head = Vec::new();
    (&mut file).take(8192).read_to_end(&mut head).map_err(|e| format!("Cannot read {}: {e}", r.rel))?;
    if is_binary(&head) {
        let size = file.metadata().map(|m| base::fmt::bytes(m.len())).unwrap_or_default();
        return Err(format!("{} is a binary file ({size}); read_file shows text files only", r.rel));
    }
    let mut reader = BufReader::new(std::io::Cursor::new(head).chain(file));

    let display = |result: String| ToolDisplay::Line { verb: "read".into(), target: r.rel.clone(), result };
    let mut out = String::new();
    let mut line = Vec::new();
    let (mut total, mut shown, mut read_bytes) = (0usize, 0usize, 0u64);
    let mut byte_capped = false;
    let mut counted_all = true;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).map_err(|e| format!("Cannot read {}: {e}", r.rel))?;
        if n == 0 {
            break;
        }
        total += 1;
        read_bytes += n as u64;
        let wanted = total >= offset && shown < limit && !byte_capped;
        if wanted {
            let text = String::from_utf8_lossy(&line);
            let entry = format!("{total:>6}\t{}\n", truncate_chars(strip_eol(&text), READ_MAX_LINE_CHARS));
            if shown > 0 && out.len() + entry.len() > READ_MAX_BYTES {
                byte_capped = true;
            } else {
                out.push_str(&entry);
                shown += 1;
            }
        } else if total >= offset && read_bytes > READ_COUNT_BYTES {
            counted_all = false;
            break;
        }
    }

    if total == 0 {
        return Ok(ToolOutput {
            content: format!("{} is empty.", r.rel),
            is_error: false,
            display: display("empty".into()),
            files: Vec::new(),
        });
    }
    if offset > total {
        return Err(format!(
            "{} has {}; offset {offset} is past the end. Use an offset between 1 and {total}",
            r.rel,
            plural(total as u64, "line", "lines")
        ));
    }
    let last = offset + shown - 1;
    let whole = offset == 1 && last == total && counted_all;
    if !whole {
        let of = if counted_all { format!(" of {}", count(total as u64)) } else { String::new() };
        let why = if byte_capped { "the 256 KB output limit was reached" } else { "the line limit was reached" };
        if last < total || !counted_all {
            out.push_str(&format!(
                "\n[Showing lines {offset}-{last}{of}; {why}. Call read_file with offset={} to continue.]\n",
                last + 1
            ));
        }
    }
    let result = if whole {
        plural(shown as u64, "line", "lines")
    } else if counted_all {
        format!("{} of {} lines", count(shown as u64), count(total as u64))
    } else {
        plural(shown as u64, "line", "lines")
    };
    Ok(ToolOutput { content: out, is_error: false, display: display(result), files: Vec::new() })
}

/// Lists one directory: directories first, then files, honouring `.gitignore`.
pub struct ListDir;

impl Tool for ListDir {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: LIST_DIR.into(),
            description: "List the entries of a workspace directory: directories first (ending in `/`), then \
                files, each group sorted by name; symlinks end in `@`. `path` is relative to the workspace \
                root (default: the root). Entries ignored by .gitignore and the .git folder are left out; at \
                most 500 entries are shown."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory relative to the workspace root (default `.`)."}
                },
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Read
    }

    fn subject(&self, ctx: &ToolContext<'_>, input: &Value) -> String {
        path_subject(ctx, input, Some("."))
    }

    fn preview(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        ToolDisplay::Line { verb: "list".into(), target: shown_path(ctx, input, Some(".")), result: String::new() }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        list_dir(ctx, input).unwrap_or_else(|e| ToolOutput::error(LIST_DIR, e))
    }
}

fn list_dir(ctx: &ToolContext<'_>, input: &Value) -> Result<ToolOutput, String> {
    let args = Args::new(LIST_DIR, LIST_USAGE, input)?;
    let path = args.opt_str("path")?.filter(|p| !p.trim().is_empty()).unwrap_or(".");
    let r = resolve(ctx.root, path).map_err(|e| e.to_string())?;
    if !r.abs.exists() {
        return Err(format!("Directory not found: {}", r.rel));
    }
    if !r.abs.is_dir() {
        return Err(format!("{} is a file, not a directory; use read_file to read it", r.rel));
    }
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in walker(&r.abs, true).max_depth(Some(1)).build() {
        let Ok(entry) = entry else { continue };
        if entry.depth() != 1 {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.file_type() {
            Some(t) if t.is_dir() => dirs.push(format!("{name}/")),
            Some(t) if t.is_symlink() => files.push(format!("{name}@")),
            _ => files.push(name),
        }
    }
    dirs.sort();
    files.sort();
    let total = dirs.len() + files.len();
    let mut out = String::new();
    if total == 0 {
        out.push_str(&format!("{} is empty.\n", r.rel));
    }
    for name in dirs.iter().chain(&files).take(LIST_MAX_ENTRIES) {
        out.push_str(name);
        out.push('\n');
    }
    if total > LIST_MAX_ENTRIES {
        out.push_str(&format!(
            "\n[{} more entries not shown. List a subdirectory, or use grep with a glob to narrow down.]\n",
            count((total - LIST_MAX_ENTRIES) as u64)
        ));
    }
    let display = ToolDisplay::Line {
        verb: "list".into(),
        target: r.rel,
        result: plural(total as u64, "entry", "entries"),
    };
    Ok(ToolOutput { content: out, is_error: false, display, files: Vec::new() })
}
