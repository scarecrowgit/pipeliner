//! Tools agents can call. File tools are rooted at the task's working directory.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context as _, Result};
use harness_core::{skills, Config, ToolAccess};
use serde_json::{json, Value};

use crate::llm::ToolSpec;

pub struct ToolContext {
    pub cwd: Option<PathBuf>,
    pub shell: String,
    pub env: BTreeMap<String, String>,
    pub access: ToolAccess,
    /// Other folders readable with read_workdir: (id, path).
    pub workdirs: Vec<(String, String)>,
    pub skills_on_demand: Vec<String>,
    pub config: Config,
    pub cancel: Arc<AtomicBool>,
}

fn spec(name: &'static str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec {
        name,
        description: description.to_string(),
        parameters,
    }
}

/// Tool specs for this context's access level.
pub fn specs(ctx: &ToolContext) -> Vec<ToolSpec> {
    let mut v = vec![];
    if ctx.access == ToolAccess::None {
        return v;
    }
    if ctx.cwd.is_some() {
        let range = json!({"type":"object","properties":{
            "path":{"type":"string"},
            "offset":{"type":"integer","description":"first line, 1-based"},
            "limit":{"type":"integer","description":"number of lines (default 2000)"}
        },"required":["path"]});
        v.push(spec(
            "read_file",
            "Read text files with line numbers. Use this, not cat/sed/head/tail in run_command. \
             One file: `path` (+ `offset`/`limit` lines). Several files or ranges in one call: `files`. \
             Images (PNG, JPEG, GIF, WebP: screenshots, renders) are shown to you: read them to look at them.",
            json!({"type":"object","properties":{
                "path":{"type":"string"},
                "offset":{"type":"integer","description":"first line, 1-based"},
                "limit":{"type":"integer","description":"number of lines (default 2000)"},
                "files":{"type":"array","items":range,"description":"several files or ranges at once"}
            }}),
        ));
        v.push(spec(
            "list_dir",
            "List one directory (default: the working directory). Shows sizes; directories end with /. \
             For files across folders use find_files.",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        ));
        v.push(spec(
            "find_files",
            "Find files by glob, recursively (`**/*.rs`, `crates/*/src/**/mod.rs`). Skips .git, target \
             and node_modules. Use this, not find/ls -R in run_command.",
            json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","description":"folder to search from, default ."}},"required":["pattern"]}),
        ));
        v.push(spec(
            "search",
            "Search file contents with a regular expression (ripgrep). Returns file:line: text. \
             Use this, not grep/rg in run_command.",
            json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string","description":"file or folder, default ."},"glob":{"type":"string","description":"e.g. *.rs"}},"required":["pattern"]}),
        ));
    }
    if !ctx.workdirs.is_empty() {
        v.push(spec(
            "read_workdir",
            "Read a file or list a folder in another workdir of this run (read only).",
            json!({"type":"object","properties":{"workdir":{"type":"string"},"path":{"type":"string"}},"required":["workdir"]}),
        ));
    }
    if !ctx.skills_on_demand.is_empty() {
        v.push(spec(
            "load_skill",
            "Load a skill's guidance into the conversation.",
            json!({"type":"object","properties":{"name":{"type":"string","enum":ctx.skills_on_demand}},"required":["name"]}),
        ));
    }
    if matches!(ctx.access, ToolAccess::ReadAndRun | ToolAccess::Full) {
        let description = if ctx.cwd.is_some() {
            "Run a shell command in the working directory: builds, tests, git, scripts. Returns exit code and \
             output (truncated). Default timeout 600s. Don't use it to read, find, search or edit files \
             (cat, sed, grep, find, heredocs): the file tools do that, show in the thread as reads and \
             edits, and edit_file fails loudly when its text isn't there."
        } else {
            "Run a shell command (starts in the home directory; no working folder is attached). Returns \
             exit code and output (truncated). Default timeout 600s."
        };
        v.push(spec(
            "run_command",
            description,
            json!({"type":"object","properties":{"command":{"type":"string"},"timeout_secs":{"type":"integer"}},"required":["command"]}),
        ));
    }
    if ctx.cwd.is_some() && ctx.access == ToolAccess::Full {
        v.push(spec(
            "write_file",
            "Create or overwrite a file with the given content (no shell quoting involved: use this, not \
             cat > file <<EOF). For changes to an existing file prefer edit_file.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ));
        v.push(spec(
            "edit_file",
            "Replace exact text in a file: `old` must match exactly once unless replace_all is true. \
             Several changes to one file in one call: `edits` (applied in order; nothing is written if any \
             fails). Use this, not sed -i or scripts in run_command.",
            json!({"type":"object","properties":{
                "path":{"type":"string"},
                "old":{"type":"string"},
                "new":{"type":"string"},
                "replace_all":{"type":"boolean"},
                "edits":{"type":"array","items":{"type":"object","properties":{
                    "old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"}
                },"required":["old","new"]}}
            },"required":["path"]}),
        ));
    }
    v
}

/// Where `run_command` starts: the working folder, else the home directory, else the process dir.
fn command_dir(ctx: &ToolContext) -> PathBuf {
    if let Some(cwd) = &ctx.cwd {
        return cwd.clone();
    }
    let home = harness_core::config::expand_tilde("~");
    if home != Path::new("~") && home.is_dir() {
        return home;
    }
    std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a str> {
    args.get(k)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing `{k}`"))
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let pb = harness_core::config::expand_tilde(p);
    if pb.is_absolute() {
        pb
    } else {
        base.join(pb)
    }
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Executes one tool call. Errors are returned as text for the model to read.
pub fn execute(ctx: &ToolContext, name: &str, args: &Value) -> String {
    match execute_inner(ctx, name, args) {
        Ok(s) => s,
        Err(e) => format!("Error: {e:#}"),
    }
}

fn execute_inner(ctx: &ToolContext, name: &str, args: &Value) -> Result<String> {
    if ctx.access == ToolAccess::None {
        bail!("tools are not allowed for this agent");
    }
    let cwd = || {
        ctx.cwd
            .clone()
            .ok_or_else(|| anyhow!("no working directory"))
    };
    match name {
        "read_file" => {
            let base = cwd()?;
            match args.get("files").and_then(|f| f.as_array()).filter(|f| !f.is_empty()) {
                Some(files) => Ok(read_many(&base, files)),
                None => read_text(
                    &resolve(&base, arg(args, "path")?),
                    args["offset"].as_u64(),
                    args["limit"].as_u64(),
                ),
            }
        }
        "find_files" => {
            let base = cwd()?;
            find_files(&base, arg(args, "pattern")?, args["path"].as_str().unwrap_or("."))
        }
        "list_dir" => {
            let base = cwd()?;
            let p = args["path"]
                .as_str()
                .map(|s| resolve(&base, s))
                .unwrap_or(base);
            list_dir(&p)
        }
        "search" => {
            let base = cwd()?;
            search(
                &base,
                arg(args, "pattern")?,
                args["path"].as_str().unwrap_or("."),
                args["glob"].as_str(),
            )
        }
        "read_workdir" => {
            let id = arg(args, "workdir")?;
            let (_, root) = ctx.workdirs.iter().find(|(i, _)| i == id).ok_or_else(|| {
                anyhow!(
                    "unknown workdir `{id}`; known: {}",
                    ctx.workdirs
                        .iter()
                        .map(|w| w.0.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            let root = PathBuf::from(root);
            let p = args["path"]
                .as_str()
                .map(|s| resolve(&root, s))
                .unwrap_or(root);
            if p.is_dir() {
                list_dir(&p)
            } else {
                read_text(&p, None, None)
            }
        }
        "load_skill" => {
            let n = arg(args, "name")?;
            skills::load(&ctx.config, n).ok_or_else(|| anyhow!("skill `{n}` not found"))
        }
        "run_command" => {
            if !matches!(ctx.access, ToolAccess::ReadAndRun | ToolAccess::Full) {
                bail!("run_command is not allowed for this agent");
            }
            let timeout = args["timeout_secs"].as_u64().unwrap_or(600).clamp(5, 3600);
            let command = arg(args, "command")?;
            let out = run_command(
                &command_dir(ctx),
                &ctx.shell,
                &ctx.env,
                command,
                Duration::from_secs(timeout),
                &ctx.cancel,
            )?;
            Ok(if ctx.cwd.is_some() && is_file_only_command(command) {
                format!("{out}\n\n{FILE_TOOLS_HINT}")
            } else {
                out
            })
        }
        "write_file" => {
            ensure_write(ctx)?;
            let base = cwd()?;
            let p = normalize(&resolve(&base, arg(args, "path")?));
            ensure_inside(&base, &p)?;
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = arg(args, "content")?;
            std::fs::write(&p, content)?;
            Ok(format!(
                "Wrote {} ({} lines)",
                rel(&base, &p),
                content.lines().count()
            ))
        }
        "edit_file" => {
            ensure_write(ctx)?;
            let base = cwd()?;
            let p = normalize(&resolve(&base, arg(args, "path")?));
            ensure_inside(&base, &p)?;
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("reading {}", rel(&base, &p)))?;
            let edits = edits_of(args)?;
            let (out, lines) = apply_edits(&text, &edits)
                .map_err(|e| anyhow!("{e} in {}; nothing was written", rel(&base, &p)))?;
            std::fs::write(&p, out)?;
            Ok(format!(
                "Edited {}: {}",
                rel(&base, &p),
                edited_lines_note(&lines)
            ))
        }
        other => bail!("unknown tool `{other}`"),
    }
}

fn ensure_write(ctx: &ToolContext) -> Result<()> {
    if ctx.access != ToolAccess::Full {
        bail!("this agent may not edit files");
    }
    Ok(())
}

fn ensure_inside(base: &Path, p: &Path) -> Result<()> {
    let b = base.canonicalize().unwrap_or_else(|_| normalize(base));
    // Resolve symlinks through the nearest existing ancestor.
    let mut existing = p.to_path_buf();
    let mut rest = vec![];
    while !existing.exists() {
        match (
            existing.file_name().map(|f| f.to_owned()),
            existing.parent(),
        ) {
            (Some(f), Some(parent)) => {
                rest.push(f);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut real = existing.canonicalize().unwrap_or(existing);
    for f in rest.into_iter().rev() {
        real.push(f);
    }
    if !real.starts_with(&b) {
        bail!("{} is outside the working directory", p.display());
    }
    Ok(())
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).display().to_string()
}

/// Marks an image in a `read_file` result; the agent loop shows the image to the model
/// in the message after the tool results (tool results are text only).
pub const IMAGE_MARK: &str = "[image attached: ";

/// Image paths a `read_file` result asked to show (see [`IMAGE_MARK`]).
pub fn marked_images(result: &str) -> Vec<PathBuf> {
    result
        .lines()
        .filter_map(|l| l.strip_prefix(IMAGE_MARK)?.strip_suffix(']'))
        .map(PathBuf::from)
        .collect()
}

fn read_text(p: &Path, offset: Option<u64>, limit: Option<u64>) -> Result<String> {
    let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
    if let Some((mime, _, w, h)) = harness_core::image::sniff(&bytes) {
        let abs = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        return Ok(format!(
            "{}: {mime}, {w}×{h}, {} KB. It is shown to you in the next message.\n{IMAGE_MARK}{}]\n",
            p.display(),
            bytes.len() / 1024,
            abs.display()
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = offset.unwrap_or(1).max(1) as usize - 1;
    let lim = limit.unwrap_or(2000) as usize;
    let end = (start + lim).min(lines.len());
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate().take(end).skip(start) {
        let l: String = l.chars().take(2000).collect();
        out.push_str(&format!("{:>5}\t{}\n", i + 1, l));
    }
    if end < lines.len() {
        out.push_str(&format!(
            "… {} more lines (use offset={} to continue)\n",
            lines.len() - end,
            end + 1
        ));
    }
    if out.is_empty() {
        out = "(empty file)".into();
    }
    Ok(out)
}

fn list_dir(p: &Path) -> Result<String> {
    let mut entries: Vec<_> = std::fs::read_dir(p)
        .with_context(|| format!("listing {}", p.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    let mut out = String::new();
    for e in entries.iter().take(500) {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let md = e.metadata().ok();
        if md.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
            out.push_str(&format!("{name}/\n"));
        } else {
            out.push_str(&format!(
                "{name}  ({} B)\n",
                md.map(|m| m.len()).unwrap_or(0)
            ));
        }
    }
    Ok(if out.is_empty() {
        "(empty)".into()
    } else {
        out
    })
}

fn search(base: &Path, pattern: &str, path: &str, glob: Option<&str>) -> Result<String> {
    let mut cmd = if which("rg") {
        let mut c = Command::new("rg");
        c.args(["-n", "--no-heading", "--color=never", "-m", "50"]);
        if let Some(g) = glob {
            c.args(["-g", g]);
        }
        c.arg("-e").arg(pattern).arg(path);
        c
    } else {
        let mut c = Command::new("grep");
        c.args(["-rnE", "--exclude-dir=.git", "--exclude-dir=target"]);
        if let Some(g) = glob {
            c.arg(format!("--include={g}"));
        }
        c.arg("-e").arg(pattern).arg(path);
        c
    };
    let out = cmd.current_dir(base).output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().take(200).collect();
    if lines.is_empty() {
        return Ok("No matches.".into());
    }
    Ok(lines
        .iter()
        .map(|l| l.chars().take(300).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Total characters `read_file` returns for several files at once.
const READ_MANY_MAX: usize = 120_000;

/// `read_file` with `files`: each file or range under a `==> path <==` header; a file that
/// can't be read says why and the others still come back.
fn read_many(base: &Path, files: &[Value]) -> String {
    let mut out = String::new();
    for f in files {
        let Some(path) = f["path"].as_str() else {
            out.push_str("==> ? <==\nError: missing `path`\n\n");
            continue;
        };
        let (off, lim) = (f["offset"].as_u64(), f["limit"].as_u64());
        let head = match (off, lim) {
            (Some(o), Some(l)) => format!("==> {path} (lines {o}–{}) <==\n", o + l.max(1) - 1),
            (Some(o), None) => format!("==> {path} (from line {o}) <==\n"),
            _ => format!("==> {path} <==\n"),
        };
        out.push_str(&head);
        match read_text(&resolve(base, path), off, lim) {
            Ok(t) => out.push_str(&t),
            Err(e) => out.push_str(&format!("Error: {e:#}\n")),
        }
        out.push('\n');
        if out.len() > READ_MANY_MAX {
            out.truncate(READ_MANY_MAX);
            out.push_str("\n… output limit reached; read the remaining files in another call\n");
            break;
        }
    }
    out
}

/// Folders `find_files` never descends into.
const SKIP_DIRS: [&str; 3] = [".git", "target", "node_modules"];
const FIND_MAX: usize = 300;

fn find_files(base: &Path, pattern: &str, path: &str) -> Result<String> {
    let root = resolve(base, path);
    let full = root.join(pattern);
    let full = full.to_string_lossy();
    let mut hits: Vec<String> = glob::glob(&full)
        .map_err(|e| anyhow!("bad pattern `{pattern}`: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|p| p.is_file())
        .filter(|p| {
            !p.strip_prefix(&root)
                .unwrap_or(p)
                .components()
                .any(|c| SKIP_DIRS.contains(&c.as_os_str().to_string_lossy().as_ref()))
        })
        .map(|p| rel(base, &p))
        .collect();
    hits.sort();
    if hits.is_empty() {
        return Ok(format!("No files match `{pattern}` under {path}."));
    }
    let n = hits.len();
    let mut out = hits.into_iter().take(FIND_MAX).collect::<Vec<_>>().join("\n");
    if n > FIND_MAX {
        out.push_str(&format!("\n… {} more; narrow the pattern", n - FIND_MAX));
    }
    Ok(out)
}

/// One replacement of `edit_file`.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    pub old: String,
    pub new: String,
    pub replace_all: bool,
}

/// The edits of an `edit_file` call: `edits`, or the single `old`/`new`.
pub fn edits_of(args: &Value) -> Result<Vec<Edit>> {
    let one = |v: &Value| -> Result<Edit> {
        Ok(Edit {
            old: arg(v, "old")?.to_string(),
            new: arg(v, "new")?.to_string(),
            replace_all: v["replace_all"].as_bool().unwrap_or(false),
        })
    };
    match args.get("edits").and_then(|e| e.as_array()) {
        Some(list) if !list.is_empty() => list.iter().map(one).collect(),
        _ => Ok(vec![one(args)?]),
    }
}

/// Applies `edits` in order. Returns the new text and, per edit, the line (1-based, in the
/// text as it was when that edit applied) where its first replacement starts.
pub fn apply_edits(text: &str, edits: &[Edit]) -> std::result::Result<(String, Vec<usize>), String> {
    let mut text = text.to_string();
    let mut lines = vec![];
    let many = edits.len() > 1;
    for (i, e) in edits.iter().enumerate() {
        let which = if many { format!("edit {}: ", i + 1) } else { String::new() };
        if e.old.is_empty() {
            return Err(format!("{which}`old` is empty"));
        }
        let n = text.matches(e.old.as_str()).count();
        if n == 0 {
            return Err(format!(
                "{which}`old` not found (read the file again and copy the exact text)"
            ));
        }
        if n > 1 && !e.replace_all {
            return Err(format!(
                "{which}`old` matches {n} times; add more context or set replace_all"
            ));
        }
        let at = text.find(e.old.as_str()).unwrap_or(0);
        lines.push(text[..at].matches('\n').count() + 1);
        text = if e.replace_all {
            text.replace(e.old.as_str(), &e.new)
        } else {
            text.replacen(e.old.as_str(), &e.new, 1)
        };
    }
    Ok((text, lines))
}

/// "line 42", "lines 42, 87".
fn edited_lines_note(lines: &[usize]) -> String {
    let list = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", ");
    if lines.len() == 1 {
        format!("line {list}")
    } else {
        format!("lines {list}")
    }
}

/// Appended to a run_command that only read, listed or edited files.
pub const FILE_TOOLS_HINT: &str = "[Tip: read_file (several files at once with `files`), \
find_files, search, edit_file (several changes with `edits`) and write_file do this without a \
shell. Use them next time: they're faster and show in the thread as reads and edits.]";

/// Programs that only read, list or edit files: work for the file tools.
const FILE_PROGRAMS: [&str; 12] = [
    "cat", "head", "tail", "sed", "grep", "rg", "egrep", "find", "ls", "wc", "nl", "awk",
];

/// Whether a shell command only reads, lists, searches or edits files (`sed -n 1,80p a.rs;
/// grep -n x b.rs | head`, `cat > f <<'EOF'`), so the file tools would have done it.
pub fn is_file_only_command(command: &str) -> bool {
    // A heredoc's body is data: look at the command line only.
    let line = if command.contains("<<") {
        command.lines().next().unwrap_or("")
    } else {
        command
    };
    let mut any = false;
    for seg in line.split(['\n', ';', '|', '&']) {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        let prog = seg
            .split_whitespace()
            .find(|w| !w.contains('='))
            .unwrap_or("");
        let prog = prog.rsplit('/').next().unwrap_or(prog);
        if !FILE_PROGRAMS.contains(&prog) {
            return false;
        }
        any = true;
    }
    any
}

/// One line for a tool call in a thread: what it read, found, ran or changed.
pub fn describe_call(name: &str, arguments: &str) -> String {
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return first_line(arguments);
    };
    let s = |k: &str| args[k].as_str().unwrap_or("").to_string();
    let range = |v: &Value| -> String {
        let p = v["path"].as_str().unwrap_or("?");
        match (v["offset"].as_u64(), v["limit"].as_u64()) {
            (Some(o), Some(l)) => format!("{p}:{o}–{}", o + l.max(1) - 1),
            (Some(o), None) => format!("{p}:{o}–"),
            (None, Some(l)) => format!("{p}:1–{l}"),
            _ => p.to_string(),
        }
    };
    match name {
        "read_file" => match args["files"].as_array().filter(|f| !f.is_empty()) {
            Some(files) => files.iter().map(range).collect::<Vec<_>>().join(", "),
            None => range(&args),
        },
        "find_files" => match args["path"].as_str().filter(|p| *p != "." && !p.is_empty()) {
            Some(p) => format!("{} in {p}", s("pattern")),
            None => s("pattern"),
        },
        "search" => {
            let mut out = format!("\"{}\"", s("pattern"));
            if let Some(p) = args["path"].as_str().filter(|p| *p != "." && !p.is_empty()) {
                out.push_str(&format!(" in {p}"));
            }
            if let Some(g) = args["glob"].as_str().filter(|g| !g.is_empty()) {
                out.push_str(&format!(" ({g})"));
            }
            out
        }
        "edit_file" => {
            let n = args["edits"].as_array().map(|e| e.len()).filter(|n| *n > 1);
            match n {
                Some(n) => format!("{} · {n} edits", s("path")),
                None => s("path"),
            }
        }
        "write_file" => format!("{} · {} lines", s("path"), s("content").lines().count()),
        "run_command" => first_line(&s("command")),
        _ => {
            let pick = ["path", "command", "pattern", "query", "title", "name", "url"]
                .iter()
                .find_map(|k| args[*k].as_str().map(first_line));
            pick.or_else(|| {
                args.as_object()
                    .and_then(|m| m.values().find_map(|v| v.as_str().map(first_line)))
            })
            .unwrap_or_default()
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

/// An `edit_file` / `write_file` call as a unified diff (for showing it in a thread).
/// `result` is the tool's reply ("Edited a.rs: lines 42, 87") and gives the line numbers.
pub fn call_diff(name: &str, arguments: &str, result: Option<&str>) -> Option<String> {
    let args: Value = serde_json::from_str(arguments).ok()?;
    let path = args["path"].as_str()?;
    let mut out = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
    match name {
        "write_file" => {
            let content = args["content"].as_str()?;
            let n = content.lines().count();
            out.push_str(&format!("@@ -0,0 +1,{n} @@\n"));
            for l in content.lines() {
                out.push_str(&format!("+{l}\n"));
            }
        }
        "edit_file" => {
            let edits = edits_of(&args).ok()?;
            let starts: Vec<usize> = result
                .and_then(|r| r.rsplit(": line").next())
                .map(|t| {
                    t.trim_start_matches('s')
                        .split(',')
                        .filter_map(|n| n.trim().parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            for (i, e) in edits.iter().enumerate() {
                let at = starts.get(i).copied().unwrap_or(1);
                let (o, n) = (e.old.lines().count(), e.new.lines().count());
                out.push_str(&format!("@@ -{at},{o} +{at},{n} @@\n"));
                for l in e.old.lines() {
                    out.push_str(&format!("-{l}\n"));
                }
                for l in e.new.lines() {
                    out.push_str(&format!("+{l}\n"));
                }
            }
        }
        _ => return None,
    }
    Some(out)
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Runs `command` (wrapped by `shell` if set) with a timeout; output truncated head+tail.
/// stdout and stderr are merged. On timeout/cancel the whole process group is killed.
pub fn run_command(
    cwd: &Path,
    shell: &str,
    env: &BTreeMap<String, String>,
    command: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String> {
    use std::os::unix::process::CommandExt;
    let full = if shell.trim().is_empty() {
        command.to_string()
    } else {
        format!("{} {}", shell.trim(), shell_quote(command))
    };
    let mut child = Command::new("sh")
        .arg("-c")
        // A group redirect covers every command in the line (and survives trailing comments).
        .arg(format!("{{ {full}\n}} 2>&1"))
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| {
            (
                k.clone(),
                harness_core::config::expand_tilde(v).display().to_string(),
            )
        }))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("spawning command")?;
    let pgid = child.id();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        // Keep the head and a rolling tail so runaway output can't exhaust memory.
        const HEAD: usize = 64 * 1024;
        const TAIL: usize = 256 * 1024;
        let mut head = Vec::new();
        let mut tail: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
        let mut dropped = 0usize;
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if head.len() < HEAD {
                            head.push(b);
                        } else {
                            tail.push_back(b);
                            if tail.len() > TAIL {
                                tail.pop_front();
                                dropped += 1;
                            }
                        }
                    }
                }
            }
        }
        if dropped > 0 {
            head.extend_from_slice(format!("\n… [{dropped} bytes omitted] …\n").as_bytes());
        }
        head.extend(tail);
        let _ = tx.send(head);
    });
    let kill_group = |sig: &str| {
        let _ = Command::new("kill")
            .args([sig, "--", &format!("-{pgid}")])
            .stderr(Stdio::null())
            .status();
    };
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if cancel.load(Ordering::SeqCst) || start.elapsed() > timeout {
            kill_group("-TERM");
            std::thread::sleep(Duration::from_millis(500));
            kill_group("-KILL");
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // Background processes may hold the pipe open: don't wait for them forever.
    let buf = rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default();
    let text = clean_terminal_output(&String::from_utf8_lossy(&buf));
    let text = truncate_middle(&text, 16_000);
    match status {
        Some(s) => Ok(format!("exit code {}\n{}", s.code().unwrap_or(-1), text)),
        None if cancel.load(Ordering::SeqCst) => bail!("cancelled"),
        None => Ok(format!("timed out after {}s\n{}", timeout.as_secs(), text)),
    }
}

/// What a terminal would show: drops ANSI escape sequences (colours, cursor moves,
/// hide/show cursor, OSC titles) and other control characters, and keeps only the last
/// state of a line redrawn with `\r` (spinners, progress bars). CLIs print these even
/// without a TTY; raw, they showed up as "[?25l[1G[J" in errors and agent results.
pub fn clean_terminal_output(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut line = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // CSI: ESC [ params… final byte in @..~
                Some('[') => {
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] … BEL or ESC \
                Some(']') => {
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Two-character escapes (ESC 7, ESC =, ...).
                _ => {}
            },
            '\r' if chars.peek() == Some(&'\n') => {}
            // A bare carriage return starts the line over.
            '\r' => line.clear(),
            '\n' => {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
            }
            '\t' => line.push('\t'),
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
    out.push_str(&line);
    out
}

pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max / 3).collect();
    let tail_chars: Vec<char> = s.chars().rev().take(max * 2 / 3).collect();
    let tail: String = tail_chars.into_iter().rev().collect();
    format!("{head}\n… [{} bytes omitted] …\n{tail}", s.len() - max)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch folder for one test.
    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pl-tools-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: Some(dir.to_path_buf()),
            shell: String::new(),
            env: Default::default(),
            access: ToolAccess::Full,
            workdirs: vec![],
            skills_on_demand: vec![],
            config: Config::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn ctx_with(cwd: Option<PathBuf>, access: ToolAccess) -> ToolContext {
        ToolContext {
            cwd,
            access,
            ..ctx(Path::new("."))
        }
    }

    fn spec_names(c: &ToolContext) -> Vec<&'static str> {
        specs(c).iter().map(|s| s.name).collect()
    }

    #[test]
    fn specs_follow_access_and_cwd() {
        let d = tmp("specs");
        let with = |a| spec_names(&ctx_with(Some(d.clone()), a));
        let without = |a| spec_names(&ctx_with(None, a));
        let reads = ["read_file", "list_dir", "find_files", "search"];
        assert!(with(ToolAccess::None).is_empty());
        assert!(without(ToolAccess::None).is_empty());
        assert_eq!(with(ToolAccess::ReadOnly), reads);
        assert!(without(ToolAccess::ReadOnly).is_empty());
        let mut rr = reads.to_vec();
        rr.push("run_command");
        assert_eq!(with(ToolAccess::ReadAndRun), rr);
        assert_eq!(without(ToolAccess::ReadAndRun), ["run_command"]);
        let mut full = rr.clone();
        full.extend(["write_file", "edit_file"]);
        assert_eq!(with(ToolAccess::Full), full);
        assert_eq!(without(ToolAccess::Full), ["run_command"]);
        let none = specs(&ctx_with(None, ToolAccess::Full));
        assert!(none[0].description.contains("home directory"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn run_command_without_a_working_folder() {
        for access in [ToolAccess::ReadAndRun, ToolAccess::Full] {
            let c = ctx_with(None, access);
            let out = execute(&c, "run_command", &json!({"command":"cat /dev/null; echo ok-$((1+1))"}));
            assert!(out.contains("exit code 0") && out.contains("ok-2"), "{out}");
            assert!(!out.contains(FILE_TOOLS_HINT), "{out}");
        }
        for access in [ToolAccess::None, ToolAccess::ReadOnly] {
            let out = execute(
                &ctx_with(None, access),
                "run_command",
                &json!({"command":"echo hi"}),
            );
            assert!(out.starts_with("Error:") && out.contains("not allowed"), "{out}");
        }
    }

    #[test]
    fn terminal_noise_is_removed() {
        let raw = "\u{1b}[?25l\u{1b}[1G\u{1b}[J\u{1b}[32m✓\u{1b}[39m Selected 1 skill\n\
                   \u{1b}]0;title\u{7}loading 10%\rloading 100%\r\ndone\u{8}\tok\n";
        assert_eq!(
            clean_terminal_output(raw),
            "✓ Selected 1 skill\nloading 100%\ndone\tok\n"
        );
        assert_eq!(clean_terminal_output("plain\ntext"), "plain\ntext");
    }

    #[test]
    fn reading_an_image_marks_it_for_the_model() {
        let d = std::env::temp_dir().join(format!("pl-img-{}", harness_core::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        std::fs::write(d.join("shot.png"), &png).unwrap();
        std::fs::write(d.join("a.txt"), "hello").unwrap();
        let c = ctx(&d);
        let one = execute(&c, "read_file", &json!({"path":"shot.png"}));
        assert!(one.contains("image/png, 640×480"), "{one}");
        assert_eq!(marked_images(&one), vec![d.join("shot.png").canonicalize().unwrap()]);
        let many = execute(
            &c,
            "read_file",
            &json!({"files":[{"path":"a.txt"},{"path":"shot.png"}]}),
        );
        assert!(many.contains("hello"));
        assert_eq!(marked_images(&many).len(), 1);
        assert!(marked_images("plain text").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_tools() {
        let dir = std::env::temp_dir().join(format!("pl-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = ctx(&dir);
        assert!(execute(
            &c,
            "write_file",
            &json!({"path":"a/b.txt","content":"hello\nworld\n"})
        )
        .starts_with("Wrote"));
        assert!(execute(&c, "read_file", &json!({"path":"a/b.txt"})).contains("world"));
        assert!(execute(
            &c,
            "edit_file",
            &json!({"path":"a/b.txt","old":"world","new":"there"})
        )
        .starts_with("Edited"));
        assert!(std::fs::read_to_string(dir.join("a/b.txt"))
            .unwrap()
            .contains("there"));
        assert!(
            execute(&c, "write_file", &json!({"path":"../x.txt","content":""}))
                .starts_with("Error")
        );
        let out = execute(&c, "run_command", &json!({"command":"echo hi"}));
        assert!(out.contains("exit code 0") && out.contains("hi"), "{out}");
        let out = execute(
            &c,
            "run_command",
            &json!({"command":"echo err1 >&2; echo err2 >&2 # note"}),
        );
        assert!(out.contains("err1") && out.contains("err2"), "{out}");
        let t = Instant::now();
        let out = execute(
            &c,
            "run_command",
            &json!({"command":"sleep 30 & sleep 60","timeout_secs":5}),
        );
        assert!(
            out.starts_with("timed out") && t.elapsed() < Duration::from_secs(15),
            "{out}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_file_reads_several_files_and_ranges() {
        let d = tmp("readmany");
        std::fs::write(d.join("a.txt"), "a1\na2\na3\n").unwrap();
        std::fs::write(d.join("b.txt"), "b1\nb2\n").unwrap();
        let out = execute(
            &ctx(&d),
            "read_file",
            &json!({"files":[{"path":"a.txt","offset":2,"limit":1},{"path":"b.txt"},{"path":"nope.txt"}]}),
        );
        assert!(out.contains("==> a.txt (lines 2–2) <=="), "{out}");
        assert!(out.contains("    2\ta2") && !out.contains("a1"), "{out}");
        assert!(out.contains("==> b.txt <==") && out.contains("    2\tb2"), "{out}");
        assert!(out.contains("==> nope.txt <==\nError:"), "{out}");
        // The single-file form still works.
        assert!(execute(&ctx(&d), "read_file", &json!({"path":"b.txt"})).contains("b1"));
    }

    #[test]
    fn find_files_globs_recursively_and_skips_build_dirs() {
        let d = tmp("find");
        for f in ["src/a.rs", "src/x/b.rs", "target/debug/c.rs", "README.md"] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let out = execute(&ctx(&d), "find_files", &json!({"pattern":"**/*.rs"}));
        assert_eq!(out, "src/a.rs\nsrc/x/b.rs");
        let out = execute(&ctx(&d), "find_files", &json!({"pattern":"*.txt"}));
        assert!(out.starts_with("No files match"), "{out}");
    }

    #[test]
    fn edit_file_applies_several_edits_or_none() {
        let d = tmp("edits");
        std::fs::write(d.join("f.rs"), "one\ntwo\nthree\n").unwrap();
        let out = execute(
            &ctx(&d),
            "edit_file",
            &json!({"path":"f.rs","edits":[{"old":"two","new":"TWO"},{"old":"three","new":"3"}]}),
        );
        assert_eq!(out, "Edited f.rs: lines 2, 3");
        assert_eq!(std::fs::read_to_string(d.join("f.rs")).unwrap(), "one\nTWO\n3\n");
        // A failing edit leaves the file as it was.
        let out = execute(
            &ctx(&d),
            "edit_file",
            &json!({"path":"f.rs","edits":[{"old":"one","new":"1"},{"old":"missing","new":"x"}]}),
        );
        assert!(out.starts_with("Error: edit 2: `old` not found"), "{out}");
        assert!(out.contains("nothing was written"), "{out}");
        assert_eq!(std::fs::read_to_string(d.join("f.rs")).unwrap(), "one\nTWO\n3\n");
        // Single old/new form.
        let out = execute(&ctx(&d), "edit_file", &json!({"path":"f.rs","old":"one","new":"1"}));
        assert_eq!(out, "Edited f.rs: line 1");
    }

    #[test]
    fn spots_shell_commands_that_only_touch_files() {
        for c in [
            "sed -n 1,130p crates/engine/src/chat.rs; grep -n \"name\" tools.rs | head -30",
            "cat > crates/engine/src/skill_install.rs <<'EOF'\nfn main() {}\nEOF",
            "sed -i 's/a/b/' x.rs",
            "find . -name '*.rs' | wc -l",
        ] {
            assert!(is_file_only_command(c), "{c}");
        }
        for c in [
            "cargo test -p harness-engine",
            "git diff --stat",
            "cat Cargo.toml && cargo build",
            "python3 - <<'EOF'\nprint(1)\nEOF",
            "",
        ] {
            assert!(!is_file_only_command(c), "{c}");
        }
    }

    #[test]
    fn describes_calls_and_turns_edits_into_diffs() {
        assert_eq!(
            describe_call("read_file", r#"{"files":[{"path":"a.rs","offset":10,"limit":5},{"path":"b.rs"}]}"#),
            "a.rs:10–14, b.rs"
        );
        assert_eq!(describe_call("read_file", r#"{"path":"a.rs"}"#), "a.rs");
        assert_eq!(
            describe_call("search", r#"{"pattern":"fn x","path":"crates","glob":"*.rs"}"#),
            "\"fn x\" in crates (*.rs)"
        );
        assert_eq!(
            describe_call("edit_file", r#"{"path":"a.rs","edits":[{"old":"a","new":"b"},{"old":"c","new":"d"}]}"#),
            "a.rs · 2 edits"
        );
        assert_eq!(describe_call("run_command", r#"{"command":"cargo test\necho"}"#), "cargo test");
        let d = call_diff(
            "edit_file",
            r#"{"path":"a.rs","edits":[{"old":"x","new":"y\nz"},{"old":"p","new":"q"}]}"#,
            Some("Edited a.rs: lines 4, 9"),
        )
        .unwrap();
        assert!(d.contains("@@ -4,1 +4,2 @@\n-x\n+y\n+z\n"), "{d}");
        assert!(d.contains("@@ -9,1 +9,1 @@\n-p\n+q\n"), "{d}");
        let w = call_diff("write_file", r#"{"path":"n.rs","content":"a\nb\n"}"#, None).unwrap();
        assert!(w.contains("@@ -0,0 +1,2 @@\n+a\n+b\n"), "{w}");
        assert_eq!(call_diff("read_file", r#"{"path":"a.rs"}"#, None), None);
    }
}
