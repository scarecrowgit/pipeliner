//! Stdio transport and JSON-RPC session shared between a [`crate::Client`] and its reader
//! threads.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Child, ChildStdin, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use serde_json::{Value, json};

use crate::{CallResult, McpTool, ServerConfig};

pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";
const STDERR_CAP: usize = 4096;
const MAX_RESULT_BYTES: usize = 100 * 1024;
const POLL: Duration = Duration::from_millis(50);
const MAX_PAGES: usize = 1000;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MCP error {}: {}", self.code, self.message)?;
        if let Some(d) = &self.data {
            let d = d.to_string();
            write!(f, " ({})", truncate_chars(&d, 500))?;
        }
        Ok(())
    }
}

type Reply = Result<Value, RpcError>;

enum Wait {
    Reply(Reply),
    Cancelled,
    TimedOut,
}

#[derive(Default)]
struct Pending {
    /// Set by the reader thread at EOF; no new waiters are registered afterwards.
    closed: bool,
    waiters: HashMap<u64, Sender<Reply>>,
}

pub(crate) struct Shared {
    name: String,
    pid: u32,
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    stderr: Mutex<Vec<u8>>,
    stderr_done: AtomicBool,
    shut_down: AtomicBool,
}

impl Shared {
    pub(crate) fn start(
        config: &ServerConfig,
        timeout: Duration,
    ) -> anyhow::Result<(Arc<Shared>, (String, String))> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().with_context(|| {
            let mut msg = format!("failed to start MCP server `{}` (`{}`", config.name, config.command);
            if let Some(cwd) = &config.cwd {
                msg.push_str(&format!(" in {}", cwd.display()));
            }
            msg.push(')');
            msg
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let shared = Arc::new(Shared {
            name: config.name.clone(),
            pid: child.id(),
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending: Mutex::new(Pending::default()),
            next_id: AtomicU64::new(1),
            stderr: Mutex::new(Vec::new()),
            stderr_done: AtomicBool::new(false),
            shut_down: AtomicBool::new(false),
        });
        spawn_thread(format!("mcp-{}-stdout", config.name), {
            let s = shared.clone();
            move || reader_loop(s, stdout)
        });
        spawn_thread(format!("mcp-{}-stderr", config.name), {
            let s = shared.clone();
            move || stderr_loop(s, stderr)
        });

        match shared.handshake(timeout) {
            Ok(info) => Ok((shared, info)),
            Err(e) => {
                shared.shutdown();
                Err(e)
            }
        }
    }

    fn handshake(&self, timeout: Duration) -> anyhow::Result<(String, String)> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "harness", "version": env!("CARGO_PKG_VERSION")},
        });
        let (_, wait) = self.request("initialize", params, timeout, None)?;
        let result = match wait {
            Wait::Reply(Ok(v)) => v,
            Wait::Reply(Err(e)) => return Err(self.with_tail(format!("`initialize` failed: {e}"))),
            Wait::TimedOut | Wait::Cancelled => {
                return Err(self.with_tail(format!(
                    "did not answer `initialize` within {}",
                    fmt_duration(timeout)
                )));
            }
        };
        // Any protocolVersion the server answers is accepted.
        let info = result.get("serverInfo");
        let field = |k: &str| {
            info.and_then(|i| i.get(k)).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        let (name, version) = (field("name"), field("version"));
        self.notify("notifications/initialized", None)?;
        Ok((name, version))
    }

    pub(crate) fn list_tools(&self, timeout: Duration) -> anyhow::Result<Vec<McpTool>> {
        let deadline = Instant::now() + timeout;
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let left = deadline.saturating_duration_since(Instant::now());
            let (_, wait) = self.request("tools/list", params, left, None)?;
            let result = match wait {
                Wait::Reply(Ok(v)) => v,
                Wait::Reply(Err(e)) => bail!("MCP server `{}`: `tools/list` failed: {e}", self.name),
                Wait::TimedOut | Wait::Cancelled => {
                    return Err(self.with_tail(format!(
                        "did not answer `tools/list` within {}",
                        fmt_duration(timeout)
                    )));
                }
            };
            for t in result.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = t.get("name").and_then(Value::as_str) else { continue };
                tools.push(McpTool {
                    name: name.to_string(),
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .filter(|s| s.is_object())
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                });
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() && cursor.as_deref() != Some(next) => {
                    cursor = Some(next.to_string())
                }
                _ => return Ok(tools),
            }
        }
        Ok(tools)
    }

    pub(crate) fn call_tool(
        &self,
        tool: &str,
        arguments: Value,
        cancel: &AtomicBool,
        timeout: Duration,
    ) -> anyhow::Result<CallResult> {
        let arguments = if arguments.is_null() { json!({}) } else { arguments };
        let params = json!({ "name": tool, "arguments": arguments });
        let (id, wait) = self.request("tools/call", params, timeout, Some(cancel))?;
        Ok(match wait {
            Wait::Reply(Ok(v)) => format_call_result(&v),
            Wait::Reply(Err(e)) => CallResult { text: e.to_string(), is_error: true },
            Wait::Cancelled => {
                self.notify_cancelled(id, "cancelled by the user");
                CallResult { text: format!("MCP tool `{tool}` was cancelled."), is_error: true }
            }
            Wait::TimedOut => {
                let t = fmt_duration(timeout);
                self.notify_cancelled(id, &format!("timed out after {t}"));
                CallResult {
                    text: format!("MCP tool `{tool}` timed out after {t} and was cancelled."),
                    is_error: true,
                }
            }
        })
    }

    pub(crate) fn is_alive(&self) -> bool {
        if self.shut_down.load(Ordering::SeqCst) || lock(&self.pending).closed {
            return false;
        }
        matches!(lock(&self.child).try_wait(), Ok(None))
    }

    pub(crate) fn stderr_tail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.stderr)).into_owned()
    }

    pub(crate) fn shutdown(&self) {
        if self.shut_down.swap(true, Ordering::SeqCst) {
            return;
        }
        // Closing stdin asks a well-behaved server to exit. If a writer is stuck holding the
        // lock (server not reading), go straight to signals; they unblock the writer.
        let closed_stdin = match self.stdin.try_lock() {
            Ok(mut g) => {
                g.take();
                true
            }
            Err(TryLockError::Poisoned(e)) => {
                e.into_inner().take();
                true
            }
            Err(TryLockError::WouldBlock) => false,
        };
        if closed_stdin && self.wait_exit(Duration::from_secs(1)) {
            // The leader is gone; clean up anything it left behind in its group.
            self.kill_group("TERM");
            return;
        }
        self.kill_group("TERM");
        if self.wait_exit(Duration::from_millis(500)) {
            return;
        }
        self.kill_group("KILL");
        let mut child = lock(&self.child);
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Poll for the child's exit for up to `d`. Returns true once it has exited (and is reaped).
    fn wait_exit(&self, d: Duration) -> bool {
        let deadline = Instant::now() + d;
        loop {
            match lock(&self.child).try_wait() {
                Ok(Some(_)) | Err(_) => return true,
                Ok(None) => {}
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill_group(&self, signal: &str) {
        #[cfg(unix)]
        {
            let ok = Command::new("kill")
                .arg(format!("-{signal}"))
                .arg("--")
                .arg(format!("-{}", self.pid))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok();
            if ok {
                return;
            }
        }
        let _ = signal;
        // No `kill` binary (or not unix): at least kill the direct child.
        let mut child = lock(&self.child);
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
        }
    }

    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&AtomicBool>,
    ) -> anyhow::Result<(u64, Wait)> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut p = lock(&self.pending);
            if p.closed || self.shut_down.load(Ordering::SeqCst) {
                drop(p);
                return Err(self.dead_error("is not running"));
            }
            p.waiters.insert(id, tx);
        }
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(e) = self.send(&msg) {
            self.forget(id);
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                self.forget(id);
                return Ok((id, Wait::Cancelled));
            }
            let now = Instant::now();
            if now >= deadline {
                self.forget(id);
                return Ok((id, Wait::TimedOut));
            }
            let left = deadline - now;
            let step = if cancel.is_some() { left.min(POLL) } else { left };
            match rx.recv_timeout(step) {
                Ok(reply) => return Ok((id, Wait::Reply(reply))),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.dead_error(&format!("closed the connection during `{method}`")));
                }
            }
        }
    }

    fn forget(&self, id: u64) {
        lock(&self.pending).waiters.remove(&id);
    }

    fn notify(&self, method: &str, params: Option<Value>) -> anyhow::Result<()> {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.send(&msg)
    }

    fn notify_cancelled(&self, id: u64, reason: &str) {
        let _ = self.notify(
            "notifications/cancelled",
            Some(json!({"requestId": id, "reason": reason})),
        );
    }

    fn send(&self, msg: &Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        let res = {
            let mut guard = lock(&self.stdin);
            match guard.as_mut() {
                None => Err(None),
                Some(stdin) => stdin
                    .write_all(line.as_bytes())
                    .and_then(|_| stdin.flush())
                    .map_err(Some),
            }
        };
        match res {
            Ok(()) => Ok(()),
            Err(None) => Err(self.dead_error("is not running (stdin closed)")),
            Err(Some(e)) => Err(self.dead_error(&format!("is not accepting input ({e})"))),
        }
    }

    /// Error for a dead transport: exit status (if known) and the stderr tail.
    fn dead_error(&self, what: &str) -> anyhow::Error {
        // Give the stderr thread a moment to drain what the process printed before dying.
        let deadline = Instant::now() + Duration::from_millis(300);
        while !self.stderr_done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = match lock(&self.child).try_wait() {
            Ok(Some(st)) => format!(" (exited: {st})"),
            _ => String::new(),
        };
        self.with_tail(format!("{what}{status}"))
    }

    fn with_tail(&self, msg: String) -> anyhow::Error {
        let tail = self.stderr_tail();
        let tail = tail.trim();
        if tail.is_empty() {
            anyhow!("MCP server `{}` {msg}", self.name)
        } else {
            anyhow!("MCP server `{}` {msg}\n--- stderr ---\n{tail}", self.name)
        }
    }

    fn push_stderr(&self, bytes: &[u8]) {
        push_capped(&mut lock(&self.stderr), bytes);
    }

    /// A harness-side warning, interleaved with the server's stderr.
    fn note(&self, msg: &str) {
        let mut buf = lock(&self.stderr);
        if buf.last().is_some_and(|b| *b != b'\n') {
            push_capped(&mut buf, b"\n");
        }
        push_capped(&mut buf, format!("[harness] {msg}\n").as_bytes());
    }
}

