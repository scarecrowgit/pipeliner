//! JSON-RPC 2.0 over newline-delimited JSON, on any reader and writer.
//!
//! A reader thread routes responses to the waiting request by id, answers the server's
//! `ping`, refuses its other requests and drops notifications. A writer thread does every
//! write, so a server that stops reading never blocks a caller: requests still time out.
//! What the server prints on stderr (and notes about its protocol mistakes) goes into a small
//! tail used for error messages.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use serde_json::{Value, json};

use crate::client::McpError;

/// Bytes of stderr (and notes) kept for error messages.
const TAIL_BYTES: usize = 4096;
/// A single message longer than this is dropped; it would only be cut to 100 KB anyway.
const MAX_LINE: usize = 16 * 1024 * 1024;
/// How often a waiting request looks at its cancel token.
const POLL: Duration = Duration::from_millis(20);

/// A poisoned lock only means another thread panicked mid-update; the data is still usable
/// for this module's simple maps and buffers.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

type Reply = Result<Value, McpError>;

#[derive(Default)]
struct Pending {
    /// Why the connection ended; set once, after which no request is accepted.
    closed: Option<String>,
    waiters: HashMap<u64, Sender<Reply>>,
}

/// One JSON-RPC connection to a server.
pub(crate) struct Rpc {
    outgoing: Mutex<Option<Sender<Vec<u8>>>>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    tail: Mutex<Vec<u8>>,
}

impl Rpc {
    /// Starts the reader and writer threads over `reader` (the server's stdout) and
    /// `writer` (its stdin). `name` only names the threads.
    pub(crate) fn start(
        name: &str,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
    ) -> std::io::Result<Arc<Rpc>> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let rpc = Arc::new(Rpc {
            outgoing: Mutex::new(Some(tx)),
            pending: Mutex::new(Pending::default()),
            next_id: AtomicU64::new(1),
            tail: Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&rpc);
        std::thread::Builder::new().name(format!("mcp-{name}-out")).spawn(move || {
            let mut writer = writer;
            for line in rx {
                if writer.write_all(&line).and_then(|()| writer.flush()).is_err() {
                    if let Some(rpc) = weak.upgrade() {
                        rpc.mark_closed("the server stopped reading its input");
                    }
                    break;
                }
            }
            // Dropping the writer closes the server's stdin.
        })?;
        let weak = Arc::downgrade(&rpc);
        let spawned = std::thread::Builder::new()
            .name(format!("mcp-{name}-in"))
            .spawn(move || read_loop(weak, reader));
        if let Err(e) = spawned {
            rpc.close("the client could not start");
            return Err(e);
        }
        Ok(rpc)
    }

    /// Copies a stderr stream into the tail on its own thread.
    pub(crate) fn attach_stderr(
        self: &Arc<Self>,
        name: &str,
        mut stderr: Box<dyn Read + Send>,
        done: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::io::Result<()> {
        let weak = Arc::downgrade(self);
        std::thread::Builder::new().name(format!("mcp-{name}-err")).spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => match weak.upgrade() {
                        Some(rpc) => rpc.push_tail(&chunk[..n]),
                        None => break,
                    },
                    Err(e) if e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            done.store(true, Ordering::SeqCst);
        })?;
        Ok(())
    }

    /// Sends a request and waits for its answer, the timeout or the cancel token. On a
    /// timeout or a cancel the server is told with `notifications/cancelled`.
    pub(crate) fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Reply {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = lock(&self.pending);
            if let Some(reason) = &pending.closed {
                return Err(McpError::Closed(reason.clone()));
            }
            pending.waiters.insert(id, tx);
        }
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if cancel.is_cancelled() {
                self.abandon(id, method, "cancelled by the user");
                return Err(McpError::Cancelled);
            }
            let left = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                None => POLL,
            };
            if left.is_zero() {
                let text = format!("no answer to `{method}` within {}", seconds(timeout));
                self.abandon(id, method, &text);
                return Err(McpError::Timeout(text));
            }
            match rx.recv_timeout(left.min(POLL)) {
                Ok(reply) => return reply,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let reason = self.closed_reason().unwrap_or_else(|| "connection closed".into());
                    return Err(McpError::Closed(format!("{reason} during `{method}`")));
                }
            }
        }
    }

    /// Sends a notification. Errors surface on the next request.
    pub(crate) fn notify(&self, method: &str, params: Option<Value>) {
        let mut msg = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.send(&msg);
    }

    /// Ends the connection: pending requests fail with `reason`, the writer closes the
    /// server's stdin once it has written what is queued.
    pub(crate) fn close(&self, reason: &str) {
        self.mark_closed(reason);
        lock(&self.outgoing).take();
    }

    /// Why the connection ended, or `None` while it is open.
    pub(crate) fn closed_reason(&self) -> Option<String> {
        lock(&self.pending).closed.clone()
    }

    /// The last few KB of stderr and of notes about protocol mistakes.
    pub(crate) fn tail(&self) -> String {
        String::from_utf8_lossy(&lock(&self.tail)).trim().to_string()
    }

    /// Adds a note of ours to the tail, on its own line.
    pub(crate) fn note(&self, text: &str) {
        let mut tail = lock(&self.tail);
        if tail.last().is_some_and(|b| *b != b'\n') {
            push_capped(&mut tail, b"\n");
        }
        push_capped(&mut tail, format!("[harness] {text}\n").as_bytes());
    }

    fn push_tail(&self, bytes: &[u8]) {
        push_capped(&mut lock(&self.tail), bytes);
    }

    fn send(&self, msg: &Value) {
        let mut line = msg.to_string().into_bytes();
        line.push(b'\n');
        if let Some(tx) = lock(&self.outgoing).as_ref() {
            // A closed writer shows up as a closed connection on the request side.
            let _ = tx.send(line);
        }
    }

    /// Forgets a request we stopped waiting for and tells the server.
    fn abandon(&self, id: u64, method: &str, reason: &str) {
        lock(&self.pending).waiters.remove(&id);
        // The spec forbids cancelling `initialize`.
        if method != "initialize" {
            self.notify("notifications/cancelled", Some(json!({"requestId": id, "reason": reason})));
        }
    }

    fn mark_closed(&self, reason: &str) {
        let mut pending = lock(&self.pending);
        if pending.closed.is_none() {
            pending.closed = Some(reason.to_string());
        }
        // Dropping the senders wakes every waiter with `Disconnected`.
        pending.waiters.clear();
    }

    fn handle(&self, msg: Value) {
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").filter(|id| !id.is_null());
        match (method, id) {
            (Some("ping"), Some(id)) => self.send(&json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            (Some(method), Some(id)) => self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601,
                "message": format!("Method not found: {method}"),
            }})),
            // Logging, progress, list_changed: nothing to do.
            (Some(_), None) => {}
            (None, Some(id)) => self.route(id, &msg),
            (None, None) => match msg.get("error") {
                Some(e) => self.note(&format!("the server reported an error: {}", short(&e.to_string()))),
                None => self.note(&format!("ignored a message without id or method: {}", short(&msg.to_string()))),
            },
        }
    }

    fn route(&self, id: &Value, msg: &Value) {
        let key = id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok()));
        let reply = match msg.get("error") {
            Some(e) => Err(rpc_error(e)),
            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
        };
        let waiter = key.and_then(|k| lock(&self.pending).waiters.remove(&k));
        match waiter {
            Some(tx) => {
                let _ = tx.send(reply);
            }
            None => self.note(&format!("ignored a response to unknown request id {id}")),
        }
    }
}

