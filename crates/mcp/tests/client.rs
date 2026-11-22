//! Integration tests against a real child process (`examples/echo_server.rs`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use mcp::{Client, ServerConfig, ServerSource};
use serde_json::json;

const T: Duration = Duration::from_secs(10);

/// Path of the built `echo_server` example, building it once per test binary.
fn echo_server() -> PathBuf {
    // Test binaries live in `<target>/<profile>/deps/`; examples in `<target>/<profile>/examples/`.
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe
        .parent()
        .and_then(|deps| deps.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug"));
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.args(["build", "--quiet", "-p", "mcp", "--example", "echo_server"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(Stdio::null());
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            cmd.arg("--release");
        }
        let status = cmd.status().expect("running cargo build for echo_server");
        assert!(status.success(), "building echo_server failed");
    });
    let path = profile_dir.join("examples").join("echo_server");
    assert!(path.exists(), "{} missing", path.display());
    path
}

fn config(env: &[(&str, &str)]) -> ServerConfig {
    ServerConfig {
        name: "echo".into(),
        command: echo_server().to_string_lossy().into_owned(),
        args: vec![],
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
        cwd: None,
        enabled: true,
        auto_approve: vec![],
        source: ServerSource::HarnessConfig,
    }
}

fn start() -> Client {
    Client::start(&config(&[]), T).expect("start echo server")
}

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn call(c: &Client, tool: &str, args: serde_json::Value) -> mcp::CallResult {
    c.call_tool(tool, args, no_cancel(), T).expect("call_tool")
}

fn process_exists(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

fn server_pid(c: &Client) -> u32 {
    call(c, "pid", json!({})).text.parse().unwrap()
}

#[test]
fn start_list_and_call() {
    let c = Client::start(&config(&[("MCP_ECHO_TEST", "value-1")]), T).unwrap();
    assert_eq!(c.server_info(), ("echo-server".to_string(), "0.1.0".to_string()));
    assert!(c.is_alive());

    let tools = c.list_tools(T).unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["echo", "add", "slow", "fail", "exit", "env", "roundtrip", "mixed", "pid"],
        "pagination must collect all pages"
    );
    assert_eq!(tools[0].description, "Echo the text back.\nSecond line.");
    assert_eq!(tools[0].input_schema["properties"]["text"]["type"], "string");
    assert_eq!(tools[3].input_schema, json!({"type": "object"}), "default schema");

    let r = call(&c, "echo", json!({"text": "hi there"}));
    assert_eq!((r.text.as_str(), r.is_error), ("hi there", false));
    assert_eq!(call(&c, "add", json!({"a": 2, "b": 3})).text, "5");
    assert_eq!(call(&c, "env", json!({"name": "MCP_ECHO_TEST"})).text, "value-1");
    // Parent env is inherited.
    assert_ne!(call(&c, "env", json!({"name": "PATH"})).text, "<unset>");

    let mixed = call(&c, "mixed", json!({}));
    assert!(mixed.text.contains("[image: image/png, 6 B]"), "{}", mixed.text);
    assert!(mixed.text.contains("\"answer\": 42"), "{}", mixed.text);

    // Server -> client requests: ping answered, roots/list rejected with -32601.
    assert_eq!(call(&c, "roundtrip", json!({})).text, "ping: ok; roots/list: -32601");

    let tail = c.stderr_tail();
    assert!(tail.contains("some stderr noise"), "{tail}");
    assert!(tail.contains("ignored non-JSON line on stdout"), "{tail}");
}

#[test]
fn tool_errors() {
    let c = start();
    let r = call(&c, "fail", json!({}));
    assert!(r.is_error);
    assert_eq!(r.text, "something went wrong");
    let r = call(&c, "no_such_tool", json!({}));
    assert!(r.is_error);
    assert!(r.text.contains("-32602") && r.text.contains("Unknown tool"), "{}", r.text);
    assert!(c.is_alive());
}