fn push_capped(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(bytes);
    if buf.len() > STDERR_CAP {
        let excess = buf.len() - STDERR_CAP;
        buf.drain(..excess);
    }
}

fn spawn_thread(name: String, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name)
        .spawn(f)
        .expect("spawning an MCP I/O thread");
}

fn reader_loop(shared: Arc<Shared>, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(items)) => items.into_iter().for_each(|m| handle_message(&shared, m)),
            Ok(msg) => handle_message(&shared, msg),
            Err(_) => shared.note(&format!(
                "ignored non-JSON line on stdout: {}",
                truncate_chars(line, 200)
            )),
        }
    }
    let mut p = lock(&shared.pending);
    p.closed = true;
    p.waiters.clear(); // dropping the senders wakes every waiter with `Disconnected`
}

fn handle_message(shared: &Arc<Shared>, msg: Value) {
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").filter(|id| !id.is_null());
    match (method, id) {
        // Server -> client request.
        (Some(method), Some(id)) => {
            let reply = if method == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {
                    "code": -32601,
                    "message": format!("Method not found: {method}"),
                }})
            };
            // Reply off the reader thread so a blocked stdin can never stall response routing.
            let s = shared.clone();
            spawn_thread(format!("mcp-{}-reply", shared.name), move || {
                let _ = s.send(&reply);
            });
        }
        // Notification (log, progress, list_changed, ...): ignored.
        (Some(_), None) => {}
        // Response to one of our requests.
        (None, Some(id)) => {
            let Some(id) = id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok())) else {
                shared.note(&format!("response with unknown id {id}"));
                return;
            };
            let reply = match msg.get("error") {
                Some(e) => Err(RpcError {
                    code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: e
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                    data: e.get("data").filter(|d| !d.is_null()).cloned(),
                }),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            if let Some(tx) = lock(&shared.pending).waiters.remove(&id) {
                let _ = tx.send(reply);
            }
        }
        (None, None) => {
            if let Some(e) = msg.get("error") {
                shared.note(&format!("server reported an error: {e}"));
            }
        }
    }
}