fn rpc_error(e: &Value) -> McpError {
    let code = e.get("code").and_then(Value::as_i64).unwrap_or(0);
    let mut message = e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string();
    if let Some(data) = e.get("data").filter(|d| !d.is_null()) {
        let data = match data {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        message = format!("{message} ({})", short(&data));
    }
    McpError::Rpc { code, message }
}

fn read_loop(rpc: Weak<Rpc>, reader: Box<dyn Read + Send>) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    let reason = loop {
        buf.clear();
        let read = read_line(&mut reader, &mut buf);
        let Some(rpc) = rpc.upgrade() else { return };
        match read {
            Ok(Line::Eof) => break "the server closed its output",
            Err(_) => break "reading from the server failed",
            Ok(Line::TooLong) => {
                rpc.note(&format!("ignored a message longer than {}", base::fmt::bytes(MAX_LINE as u64)));
            }
            Ok(Line::Full) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim();
                if text.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(text) {
                    Ok(Value::Array(items)) => items.into_iter().for_each(|m| rpc.handle(m)),
                    Ok(msg @ Value::Object(_)) => rpc.handle(msg),
                    _ => rpc.note(&format!("ignored a non-JSON line on stdout: {}", short(text))),
                }
            }
        }
    };
    if let Some(rpc) = rpc.upgrade() {
        rpc.mark_closed(reason);
    }
}

enum Line {
    Full,
    TooLong,
    Eof,
}

/// Reads one `\n`-terminated line into `buf` (without the newline). A line over
/// [`MAX_LINE`] is skipped whole and reported as `TooLong`.
fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<Line> {
    let mut too_long = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if buf.is_empty() {
                Line::Eof
            } else {
                Line::Full
            });
        }
        let (chunk, used, end) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (&available[..i], i + 1, true),
            None => (available, available.len(), false),
        };
        if !too_long {
            buf.extend_from_slice(chunk);
            if buf.len() > MAX_LINE {
                too_long = true;
                buf.clear();
            }
        }
        reader.consume(used);
        if end {
            return Ok(if too_long { Line::TooLong } else { Line::Full });
        }
    }
}

fn push_capped(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(bytes);
    if buf.len() > TAIL_BYTES {
        let excess = buf.len() - TAIL_BYTES;
        buf.drain(..excess);
    }
}

fn short(text: &str) -> String {
    base::fmt::truncate_chars(text, 200)
}

/// `20 s`, or milliseconds below a second (tests use short timeouts).
pub(crate) fn seconds(d: Duration) -> String {
    if d.as_millis() < 1000 { format!("{} ms", d.as_millis()) } else { format!("{} s", d.as_secs()) }
}
