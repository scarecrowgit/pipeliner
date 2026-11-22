//! Implementations of the file tools.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::diff::diff_text;
use crate::paths::{Resolved, canonical_root, rel_display, resolve};
use crate::{ToolDisplay, ToolOutput};

pub(crate) const READ_DEFAULT_LIMIT: usize = 2000;
const READ_MAX_BYTES: usize = 256 * 1024;
const READ_MAX_LINE_CHARS: usize = 2000;
const LIST_MAX_ENTRIES: usize = 500;
const GREP_MAX_MATCHES: usize = 200;
const GREP_MAX_LINE_CHARS: usize = 300;
const GREP_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const BINARY_SNIFF: usize = 8192;
pub(crate) const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules"];

pub(crate) fn error(message: impl Into<String>) -> ToolOutput {
    let message = message.into();
    ToolOutput { content: message.clone(), is_error: true, display: ToolDisplay::Error { message } }
}

// ---- argument helpers -------------------------------------------------------------------

pub(crate) fn req_str<'a>(input: &'a Value, key: &str, tool: &str) -> Result<&'a str, String> {
    match input.get(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(format!("{tool}: `{key}` must be a string")),
        None => Err(format!("{tool}: missing required parameter `{key}`")),
    }
}

pub(crate) fn opt_str<'a>(input: &'a Value, key: &str, tool: &str) -> Result<Option<&'a str>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{tool}: `{key}` must be a string")),
    }
}

pub(crate) fn opt_uint(input: &Value, key: &str, tool: &str) -> Result<Option<u64>, String> {
    let bad = || format!("{tool}: `{key}` must be a non-negative integer");
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                Ok(Some(u))
            } else {
                match n.as_f64() {
                    Some(f) if f >= 0.0 && f.fract() == 0.0 => Ok(Some(f as u64)),
                    _ => Err(bad()),
                }
            }
        }
        Some(Value::String(s)) => s.trim().parse::<u64>().map(Some).map_err(|_| bad()),
        Some(_) => Err(bad()),
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0)
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Read a text file for editing. `Ok(None)` if it doesn't exist.
fn read_text(r: &Resolved, tool: &str) -> Result<Option<String>, String> {
    if r.abs.is_dir() {
        return Err(format!("{tool}: {} is a directory", r.rel));
    }
    match std::fs::read(&r.abs) {
        Ok(bytes) => {
            if is_binary(&bytes) {
                return Err(format!("{tool}: {} looks like a binary file", r.rel));
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| format!("{tool}: {} is not valid UTF-8 text", r.rel))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{tool}: cannot read {}: {e}", r.rel)),
    }
}

// ---- read_file --------------------------------------------------------------------------

pub(crate) fn read_args(input: &Value) -> Result<(&str, usize, usize), String> {
    let path = req_str(input, "path", "read_file")?;
    let offset = opt_uint(input, "offset", "read_file")?.unwrap_or(1).max(1) as usize;
    let limit = opt_uint(input, "limit", "read_file")?.unwrap_or(READ_DEFAULT_LIMIT as u64) as usize;
    if limit == 0 {
        return Err("read_file: `limit` must be at least 1".into());
    }
    Ok((path, offset, limit))
}

pub(crate) fn read_file(root: &Path, input: &Value) -> ToolOutput {
    let (path, offset, limit) = match read_args(input) {
        Ok(a) => a,
        Err(e) => return error(e),
    };
    let r = match resolve(root, path) {
        Ok(r) => r,
        Err(e) => return error(e),
    };
    if r.abs.is_dir() {
        return error(format!("{} is a directory; use list_dir to see its entries", r.rel));
    }
    let bytes = match std::fs::read(&r.abs) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return error(format!("File not found: {}", r.rel));
        }
        Err(e) => return error(format!("Cannot read {}: {e}", r.rel)),
    };
    if is_binary(&bytes) {
        return error(format!("{} looks like a binary file ({} bytes); not showing it", r.rel, bytes.len()));
    }
    let text = String::from_utf8_lossy(&bytes);
    let display = |lines| ToolDisplay::Read { path: r.rel.clone(), lines };
    if text.is_empty() {
        return ToolOutput { content: format!("{} is empty", r.rel), is_error: false, display: display(0) };
    }
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    if offset > total {
        return ToolOutput {
            content: format!("{} has only {total} lines; offset {offset} is past the end", r.rel),
            is_error: true,
            display: ToolDisplay::Error { message: format!("offset {offset} past end ({total} lines)") },
        };
    }
    let mut out = String::new();
    let mut shown = 0;
    let mut byte_capped = false;
    for (i, line) in all.iter().enumerate().skip(offset - 1).take(limit) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let entry = format!("{:>6}\t{}\n", i + 1, truncate_chars(line, READ_MAX_LINE_CHARS));
        if out.len() + entry.len() > READ_MAX_BYTES && shown > 0 {
            byte_capped = true;
            break;
        }
        out.push_str(&entry);
        shown += 1;
    }
    let last = offset - 1 + shown;
    if last < total {
        let why = if byte_capped { "output size limit reached" } else { "line limit reached" };
        out.push_str(&format!(
            "\n[Showing lines {offset}-{last} of {total} ({why}). Use offset={} to read more.]\n",
            last + 1
        ));
    }
    ToolOutput { content: out, is_error: false, display: display(shown) }
}