fn stderr_loop(shared: Arc<Shared>, mut stderr: ChildStderr) {
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => shared.push_stderr(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    shared.stderr_done.store(true, Ordering::SeqCst);
}

/// Turn a `tools/call` result into text. See [`CallResult`].
pub(crate) fn format_call_result(v: &Value) -> CallResult {
    let mut parts: Vec<String> = Vec::new();
    let mut has_text = false;
    for block in v.get("content").and_then(Value::as_array).into_iter().flatten() {
        let ty = block.get("type").and_then(Value::as_str).unwrap_or("unknown");
        let s = |k: &str| block.get(k).and_then(Value::as_str);
        match ty {
            "text" => {
                has_text = true;
                parts.push(s("text").unwrap_or_default().to_string());
            }
            "image" | "audio" => parts.push(format!(
                "[{ty}: {}, {}]",
                s("mimeType").unwrap_or("unknown type"),
                fmt_size(base64_len(s("data").unwrap_or_default()))
            )),
            "resource" => {
                let r = block.get("resource");
                let rs = |k: &str| r.and_then(|r| r.get(k)).and_then(Value::as_str);
                let uri = rs("uri").unwrap_or("?");
                if let Some(text) = rs("text") {
                    has_text = true;
                    parts.push(format!("[resource: {uri}]\n{text}"));
                } else {
                    let mime = rs("mimeType").unwrap_or("unknown type");
                    let size = fmt_size(base64_len(rs("blob").unwrap_or_default()));
                    parts.push(format!("[resource: {uri}, {mime}, {size}]"));
                }
            }
            "resource_link" => {
                let uri = s("uri").unwrap_or("?");
                match s("name") {
                    Some(name) if name != uri => parts.push(format!("[resource link: {name} ({uri})]")),
                    _ => parts.push(format!("[resource link: {uri}]")),
                }
            }
            other => parts.push(format!("[{other} content]")),
        }
    }
    if !has_text
        && let Some(sc) = v.get("structuredContent").filter(|s| !s.is_null())
    {
        parts.push(serde_json::to_string_pretty(sc).unwrap_or_else(|_| sc.to_string()));
    }
    let mut text = parts.join("\n\n");
    if text.is_empty() {
        text = "(no content)".to_string();
    }
    CallResult {
        text: truncate_result(text),
        is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn truncate_result(mut text: String) -> String {
    if text.len() <= MAX_RESULT_BYTES {
        return text;
    }
    let total = text.len();
    let mut cut = MAX_RESULT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text.push_str(&format!(
        "\n\n[output truncated: showing the first {} of {}]",
        fmt_size(cut),
        fmt_size(total)
    ));
    text
}

fn base64_len(data: &str) -> usize {
    let trimmed = data.trim_end_matches('=');
    trimmed.len() * 3 / 4
}

fn fmt_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 || !ms.is_multiple_of(1000) { format!("{ms} ms") } else { format!("{} s", ms / 1000) }
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_content_blocks() {
        let r = format_call_result(&json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "image", "data": "A".repeat(4096), "mimeType": "image/png"},
                {"type": "audio", "data": "AAAA", "mimeType": "audio/wav"},
                {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "file body"}},
                {"type": "resource", "resource": {"uri": "file:///b.bin", "blob": "AAAA", "mimeType": "application/octet-stream"}},
                {"type": "resource_link", "uri": "file:///c", "name": "c"},
            ],
            "structuredContent": {"ignored": true},
        }));
        assert!(!r.is_error);
        assert_eq!(
            r.text,
            "hello\n\n[image: image/png, 3 KB]\n\n[audio: audio/wav, 3 B]\n\n[resource: file:///a.txt]\nfile body\n\n[resource: file:///b.bin, application/octet-stream, 3 B]\n\n[resource link: c (file:///c)]"
        );
    }

    #[test]
    fn structured_content_and_errors() {
        let r = format_call_result(&json!({
            "content": [], "structuredContent": {"answer": 42}, "isError": true
        }));
        assert!(r.is_error);
        assert_eq!(r.text, "{\n  \"answer\": 42\n}");
        assert_eq!(format_call_result(&json!({})).text, "(no content)");
    }

    #[test]
    fn truncates_huge_results() {
        let big = "é".repeat(80 * 1024); // 160 KB
        let r = format_call_result(&json!({"content": [{"type": "text", "text": big}]}));
        assert!(r.text.len() < MAX_RESULT_BYTES + 200);
        assert!(r.text.contains("[output truncated"), "{}", &r.text[r.text.len() - 100..]);
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration(Duration::from_millis(300)), "300 ms");
        assert_eq!(fmt_duration(Duration::from_secs(30)), "30 s");
    }
}
