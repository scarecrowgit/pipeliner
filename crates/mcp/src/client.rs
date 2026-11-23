//! A client for one MCP server over stdio (protocol `2025-06-18`).
//!
//! Blocking; meant for the supervisor's threads and the engine's. A [`Client`] is `Send +
//! Sync`: calls from several threads share the connection and are matched by request id.
//! [`Client::spawn`] runs a server process in its own process group; [`Client::over`] talks
//! to any reader and writer, which is how the tests run without a process.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use serde_json::{Value, json};

use crate::config::ServerConfig;
use crate::content::format_call_result;
use crate::rpc::{Rpc, lock};

#[cfg(test)]
mod tests;

/// The protocol version this client asks for. Whatever version the server answers is used.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// At most this many `tools/list` pages are read.
const MAX_PAGES: usize = 100;
/// After stdin is closed, how long a server gets to exit on its own.
const EXIT_GRACE: Duration = Duration::from_secs(1);
/// Between SIGTERM and SIGKILL.
const KILL_GRACE: Duration = Duration::from_millis(500);
/// How long a dead server's stderr gets to reach the tail before an error is built.
const STDERR_GRACE: Duration = Duration::from_millis(300);

/// Why a request did not produce a result. The caller branches on the kind: a closed
/// connection means the server is gone, the others leave it usable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpError {
    /// The process could not be started.
    Spawn(String),
    /// The server exited or closed its pipes.
    Closed(String),
    /// No answer in time; the request was cancelled on the server.
    Timeout(String),
    /// The caller's cancel token fired; the request was cancelled on the server.
    Cancelled,
    /// The server answered with a JSON-RPC error.
    Rpc { code: i64, message: String },
    /// The server answered something that is not what the protocol says.
    Protocol(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(m) | Self::Closed(m) | Self::Timeout(m) | Self::Protocol(m) => f.write_str(m),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Rpc { code, message } => write!(f, "MCP error {code}: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

/// What the server said about itself in `initialize`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// The protocol version the server chose.
    pub protocol_version: String,
}

/// A tool as the server lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments; `{"type": "object"}` when the server gave none.
    pub input_schema: Value,
}

/// A finished `tools/call`: text for the model (see [`crate::MAX_RESULT_BYTES`]) and whether
/// the tool reported a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
}

/// The server process behind a spawned client.
struct Process {
    child: Mutex<Child>,
    /// The process id, which is also its process group id.
    pid: u32,
    stderr_done: Arc<AtomicBool>,
}

/// A connection to one MCP server. Dropping it stops the server.
pub struct Client {
    rpc: Arc<Rpc>,
    process: Option<Process>,
    stopped: AtomicBool,
}

