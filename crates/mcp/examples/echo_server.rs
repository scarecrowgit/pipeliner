//! A tiny MCP server over stdio, for the `mcp` crate's tests and for trying the client by hand.
//!
//! Run: `cargo run -p mcp --example echo_server`, then type JSON-RPC lines.
//!
//! Tools: `echo {text}`, `slow {ms}` (stops early on `notifications/cancelled`), `fail`
//! (an error result), `pid` (its process id), `spawn_child` (starts `sleep 600` and returns
//! its pid), `exit {code}`. `tools/list` returns two tools per page.
//!
//! Test knobs (environment): `ECHO_EXIT_IMMEDIATELY=1` exits before reading anything,
//! `ECHO_NO_INIT=1` never answers `initialize`, `ECHO_GARBAGE=1` answers every request with
//! a line that is not JSON, `ECHO_IGNORE_EOF=1` keeps running after stdin closes.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[derive(Default)]
struct Server {
    out: Mutex<()>,
    cancelled: Mutex<HashSet<String>>,
}

impl Server {
    fn send(&self, msg: &Value) {
        let _guard = self.out.lock();
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{msg}");
        let _ = out.flush();
    }

    fn result(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&self, id: &Value, code: i64, message: &str) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    fn take_cancelled(&self, key: &str) -> bool {
        self.cancelled.lock().map(|mut set| set.remove(key)).unwrap_or(false)
    }
}

fn tools() -> Vec<Value> {
    let object = |props: Value| json!({"type": "object", "properties": props});
    vec![
        json!({"name": "echo", "description": "Echo the text back.", "inputSchema": object(json!({"text": {"type": "string"}}))}),
        json!({"name": "slow", "description": "Sleep for `ms` milliseconds.", "inputSchema": object(json!({"ms": {"type": "integer"}}))}),
        json!({"name": "fail", "description": "Always fails."}),
        json!({"name": "pid", "description": "The server's process id."}),
        json!({"name": "spawn_child", "description": "Start a long `sleep` and return its pid."}),
        json!({"name": "exit", "description": "Exit the process.", "inputSchema": object(json!({"code": {"type": "integer"}}))}),
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
        "slow" => {
            let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(1000);
            let start = Instant::now();
            let key = id.to_string();
            while start.elapsed() < Duration::from_millis(ms) {
                if server.take_cancelled(&key) {
                    eprintln!("echo_server: request {key} cancelled");
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            text(format!("slept {ms} ms"))
        }
        "fail" => json!({"content": [{"type": "text", "text": "something went wrong"}], "isError": true}),
        "pid" => text(std::process::id().to_string()),
        "spawn_child" => match std::process::Command::new("sleep").arg("600").spawn() {
            Ok(mut child) => {
                let pid = child.id();
                std::thread::spawn(move || child.wait());
                text(pid.to_string())
            }
            Err(e) => json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true}),
        },
        "exit" => {
            eprintln!("echo_server: exiting on request");
            let code = args.get("code").and_then(Value::as_i64).unwrap_or(3);
            std::process::exit(i32::try_from(code).unwrap_or(3));
        }
        _ => return server.error(id, -32602, &format!("Unknown tool: {name}")),
    };
    server.result(id, result);
}

fn handle_request(server: &Arc<Server>, id: Value, method: &str, params: Value) {
    if std::env::var_os("ECHO_GARBAGE").is_some() {
        let _guard = server.out.lock();
        println!("garbage instead of an answer to {method}");
        return;
    }
    match method {
        "initialize" => {
            if std::env::var_os("ECHO_NO_INIT").is_some() {
                return;
            }
            let version = params.get("protocolVersion").cloned().unwrap_or(json!("2025-06-18"));
            server.result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "echo-server", "version": "0.2.0"},
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
    if std::env::var_os("ECHO_EXIT_IMMEDIATELY").is_some() {
        eprintln!("echo_server: boom, exiting immediately");
        std::process::exit(2);
    }
    eprintln!("echo_server: starting up");

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
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        match (method, msg.get("id").cloned()) {
            (Some(method), Some(id)) => handle_request(&server, id, &method, params),
            (Some(method), None) if method == "notifications/cancelled" => {
                if let (Some(req), Ok(mut set)) = (params.get("requestId"), server.cancelled.lock()) {
                    set.insert(req.to_string());
                }
            }
            _ => {}
        }
    }
    eprintln!("echo_server: stdin closed");
    if std::env::var_os("ECHO_IGNORE_EOF").is_some() {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}
