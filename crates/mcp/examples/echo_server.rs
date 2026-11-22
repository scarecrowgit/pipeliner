//! Tiny MCP stdio server used by the `mcp` crate's integration tests.
//!
//! Tools: `echo {text}`, `add {a, b}`, `slow {ms}`, `fail`, `exit {code}`, `env {name}`,
//! `roundtrip` (sends `ping` and `roots/list` to the client), `mixed` (image + structured
//! content), `pid`. `tools/list` returns 2 tools per page.
//!
//! Test knobs (env): `ECHO_EXIT_IMMEDIATELY=1` (exit before reading anything),
//! `ECHO_NO_INIT=1` (never answer `initialize`), `ECHO_IGNORE_EOF=1` (keep running after
//! stdin closes, so the client must signal it).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead as _, Write as _};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[derive(Default)]
struct Server {
    out: Mutex<()>,
    cancelled: Mutex<HashSet<String>>,
    replies: Mutex<HashMap<String, Sender<Value>>>,
}

impl Server {
    fn send(&self, v: &Value) {
        let _g = self.out.lock().unwrap();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    }

    fn result(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    /// Send a request to the client and wait for its response.
    fn ask_client(&self, id: &str, method: &str) -> Option<Value> {
        let (tx, rx) = mpsc::channel();
        self.replies.lock().unwrap().insert(id.to_string(), tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {}}));
        rx.recv_timeout(Duration::from_secs(5)).ok()
    }
}

fn tools() -> Vec<Value> {
    let obj = |props: Value| json!({"type": "object", "properties": props});
    vec![
        json!({"name": "echo", "description": "Echo the text back.\nSecond line.", "inputSchema": obj(json!({"text": {"type": "string"}}))}),
        json!({"name": "add", "description": "Add two numbers.", "inputSchema": obj(json!({"a": {"type": "number"}, "b": {"type": "number"}}))}),
        json!({"name": "slow", "description": "Sleep for `ms` milliseconds.", "inputSchema": obj(json!({"ms": {"type": "integer"}}))}),
        json!({"name": "fail", "description": "Always fails."}),
        json!({"name": "exit", "description": "Exit the process.", "inputSchema": obj(json!({"code": {"type": "integer"}}))}),
        json!({"name": "env", "description": "Read an environment variable.", "inputSchema": obj(json!({"name": {"type": "string"}}))}),
        json!({"name": "roundtrip", "description": "Send ping and roots/list to the client."}),
        json!({"name": "mixed", "description": "Image plus structured content."}),
        json!({"name": "pid", "description": "Process id."}),
    ]
}

fn text(s: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": s.into()}]})
}

fn call_tool(server: &Server, id: &Value, params: &Value) {
    let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    eprintln!("echo_server: tools/call {name}");
    let result = match name {
        "echo" => text(args.get("text").and_then(Value::as_str).unwrap_or_default()),
        "add" => {
            let n = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let sum = n("a") + n("b");
            text(if sum.fract() == 0.0 { format!("{}", sum as i64) } else { sum.to_string() })
        }
        "slow" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(1000);
            let start = Instant::now();
            let key = id.to_string();
            while start.elapsed() < Duration::from_millis(ms) {
                if server.cancelled.lock().unwrap().remove(&key) {
                    eprintln!("echo_server: request {key} cancelled");
                    return; // no response for a cancelled request
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            text(format!("slept {ms} ms"))
        }
        "fail" => json!({"content": [{"type": "text", "text": "something went wrong"}], "isError": true}),
        "exit" => {
            eprintln!("echo_server: exiting on request");
            std::process::exit(args.get("code").and_then(Value::as_i64).unwrap_or(3) as i32);
        }
        "env" => {
            let var = args.get("name").and_then(Value::as_str).unwrap_or_default();
            text(std::env::var(var).unwrap_or_else(|_| "<unset>".into()))
        }
        "roundtrip" => {
            let ping = server.ask_client(&format!("srv-{id}-ping"), "ping");
            let roots = server.ask_client(&format!("srv-{id}-roots"), "roots/list");
            let ping = match ping {
                Some(r) if r.get("result") == Some(&json!({})) => "ok".to_string(),
                other => format!("{other:?}"),
            };
            let roots = roots
                .and_then(|r| r.pointer("/error/code").and_then(Value::as_i64))
                .map_or("none".to_string(), |c| c.to_string());
            text(format!("ping: {ping}; roots/list: {roots}"))
        }
        "mixed" => json!({
            "content": [{"type": "image", "data": "AAAAAAAA", "mimeType": "image/png"}],
            "structuredContent": {"answer": 42},
        }),
        "pid" => text(std::process::id().to_string()),
        _ => return server.error(id, -32602, &format!("Unknown tool: {name}")),
    };
    server.result(id, result);
}

fn handle_request(server: &Arc<Server>, id: Value, method: &str, params: Value) {
    match method {
        "initialize" => {
            if std::env::var("ECHO_NO_INIT").is_ok() {
                return;
            }
            let version = params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18"));
            server.result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "echo-server", "version": "0.1.0"},
                }),
            );
        }
        "ping" => server.result(&id, json!({})),
        "tools/list" => {
            let all = tools();
            let start: usize =
                params.get("cursor").and_then(Value::as_str).and_then(|c| c.parse().ok()).unwrap_or(0);
            let end = (start + 2).min(all.len());
            let mut result = json!({"tools": all[start.min(end)..end]});
            if end < all.len() {
                result["nextCursor"] = json!(end.to_string());
            }
            server.result(&id, result);
        }
        "tools/call" => {
            let server = server.clone();
            std::thread::spawn(move || call_tool(&server, &id, &params));
        }
        _ => server.error(&id, -32601, &format!("Method not found: {method}")),
    }
}

fn main() {
    if std::env::var("ECHO_EXIT_IMMEDIATELY").is_ok() {
        eprintln!("echo_server: boom, exiting immediately");
        std::process::exit(2);
    }
    eprintln!("echo_server: starting up (some stderr noise)");
    println!("echo_server: this line is not JSON");

    let server = Arc::new(Server::default());
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("echo_server: bad input line: {e}");
                continue;
            }
        };
        let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        match (method, id) {
            (Some(method), Some(id)) => handle_request(&server, id, &method, params),
            (Some(method), None) => {
                if method == "notifications/cancelled"
                    && let Some(req) = params.get("requestId")
                {
                    server.cancelled.lock().unwrap().insert(req.to_string());
                }
            }
            (None, Some(id)) => {
                let key = id.as_str().unwrap_or_default().to_string();
                if let Some(tx) = server.replies.lock().unwrap().remove(&key) {
                    let _ = tx.send(msg);
                }
            }
            (None, None) => {}
        }
    }
    eprintln!("echo_server: stdin closed");
    if std::env::var("ECHO_IGNORE_EOF").is_ok() {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}
