//! The client over in-memory pipes, with the test playing the server.

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

/// The write end of an in-memory pipe; writes fail once the reader is gone.
struct PipeWriter(Sender<Vec<u8>>);

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(buf.to_vec()).map_err(|_| std::io::ErrorKind::BrokenPipe)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The read end; EOF once every writer is dropped.
struct PipeReader {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.buf.len() {
            match self.rx.recv() {
                Ok(chunk) => (self.buf, self.pos) = (chunk, 0),
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn pipe() -> (PipeWriter, PipeReader) {
    let (tx, rx) = mpsc::channel();
    (PipeWriter(tx), PipeReader { rx, buf: Vec::new(), pos: 0 })
}

/// The test's side of the connection: what the client sent, and a pipe to answer on.
struct Fake {
    client: Arc<Client>,
    inbox: Receiver<Value>,
    out: Mutex<Option<PipeWriter>>,
}

impl Fake {
    fn new() -> Self {
        let (to_client, client_reads) = pipe();
        let (client_writes, from_client) = pipe();
        let client = Client::over("fake", client_reads, client_writes).unwrap();
        let (tx, inbox) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(from_client).lines() {
                let msg: Value = serde_json::from_str(&line.unwrap()).expect("client sent JSON");
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Fake { client: Arc::new(client), inbox, out: Mutex::new(Some(to_client)) }
    }

    /// The next message from the client.
    fn recv(&self) -> Value {
        self.inbox.recv_timeout(WAIT).expect("the client sent nothing")
    }

    fn raw(&self, line: &str) {
        let mut out = lock(&self.out);
        out.as_mut().unwrap().write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    fn send(&self, msg: Value) {
        self.raw(&msg.to_string());
    }

    fn reply(&self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}));
    }

    /// Closes the server's output, as a dying process would.
    fn hang_up(&self) {
        lock(&self.out).take();
    }

    /// Runs `f` with the client on another thread.
    fn run<T: Send + 'static>(&self, f: impl FnOnce(&Client) -> T + Send + 'static) -> JoinHandle<T> {
        let client = self.client.clone();
        std::thread::spawn(move || f(&client))
    }
}

fn text_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}]})
}

fn call(f: &Fake, tool: &'static str, timeout: Duration) -> JoinHandle<Result<CallResult, McpError>> {
    f.run(move |c| c.call_tool(tool, json!({"x": 1}), &CancelToken::new(), timeout))
}