// ---- list_dir ---------------------------------------------------------------------------

pub(crate) fn list_dir(root: &Path, input: &Value) -> ToolOutput {
    let path = match opt_str(input, "path", "list_dir") {
        Ok(p) => p.unwrap_or("."),
        Err(e) => return error(e),
    };
    let r = match resolve(root, path) {
        Ok(r) => r,
        Err(e) => return error(e),
    };
    if !r.abs.exists() {
        return error(format!("Directory not found: {}", r.rel));
    }
    if !r.abs.is_dir() {
        return error(format!("{} is a file, not a directory; use read_file", r.rel));
    }
    let entries = match dir_entries(&r.abs) {
        Ok(e) => e,
        Err(e) => return error(format!("Cannot list {}: {e}", r.rel)),
    };
    let total = entries.len();
    let mut out = String::new();
    if total == 0 {
        out.push_str(&format!("{} is empty\n", r.rel));
    }
    for e in entries.iter().take(LIST_MAX_ENTRIES) {
        out.push_str(e);
        out.push('\n');
    }
    if total > LIST_MAX_ENTRIES {
        out.push_str(&format!("\n[{} more entries not shown]\n", total - LIST_MAX_ENTRIES));
    }
    ToolOutput { content: out, is_error: false, display: ToolDisplay::List { path: r.rel, entries: total } }
}

/// Sorted entry names (dirs with trailing `/`), skipping `SKIP_DIRS`.
pub(crate) fn dir_entries(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        // Follows symlinks: a link to a directory is listed as a directory.
        if entry.path().is_dir() {
            out.push(format!("{name}/"));
        } else {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

// ---- grep -------------------------------------------------------------------------------

pub(crate) struct GrepArgs<'a> {
    pub pattern: &'a str,
    pub regex: regex::Regex,
    pub path: Resolved,
    pub glob: Option<&'a str>,
}

pub(crate) fn grep_args<'a>(root: &Path, input: &'a Value) -> Result<GrepArgs<'a>, String> {
    let pattern = req_str(input, "pattern", "grep")?;
    let regex = regex::Regex::new(pattern).map_err(|e| format!("grep: invalid regex: {e}"))?;
    let path = resolve(root, opt_str(input, "path", "grep")?.unwrap_or("."))?;
    if !path.abs.exists() {
        return Err(format!("grep: path not found: {}", path.rel));
    }
    let glob = opt_str(input, "glob", "grep")?.filter(|g| !g.trim().is_empty());
    Ok(GrepArgs { pattern, regex, path, glob })
}

pub(crate) fn grep(root: &Path, input: &Value, cancel: &AtomicBool) -> ToolOutput {
    let args = match grep_args(root, input) {
        Ok(a) => a,
        Err(e) => return error(e),
    };
    let croot = canonical_root(root);
    let mut builder = ignore::WalkBuilder::new(&args.path.abs);
    builder.hidden(true).git_ignore(true).git_exclude(true).parents(true).require_git(false);
    builder.sort_by_file_name(|a, b| a.cmp(b));
    builder.filter_entry(|e| {
        e.depth() == 0 || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
    });
    if let Some(glob) = args.glob {
        let base = if args.path.abs.is_dir() { args.path.abs.clone() } else { croot.clone() };
        let mut ob = ignore::overrides::OverrideBuilder::new(base);
        for g in glob.split(',').map(str::trim).filter(|g| !g.is_empty()) {
            if let Err(e) = ob.add(g) {
                return error(format!("grep: invalid glob {g:?}: {e}"));
            }
        }
        match ob.build() {
            Ok(o) => {
                builder.overrides(o);
            }
            Err(e) => return error(format!("grep: invalid glob: {e}")),
        }
    }

    let mut out = String::new();
    let mut matches = 0usize;
    let mut files = 0usize;
    let mut capped = false;
    let mut cancelled = false;
    'walk: for entry in builder.build() {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.metadata().is_ok_and(|m| m.len() > GREP_MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        if is_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let rel = rel_display(&croot, entry.path());
        let mut file_hit = false;
        for (i, line) in text.lines().enumerate() {
            if !args.regex.is_match(line) {
                continue;
            }
            if matches >= GREP_MAX_MATCHES {
                capped = true;
                break 'walk;
            }
            if !file_hit {
                file_hit = true;
                files += 1;
            }
            matches += 1;
            let line = line.strip_suffix('\r').unwrap_or(line);
            out.push_str(&format!("{rel}:{}: {}\n", i + 1, truncate_chars(line.trim_end(), GREP_MAX_LINE_CHARS)));
        }
    }

    if matches == 0 && !cancelled {
        out.push_str("No matches found.\n");
    } else {
        let s = if matches == 1 { "" } else { "es" };
        let fs = if files == 1 { "" } else { "s" };
        out.push_str(&format!("\n{matches} match{s} in {files} file{fs}"));
        if capped {
            out.push_str(&format!(" (stopped at {GREP_MAX_MATCHES} matches; narrow the pattern, path or glob)"));
        }
        out.push('\n');
    }
    if cancelled {
        out.push_str("[grep cancelled]\n");
    }
    ToolOutput {
        content: out,
        is_error: cancelled,
        display: ToolDisplay::Grep { pattern: args.pattern.to_string(), matches, files },
    }
}

