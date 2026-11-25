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
    ToolSpec { name, description: description.to_string(), parameters }
}

/// Tool specs for this context's access level.
pub fn specs(ctx: &ToolContext) -> Vec<ToolSpec> {
    let mut v = vec![];
    if ctx.access == ToolAccess::None {
        return v;
    }
    if ctx.cwd.is_some() {
        v.push(spec(
            "read_file",
            "Read a text file. Paths are relative to the working directory. Use offset/limit (lines) for big files.",
            json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer"},"limit":{"type":"integer"}},"required":["path"]}),
        ));
        v.push(spec(
            "list_dir",
            "List a directory (default: the working directory). Shows sizes; directories end with /.",
            json!({"type":"object","properties":{"path":{"type":"string"}}}),
        ));
        v.push(spec(
            "search",
            "Search file contents with a regular expression (ripgrep/grep). Returns file:line: text.",
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
    if ctx.cwd.is_some() && matches!(ctx.access, ToolAccess::ReadAndRun | ToolAccess::Full) {
        v.push(spec(
            "run_command",
            "Run a shell command in the working directory. Returns exit code and output (truncated). Default timeout 600s.",
            json!({"type":"object","properties":{"command":{"type":"string"},"timeout_secs":{"type":"integer"}},"required":["command"]}),
        ));
    }
    if ctx.cwd.is_some() && ctx.access == ToolAccess::Full {
        v.push(spec(
            "write_file",
            "Create or overwrite a file with the given content.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ));
        v.push(spec(
            "edit_file",
            "Replace an exact snippet in a file. `old` must match exactly once unless replace_all is true. Prefer this for small changes.",
            json!({"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old","new"]}),
        ));
    }
    v
}

fn arg<'a>(args: &'a Value, k: &str) -> Result<&'a str> {
    args.get(k).and_then(|v| v.as_str()).ok_or_else(|| anyhow!("missing `{k}`"))
}

fn resolve(base: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(harness_core::config::expand_tilde(p));
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
    let cwd = || ctx.cwd.clone().ok_or_else(|| anyhow!("no working directory"));
    match name {
        "read_file" => {
            let p = resolve(&cwd()?, arg(args, "path")?);
            read_text(&p, args["offset"].as_u64(), args["limit"].as_u64())
        }
        "list_dir" => {
            let base = cwd()?;
            let p = args["path"].as_str().map(|s| resolve(&base, s)).unwrap_or(base);
            list_dir(&p)
        }
        "search" => {
            let base = cwd()?;
            search(&base, arg(args, "pattern")?, args["path"].as_str().unwrap_or("."), args["glob"].as_str())
        }
        "read_workdir" => {
            let id = arg(args, "workdir")?;
            let (_, root) = ctx
                .workdirs
                .iter()
                .find(|(i, _)| i == id)
                .ok_or_else(|| anyhow!("unknown workdir `{id}`; known: {}", ctx.workdirs.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(", ")))?;
            let root = PathBuf::from(root);
            let p = args["path"].as_str().map(|s| resolve(&root, s)).unwrap_or(root);
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
            run_command(&cwd()?, &ctx.shell, &ctx.env, arg(args, "command")?, Duration::from_secs(timeout), &ctx.cancel)
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
            Ok(format!("Wrote {} ({} lines)", rel(&base, &p), content.lines().count()))
        }
        "edit_file" => {
            ensure_write(ctx)?;
            let base = cwd()?;
            let p = normalize(&resolve(&base, arg(args, "path")?));
            ensure_inside(&base, &p)?;
            let text = std::fs::read_to_string(&p).with_context(|| format!("reading {}", rel(&base, &p)))?;
            let old = arg(args, "old")?;
            let new = arg(args, "new")?;
            if old.is_empty() {
                bail!("`old` is empty");
            }
            let n = text.matches(old).count();
            let all = args["replace_all"].as_bool().unwrap_or(false);
            if n == 0 {
                bail!("`old` not found in {}; read the file again and copy the exact text", rel(&base, &p));
            }
            if n > 1 && !all {
                bail!("`old` matches {n} times in {}; add more context or set replace_all", rel(&base, &p));
            }
            let out = if all { text.replace(old, new) } else { text.replacen(old, new, 1) };
            std::fs::write(&p, out)?;
            Ok(format!("Edited {} ({n} replacement{})", rel(&base, &p), if n == 1 { "" } else { "s" }))
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
        match (existing.file_name().map(|f| f.to_owned()), existing.parent()) {
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

fn read_text(p: &Path, offset: Option<u64>, limit: Option<u64>) -> Result<String> {
    let bytes = std::fs::read(p).with_context(|| format!("reading {}", p.display()))?;
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
        out.push_str(&format!("… {} more lines (use offset={} to continue)\n", lines.len() - end, end + 1));
    }
    if out.is_empty() {
        out = "(empty file)".into();
    }
    Ok(out)
}

fn list_dir(p: &Path) -> Result<String> {
    let mut entries: Vec<_> = std::fs::read_dir(p).with_context(|| format!("listing {}", p.display()))?.filter_map(|e| e.ok()).collect();
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
            out.push_str(&format!("{name}  ({} B)\n", md.map(|m| m.len()).unwrap_or(0)));
        }
    }
    Ok(if out.is_empty() { "(empty)".into() } else { out })
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
    Ok(lines.iter().map(|l| l.chars().take(300).collect::<String>()).collect::<Vec<_>>().join("\n"))
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
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
    let full = if shell.trim().is_empty() { command.to_string() } else { format!("{} {}", shell.trim(), shell_quote(command)) };
    let mut child = Command::new("sh")
        .arg("-c")
        // A group redirect covers every command in the line (and survives trailing comments).
        .arg(format!("{{ {full}\n}} 2>&1"))
        .current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k.clone(), harness_core::config::expand_tilde(v).display().to_string())))
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
        let _ = Command::new("kill").args([sig, "--", &format!("-{pgid}")]).stderr(Stdio::null()).status();
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
    let text = String::from_utf8_lossy(&buf).to_string();
    let text = truncate_middle(&text, 16_000);
    match status {
        Some(s) => Ok(format!("exit code {}\n{}", s.code().unwrap_or(-1), text)),
        None if cancel.load(Ordering::SeqCst) => bail!("cancelled"),
        None => Ok(format!("timed out after {}s\n{}", timeout.as_secs(), text)),
    }
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

    #[test]
    fn file_tools() {
        let dir = std::env::temp_dir().join(format!("pl-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = ctx(&dir);
        assert!(execute(&c, "write_file", &json!({"path":"a/b.txt","content":"hello\nworld\n"})).starts_with("Wrote"));
        assert!(execute(&c, "read_file", &json!({"path":"a/b.txt"})).contains("world"));
        assert!(execute(&c, "edit_file", &json!({"path":"a/b.txt","old":"world","new":"there"})).starts_with("Edited"));
        assert!(std::fs::read_to_string(dir.join("a/b.txt")).unwrap().contains("there"));
        assert!(execute(&c, "write_file", &json!({"path":"../x.txt","content":""})).starts_with("Error"));
        let out = execute(&c, "run_command", &json!({"command":"echo hi"}));
        assert!(out.contains("exit code 0") && out.contains("hi"), "{out}");
        let out = execute(&c, "run_command", &json!({"command":"echo err1 >&2; echo err2 >&2 # note"}));
        assert!(out.contains("err1") && out.contains("err2"), "{out}");
        let t = Instant::now();
        let out = execute(&c, "run_command", &json!({"command":"sleep 30 & sleep 60","timeout_secs":5}));
        assert!(out.starts_with("timed out") && t.elapsed() < Duration::from_secs(15), "{out}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