#[test]
fn handshake() {
    let f = Fake::new();
    let job = f.run(|c| c.initialize(WAIT, &CancelToken::new()));
    let init = f.recv();
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["params"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(init["params"]["clientInfo"]["name"], "harness");
    f.reply(&init, json!({"protocolVersion": "2025-03-26", "capabilities": {}, "serverInfo": {"name": "fake", "version": "1.2"}}));
    let info = job.join().unwrap().unwrap();
    assert_eq!(info, ServerInfo { name: "fake".into(), version: "1.2".into(), protocol_version: "2025-03-26".into() });
    let note = f.recv();
    assert_eq!(note["method"], "notifications/initialized");
    assert!(note.get("id").is_none());
    assert!(f.client.is_alive());
}

#[test]
fn initialize_must_return_an_object() {
    let f = Fake::new();
    let job = f.run(|c| c.initialize(WAIT, &CancelToken::new()));
    let init = f.recv();
    f.reply(&init, json!("hello"));
    assert!(matches!(job.join().unwrap(), Err(McpError::Protocol(m)) if m.contains("a string")));
}

#[test]
fn list_tools_follows_cursors() {
    let f = Fake::new();
    let job = f.run(|c| c.list_tools(WAIT, &CancelToken::new()));
    let first = f.recv();
    assert_eq!(first["method"], "tools/list");
    assert_eq!(first["params"], json!({}));
    f.reply(&first, json!({"tools": [
        {"name": "a", "description": "A.", "inputSchema": {"type": "object", "properties": {"p": {}}}},
        {"name": "b"},
    ], "nextCursor": "page-2"}));
    let second = f.recv();
    assert_eq!(second["params"]["cursor"], "page-2");
    f.reply(&second, json!({"tools": [{"description": "nameless"}, {"name": "a"}, {"name": "c", "inputSchema": 3}]}));
    let tools = job.join().unwrap().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert_eq!(tools[0].description, "A.");
    assert_eq!(tools[0].input_schema["properties"], json!({"p": {}}));
    assert_eq!(tools[1].input_schema, json!({"type": "object"}));
    assert_eq!(tools[2].input_schema, json!({"type": "object"}));
}

#[test]
fn list_tools_stops_on_a_repeated_cursor() {
    let f = Fake::new();
    let job = f.run(|c| c.list_tools(WAIT, &CancelToken::new()));
    let first = f.recv();
    f.reply(&first, json!({"tools": [{"name": "a"}], "nextCursor": "same"}));
    let second = f.recv();
    f.reply(&second, json!({"tools": [{"name": "b"}], "nextCursor": "same"}));
    assert_eq!(job.join().unwrap().unwrap().len(), 2);
}

#[test]
fn call_results_and_errors() {
    let f = Fake::new();
    let job = call(&f, "echo", WAIT);
    let req = f.recv();
    assert_eq!(req["method"], "tools/call");
    assert_eq!(req["params"], json!({"name": "echo", "arguments": {"x": 1}}));
    f.reply(&req, text_result("hi"));
    assert_eq!(job.join().unwrap().unwrap(), CallResult { text: "hi".into(), is_error: false });

    let job = call(&f, "fail", WAIT);
    let req = f.recv();
    f.reply(&req, json!({"content": [{"type": "text", "text": "boom"}], "isError": true}));
    assert_eq!(job.join().unwrap().unwrap(), CallResult { text: "boom".into(), is_error: true });

    let job = call(&f, "missing", WAIT);
    let req = f.recv();
    f.send(json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -32602, "message": "Unknown tool", "data": "missing"}}));
    let err = job.join().unwrap().unwrap_err();
    assert_eq!(err, McpError::Rpc { code: -32602, message: "Unknown tool (missing)".into() });
    assert_eq!(err.to_string(), "MCP error -32602: Unknown tool (missing)");

    let job = f.run(|c| c.call_tool("bare", Value::Null, &CancelToken::new(), WAIT));
    let req = f.recv();
    assert_eq!(req["params"]["arguments"], json!({}));
    f.reply(&req, text_result("ok"));
    assert!(job.join().unwrap().is_ok());
}

#[test]
fn cancel_tells_the_server() {
    let f = Fake::new();
    let cancel = CancelToken::new();
    let remote = cancel.clone();
    let job = f.run(move |c| c.call_tool("slow", json!({}), &remote, WAIT));
    let req = f.recv();
    cancel.cancel();
    assert_eq!(job.join().unwrap(), Err(McpError::Cancelled));
    let note = f.recv();
    assert_eq!(note["method"], "notifications/cancelled");
    assert_eq!(note["params"]["requestId"], req["id"]);
    // A late answer to the cancelled request is ignored and the client stays usable.
    f.reply(&req, text_result("late"));
    let job = call(&f, "echo", WAIT);
    let req = f.recv();
    f.reply(&req, text_result("after"));
    assert_eq!(job.join().unwrap().unwrap().text, "after");
}

#[test]
fn timeout_cancels_the_request() {
    let f = Fake::new();
    let started = Instant::now();
    let job = call(&f, "slow", Duration::from_millis(150));
    let req = f.recv();
    let err = job.join().unwrap().unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(err, McpError::Timeout("no answer to `tools/call` within 150 ms".into()));
    let note = f.recv();
    assert_eq!(note["method"], "notifications/cancelled");
    assert_eq!(note["params"]["requestId"], req["id"]);
}

#[test]
fn initialize_timeout_is_not_cancelled() {
    let f = Fake::new();
    let job = f.run(|c| c.initialize(Duration::from_millis(100), &CancelToken::new()));
    assert_eq!(f.recv()["method"], "initialize");
    assert!(matches!(job.join().unwrap(), Err(McpError::Timeout(_))));
    assert!(f.inbox.recv_timeout(Duration::from_millis(200)).is_err(), "nothing after a timed-out initialize");
}