#[test]
fn cancel_slow_call() {
    let c = start();
    let cancel = no_cancel();
    let flag = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::SeqCst);
    });
    let t0 = Instant::now();
    let r = c.call_tool("slow", json!({"ms": 20_000}), cancel, Duration::from_secs(60)).unwrap();
    canceller.join().unwrap();
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    assert!(r.is_error && r.text.contains("cancelled"), "{r:?}");
    // The server saw notifications/cancelled.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !c.stderr_tail().contains("cancelled") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(c.stderr_tail().contains("echo_server: request"), "{}", c.stderr_tail());
    // Still usable.
    assert_eq!(call(&c, "echo", json!({"text": "after"})).text, "after");
}

#[test]
fn timeout_slow_call() {
    let c = start();
    let t0 = Instant::now();
    let r = c
        .call_tool("slow", json!({"ms": 20_000}), no_cancel(), Duration::from_millis(300))
        .unwrap();
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    assert!(r.is_error && r.text.contains("timed out"), "{r:?}");
    assert_eq!(call(&c, "echo", json!({"text": "after"})).text, "after");
    // A fast `slow` call still completes normally.
    assert_eq!(call(&c, "slow", json!({"ms": 20})).text, "slept 20 ms");
}

#[test]
fn concurrent_calls() {
    let c = Arc::new(start());
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let c = c.clone();
            std::thread::spawn(move || {
                // A slow call per thread overlaps with the others' fast ones.
                let slow = c.call_tool("slow", json!({"ms": 150}), no_cancel(), T).unwrap();
                assert_eq!(slow.text, "slept 150 ms");
                for i in 0..20 {
                    let text = format!("thread {t} call {i}");
                    let r = c.call_tool("echo", json!({"text": text}), no_cancel(), T).unwrap();
                    assert_eq!(r.text, text);
                    let r = c.call_tool("add", json!({"a": t, "b": i}), no_cancel(), T).unwrap();
                    assert_eq!(r.text, (t + i).to_string());
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn process_exit_is_an_error() {
    let c = start();
    let pid = server_pid(&c);
    let err = c.call_tool("exit", json!({"code": 3}), no_cancel(), T).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("echo_server: exiting on request"), "stderr tail missing: {msg}");
    assert!(!c.is_alive());
    let err = c.call_tool("echo", json!({"text": "x"}), no_cancel(), T).unwrap_err();
    assert!(format!("{err:#}").contains("not running"), "{err:#}");
    assert!(c.list_tools(T).is_err());
    drop(c);
    assert!(!process_exists(pid));
}

#[test]
fn shutdown_and_drop_kill_the_process() {
    // Graceful: the server exits when stdin closes.
    let c = start();
    let pid = server_pid(&c);
    assert!(process_exists(pid));
    c.shutdown();
    assert!(!c.is_alive());
    assert!(!process_exists(pid), "server still running after shutdown");
    assert!(c.call_tool("echo", json!({"text": "x"}), no_cancel(), T).is_err());
    c.shutdown(); // idempotent

    // Stubborn: ignores stdin EOF, so shutdown must signal the process group.
    let c = Client::start(&config(&[("ECHO_IGNORE_EOF", "1")]), T).unwrap();
    let pid = server_pid(&c);
    let t0 = Instant::now();
    drop(c);
    assert!(t0.elapsed() < Duration::from_secs(4), "{:?}", t0.elapsed());
    assert!(!process_exists(pid), "stubborn server survived drop");
}

#[test]
fn start_failures() {
    let mut cfg = config(&[]);
    cfg.command = "/nonexistent/mcp-server-binary".into();
    let err = Client::start(&cfg, T).err().expect("spawn must fail");
    assert!(format!("{err:#}").contains("failed to start MCP server `echo`"), "{err:#}");

    let err = Client::start(&config(&[("ECHO_EXIT_IMMEDIATELY", "1")]), T)
        .err()
        .expect("dead server");
    assert!(format!("{err:#}").contains("boom, exiting immediately"), "{err:#}");

    let t0 = Instant::now();
    let err = Client::start(&config(&[("ECHO_NO_INIT", "1")]), Duration::from_millis(400))
        .err()
        .expect("handshake timeout");
    assert!(t0.elapsed() < Duration::from_secs(3));
    let msg = format!("{err:#}");
    assert!(msg.contains("did not answer `initialize`") && msg.contains("starting up"), "{msg}");
}