// ---- edit_file / write_file -------------------------------------------------------------

pub(crate) struct PlannedWrite {
    pub resolved: Resolved,
    pub new_content: String,
    pub display: ToolDisplay,
    pub created: bool,
    pub added: usize,
    pub removed: usize,
}

fn diff_display(path: &str, old: &str, new: &str, created: bool) -> (ToolDisplay, usize, usize) {
    let d = diff_text(old, new);
    let (added, removed) = (d.added, d.removed);
    (ToolDisplay::Diff { path: path.to_string(), added, removed, lines: d.lines, created }, added, removed)
}

/// Compute an edit without applying it.
pub(crate) fn plan_edit(root: &Path, input: &Value) -> Result<PlannedWrite, String> {
    let path = req_str(input, "path", "edit_file")?;
    let old_string = req_str(input, "old_string", "edit_file")?;
    let new_string = req_str(input, "new_string", "edit_file")?;
    if old_string.is_empty() {
        return Err("edit_file: `old_string` must not be empty (use write_file to create a file)".into());
    }
    if old_string == new_string {
        return Err("edit_file: `old_string` and `new_string` are identical; nothing to change".into());
    }
    let r = resolve(root, path)?;
    let Some(content) = read_text(&r, "edit_file")? else {
        return Err(format!("edit_file: file not found: {} (use write_file to create it)", r.rel));
    };

    // Preserve CRLF line endings: match/replace with the file's convention.
    let crlf = content.contains("\r\n");
    let candidates: Vec<(String, String)> = if crlf && !old_string.contains("\r\n") {
        vec![
            (to_crlf(old_string), to_crlf(new_string)),
            (old_string.to_string(), new_string.to_string()),
        ]
    } else {
        vec![(old_string.to_string(), new_string.to_string())]
    };
    let mut count = 0;
    let mut chosen = None;
    for (old, new) in &candidates {
        count = content.matches(old.as_str()).count();
        if count > 0 {
            chosen = Some((old, new));
            break;
        }
    }
    let Some((old, new)) = chosen else {
        return Err(format!(
            "edit_file: old_string not found in {}. It must match the file exactly, including \
             whitespace and indentation; read_file the current content first.",
            r.rel
        ));
    };
    if count > 1 {
        return Err(format!(
            "edit_file: old_string found {count} times in {}; it must be unique. Include more \
             surrounding lines to identify the exact location.",
            r.rel
        ));
    }
    let new_content = content.replacen(old.as_str(), new, 1);
    let (display, added, removed) = diff_display(&r.rel, &content, &new_content, false);
    Ok(PlannedWrite { resolved: r, new_content, display, created: false, added, removed })
}

fn to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\n', "\r\n")
}

pub(crate) fn plan_write(root: &Path, input: &Value) -> Result<PlannedWrite, String> {
    let path = req_str(input, "path", "write_file")?;
    let content = req_str(input, "content", "write_file")?;
    let r = resolve(root, path)?;
    if r.rel == "." {
        return Err("write_file: `path` must name a file".into());
    }
    let prev = match read_text(&r, "write_file") {
        Ok(p) => p,
        // Overwriting a binary / non-UTF-8 file: diff against nothing.
        Err(_) if r.abs.is_file() => Some(String::new()),
        Err(e) => return Err(e),
    };
    let created = prev.is_none();
    let (display, added, removed) = diff_display(&r.rel, prev.as_deref().unwrap_or(""), content, created);
    Ok(PlannedWrite { resolved: r, new_content: content.to_string(), display, created, added, removed })
}

pub(crate) fn apply(plan: PlannedWrite, tool: &str) -> ToolOutput {
    let r = &plan.resolved;
    if let Some(parent) = r.abs.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return error(format!("{tool}: cannot create directory for {}: {e}", r.rel));
    }
    if let Err(e) = std::fs::write(&r.abs, &plan.new_content) {
        return error(format!("{tool}: cannot write {}: {e}", r.rel));
    }
    let content = if plan.created {
        format!("Created {} ({} lines)", r.rel, plan.new_content.lines().count())
    } else if tool == "edit_file" {
        format!("Edited {} (+{} -{} lines)", r.rel, plan.added, plan.removed)
    } else {
        format!("Wrote {} (+{} -{} lines)", r.rel, plan.added, plan.removed)
    };
    ToolOutput { content, is_error: false, display: plan.display }
}