#[test]
fn answers_ping_and_refuses_other_requests() {
    let f = Fake::new();
    f.send(json!({"jsonrpc": "2.0", "id": "p1", "method": "ping"}));
    assert_eq!(f.recv(), json!({"jsonrpc": "2.0", "id": "p1", "result": {}}));
    f.send(json!({"jsonrpc": "2.0", "id": 7, "method": "roots/list", "params": {}}));
    let reply = f.recv();
    assert_eq!(reply["id"], 7);
    assert_eq!(reply["error"]["code"], -32601);
    // Notifications get no answer.
    f.send(json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info"}}));
    assert!(f.inbox.recv_timeout(Duration::from_millis(150)).is_err());
}

#[test]
fn results_are_capped() {
    let f = Fake::new();
    let job = call(&f, "big", WAIT);
    let req = f.recv();
    f.reply(&req, text_result(&"x".repeat(300 * 1024)));
    let result = job.join().unwrap().unwrap();
    assert!(result.text.len() < crate::MAX_RESULT_BYTES + 200, "{}", result.text.len());
    assert!(result.text.ends_with("]") && result.text.contains("output truncated"));
}

#[test]
fn interleaved_responses_reach_their_callers() {
    let f = Fake::new();
    let one = call(&f, "one", WAIT);
    let first = f.recv();
    let two = call(&f, "two", WAIT);
    let second = f.recv();
    assert_ne!(first["id"], second["id"]);
    f.raw("not json at all");
    f.send(json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}));
    f.send(json!({"jsonrpc": "2.0", "id": 999, "result": {}}));
    // Both answers in one batch, the later request first.
    let batch = json!([
        {"jsonrpc": "2.0", "id": second["id"], "result": text_result("for two")},
        {"jsonrpc": "2.0", "id": first["id"], "result": text_result("for one")},
    ]);
    f.send(batch);
    assert_eq!(two.join().unwrap().unwrap().text, "for two");
    assert_eq!(one.join().unwrap().unwrap().text, "for one");
    let tail = f.client.stderr_tail();
    assert!(tail.contains("ignored a non-JSON line on stdout: not json at all"), "{tail}");
    assert!(tail.contains("unknown request id 999"), "{tail}");
}

#[test]
fn wrong_id_means_a_timeout_with_a_note() {
    let f = Fake::new();
    let job = call(&f, "x", Duration::from_millis(200));
    let req = f.recv();
    let wrong = req["id"].as_u64().unwrap() + 100;
    f.send(json!({"jsonrpc": "2.0", "id": wrong, "result": text_result("misrouted")}));
    assert!(matches!(job.join().unwrap(), Err(McpError::Timeout(_))));
    assert!(f.client.stderr_tail().contains(&format!("unknown request id {wrong}")));
}

#[test]
fn hang_up_fails_pending_and_later_requests() {
    let f = Fake::new();
    let job = call(&f, "x", WAIT);
    f.recv();
    f.raw("garbage before dying");
    f.hang_up();
    let err = job.join().unwrap().unwrap_err();
    assert_eq!(err, McpError::Closed("the server closed its output during `tools/call`".into()));
    assert!(!f.client.is_alive());
    assert_eq!(f.client.closed_reason().as_deref(), Some("the server closed its output"));
    let again = f.client.call_tool("x", json!({}), &CancelToken::new(), WAIT);
    assert!(matches!(again, Err(McpError::Closed(_))));
}

#[test]
fn shutdown_closes_the_connection() {
    let f = Fake::new();
    let job = call(&f, "x", WAIT);
    f.recv();
    f.client.shutdown();
    assert!(matches!(job.join().unwrap(), Err(McpError::Closed(m)) if m.contains("shut down")));
    assert!(!f.client.is_alive());
    // The server side sees its input end.
    assert!(matches!(f.inbox.recv_timeout(WAIT), Err(mpsc::RecvTimeoutError::Disconnected)));
    f.client.shutdown();
}