impl Client {
    /// Starts the server process: own process group, piped stdin/stdout/stderr, the
    /// config's env on top of ours. Does not talk to it yet; see [`Client::initialize`].
    pub fn spawn(config: &ServerConfig) -> Result<Client, McpError> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().map_err(|e| {
            let place = config.cwd.as_ref().map(|c| format!(" in {}", c.display())).unwrap_or_default();
            McpError::Spawn(format!("could not start `{}`{place}: {e}", config.command))
        })?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
            kill_child(&mut child);
            return Err(McpError::Spawn("the server's pipes are missing".into()));
        };
        let process = Process {
            pid: child.id(),
            child: Mutex::new(child),
            stderr_done: Arc::new(AtomicBool::new(false)),
        };
        let started = Rpc::start(&config.name, Box::new(stdout), Box::new(stdin)).and_then(|rpc| {
            rpc.attach_stderr(&config.name, Box::new(stderr), process.stderr_done.clone())?;
            Ok(rpc)
        });
        match started {
            Ok(rpc) => Ok(Client { rpc, process: Some(process), stopped: AtomicBool::new(false) }),
            Err(e) => {
                kill_child(&mut lock(&process.child));
                Err(McpError::Spawn(format!("could not start the client threads: {e}")))
            }
        }
    }

    /// A client over an existing connection: `reader` is what the server writes, `writer`
    /// what it reads. There is no process and no stderr.
    pub fn over<R, W>(name: &str, reader: R, writer: W) -> Result<Client, McpError>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let rpc = Rpc::start(name, Box::new(reader), Box::new(writer))
            .map_err(|e| McpError::Spawn(format!("could not start the client threads: {e}")))?;
        Ok(Client { rpc, process: None, stopped: AtomicBool::new(false) })
    }

    /// The handshake: `initialize`, then `notifications/initialized`.
    pub fn initialize(&self, timeout: Duration, cancel: &CancelToken) -> Result<ServerInfo, McpError> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "harness", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = self.rpc.request("initialize", params, timeout, cancel)?;
        if !result.is_object() {
            return Err(McpError::Protocol(format!(
                "`initialize` returned {} instead of an object",
                kind(&result)
            )));
        }
        let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or_default().to_string();
        let info = ServerInfo {
            name: text(result.pointer("/serverInfo/name")),
            version: text(result.pointer("/serverInfo/version")),
            protocol_version: text(result.get("protocolVersion")),
        };
        self.rpc.notify("notifications/initialized", None);
        Ok(info)
    }

    /// Every tool of the server, following `nextCursor` across pages, within `timeout` in
    /// total. A tool without a name is skipped; a repeated name keeps its first listing.
    pub fn list_tools(&self, timeout: Duration, cancel: &CancelToken) -> Result<Vec<ToolInfo>, McpError> {
        let deadline = Instant::now() + timeout;
        let mut tools: Vec<ToolInfo> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let left = deadline.saturating_duration_since(Instant::now());
            let result = self.rpc.request("tools/list", params, left, cancel)?;
            let Some(list) = result.get("tools").and_then(Value::as_array) else {
                return Err(McpError::Protocol("`tools/list` returned no `tools` array".into()));
            };
            for tool in list {
                let Some(name) = tool.get("name").and_then(Value::as_str) else { continue };
                if tools.iter().any(|t| t.name == name) {
                    continue;
                }
                tools.push(ToolInfo {
                    name: name.to_string(),
                    description: tool.get("description").and_then(Value::as_str).unwrap_or_default().to_string(),
                    input_schema: tool
                        .get("inputSchema")
                        .filter(|s| s.is_object())
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                });
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() && cursor.as_deref() != Some(next) => {
                    cursor = Some(next.to_string());
                }
                _ => return Ok(tools),
            }
        }
        Ok(tools)
    }

    /// Runs `tool` with `arguments` (`null` means no arguments). A tool that fails reports
    /// it in [`CallResult::is_error`]; `Err` means no result came back.
    pub fn call_tool(
        &self,
        tool: &str,
        arguments: Value,
        cancel: &CancelToken,
        timeout: Duration,
    ) -> Result<CallResult, McpError> {
        let arguments = if arguments.is_null() { json!({}) } else { arguments };
        let params = json!({ "name": tool, "arguments": arguments });
        let result = self.rpc.request("tools/call", params, timeout, cancel)?;
        Ok(format_call_result(&result))
    }

    /// Whether the connection is open and the process (if any) still runs.
    pub fn is_alive(&self) -> bool {
        if self.stopped.load(Ordering::SeqCst) || self.rpc.closed_reason().is_some() {
            return false;
        }
        match &self.process {
            Some(p) => matches!(lock(&p.child).try_wait(), Ok(None)),
            None => true,
        }
    }

    /// Why the connection ended (`the server closed its output`), or `None` while it is open.
    pub fn closed_reason(&self) -> Option<String> {
        self.rpc.closed_reason()
    }

    /// `exited with exit status: 1` once the process has ended, else `None`.
    pub fn exit_status(&self) -> Option<String> {
        let p = self.process.as_ref()?;
        let status = lock(&p.child).try_wait().ok().flatten()?;
        Some(format!("exited with {status}"))
    }

    /// The server process id, for a spawned client.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().map(|p| p.pid)
    }

    /// The last few KB of the server's stderr, with notes about messages we ignored. When the
    /// process has exited, waits briefly for the rest of its stderr to arrive.
    pub fn stderr_tail(&self) -> String {
        if let Some(p) = &self.process
            && matches!(lock(&p.child).try_wait(), Ok(Some(_)))
        {
            let end = Instant::now() + STDERR_GRACE;
            while !p.stderr_done.load(Ordering::SeqCst) && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.rpc.tail()
    }

    /// Stops the server: closes its stdin, waits a second for it to exit, then signals its
    /// whole process group (SIGTERM, then SIGKILL). Pending requests fail with
    /// [`McpError::Closed`]. Blocks for at most about two seconds; safe to call again.
    pub fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.rpc.close("the connection was shut down");
        let Some(p) = &self.process else { return };
        if !p.wait_exit(EXIT_GRACE) {
            p.signal_group("TERM");
            if !p.wait_exit(KILL_GRACE) {
                p.signal_group("KILL");
                kill_child(&mut lock(&p.child));
                return;
            }
        }
        // The leader is gone; children it left in its group go too.
        p.signal_group("KILL");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Process {
    /// Polls for the process to exit (and reaps it) for up to `limit`.
    fn wait_exit(&self, limit: Duration) -> bool {
        let end = Instant::now() + limit;
        loop {
            if !matches!(lock(&self.child).try_wait(), Ok(None)) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Signals the server's whole process group, so helpers it started go with it. Falls back
    /// to killing the direct child when the group cannot be signalled.
    fn signal_group(&self, signal: &str) {
        let Ok(pgid) = i32::try_from(self.pid) else { return };
        let sent = match signal {
            "KILL" => base::process::kill_group(pgid),
            _ => base::process::terminate_group(pgid),
        };
        if !sent {
            let mut child = lock(&self.child);
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
            }
        }
    }
}

fn kill_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}
