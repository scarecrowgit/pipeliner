//! `grep`: a regex search over the workspace that honours `.gitignore`.

use std::fmt::Write as _;

use base::fmt::{count, plural, truncate_chars};
use ignore::overrides::OverrideBuilder;
use model::{ToolClass, ToolDisplay, ToolSpec};
use serde_json::{Value, json};

use crate::api::{Tool, ToolContext, ToolOutput};
use crate::args::{Args, peek_str};
use crate::builtin::names::GREP;
use crate::sandbox::{canonical_root, rel_display, resolve};
use crate::text::{is_binary, strip_eol, walker};

/// Matching lines one call returns at most.
pub const GREP_MAX_MATCHES: usize = 200;
/// Characters kept of a matching line.
const GREP_MAX_LINE_CHARS: usize = 300;
/// Files larger than this are skipped.
const GREP_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

const USAGE: &str = r#"{"pattern": "fn main", "path": "src", "glob": "*.rs"}"#;

/// Searches file contents with a regular expression.
pub struct Grep;

impl Tool for Grep {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: GREP.into(),
            description: "Search file contents with a regular expression (Rust regex syntax; start with `(?i)` \
                for case-insensitive). Searches `path` (a file or directory relative to the workspace root, \
                default the root) recursively, skipping files ignored by .gitignore, hidden files and binary \
                files. `glob` limits the files searched, e.g. `*.rs` or `crates/**/*.toml` (comma-separate \
                several). Each match is printed as `path:line: text`; at most 200 matches are returned, so \
                prefer specific patterns."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression to search for."},
                    "path": {"type": "string", "description": "File or directory to search, relative to the workspace root (default `.`)."},
                    "glob": {"type": "string", "description": "Glob filter for file paths, e.g. `*.rs` or `src/**/*.ts`."}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Read
    }

    fn subject(&self, input: &Value) -> String {
        crate::sandbox::normalize_subject(peek_str(input, "path").as_deref().unwrap_or("."))
    }

    fn preview(&self, _ctx: &ToolContext<'_>, input: &Value) -> ToolDisplay {
        let pattern = peek_str(input, "pattern").map(|p| p.into_owned()).unwrap_or_default();
        ToolDisplay::Line { verb: "grep".into(), target: pattern, result: String::new() }
    }

    fn run(&self, ctx: &ToolContext<'_>, input: &Value) -> ToolOutput {
        grep(ctx, input).unwrap_or_else(|e| ToolOutput::error(GREP, e))
    }
}

fn grep(ctx: &ToolContext<'_>, input: &Value) -> Result<ToolOutput, String> {
    let args = Args::new(GREP, USAGE, input)?;
    let pattern = args.str("pattern")?;
    if pattern.is_empty() {
        return Err(format!("{GREP}: `pattern` must not be empty. Example input: {USAGE}"));
    }
    let regex = regex::RegexBuilder::new(pattern)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|e| format!("{GREP}: `pattern` is not a valid regular expression: {e}"))?;
    let path = args.opt_str("path")?.filter(|p| !p.trim().is_empty()).unwrap_or(".");
    let glob = args.opt_str("glob")?.map(str::trim).filter(|g| !g.is_empty());
    let croot = canonical_root(ctx.root).map_err(|e| e.to_string())?;
    let target = resolve(ctx.root, path).map_err(|e| e.to_string())?;
    if !target.abs.exists() {
        return Err(format!("Path not found: {}. Use list_dir to see what exists", target.rel));
    }

    let mut builder = walker(&target.abs, false);
    if let Some(glob) = glob {
        // Globs are relative to the workspace root, like .gitignore lines.
        let mut globs = OverrideBuilder::new(&croot);
        for g in glob.split(',').map(str::trim).filter(|g| !g.is_empty()) {
            globs.add(g).map_err(|e| format!("{GREP}: invalid glob {g:?}: {e}"))?;
        }
        builder.overrides(globs.build().map_err(|e| format!("{GREP}: invalid glob: {e}"))?);
    }

    let mut out = String::new();
    let (mut matches, mut files) = (0usize, 0usize);
    let (mut capped, mut cancelled) = (false, false);
    'walk: for entry in builder.build() {
        if ctx.cancel.is_cancelled() {
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
        let mut hit = false;
        for (i, line) in text.split_inclusive('\n').enumerate() {
            let line = strip_eol(line);
            if !regex.is_match(line) {
                continue;
            }
            if matches >= GREP_MAX_MATCHES {
                capped = true;
                break 'walk;
            }
            if !hit {
                hit = true;
                files += 1;
            }
            matches += 1;
            let _ = writeln!(out, "{rel}:{}: {}", i + 1, truncate_chars(line.trim_end(), GREP_MAX_LINE_CHARS));
        }
    }

    let result = if matches == 0 {
        "no matches".to_string()
    } else {
        format!("{} · {}", plural(matches as u64, "match", "matches"), plural(files as u64, "file", "files"))
    };
    if matches == 0 {
        out.push_str("No matches found.\n");
    } else {
        let _ = write!(out, "\n{result}");
        if capped {
            let _ = write!(
                out,
                " (stopped at {} matches; narrow the pattern, path or glob)",
                count(GREP_MAX_MATCHES as u64)
            );
        }
        out.push('\n');
    }
    if cancelled {
        out.push_str("[grep was cancelled before it finished]\n");
    }
    let display = ToolDisplay::Line { verb: "grep".into(), target: pattern.to_string(), result };
    Ok(ToolOutput { content: out, is_error: cancelled, display, files: Vec::new() })
}
