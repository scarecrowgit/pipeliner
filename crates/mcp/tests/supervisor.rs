//! The supervisor against real processes: `examples/echo_server.rs`.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use base::cancel::CancelToken;
use mcp::{ServerConfig, Supervisor, Timeouts};
use model::workspace::{McpServerInfo as Info, McpState};
use serde_json::{Value, json};

/// Path of the built `echo_server` example; built once if `cargo test` has not done it.
fn echo_server() -> PathBuf {
    // Test binaries live in `<target>/<profile>/deps/`, examples in `<target>/<profile>/examples/`.
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(|deps| deps.parent()).unwrap().to_path_buf();
    let path = profile_dir.join("examples").join("echo_server");
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        if path.exists() {
            return;
        }
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--quiet", "-p", "mcp", "--example", "echo_server"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(Stdio::null())
            .status()
            .expect("running cargo build for echo_server");
        assert!(status.success(), "building echo_server failed");
    });
    assert!(path.exists(), "{} missing", path.display());
    path
}

fn echo(name: &str, env: &[(&str, &str)]) -> ServerConfig {
    let mut cfg = ServerConfig::new(name, echo_server().to_string_lossy());
    cfg.env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    cfg
}

fn timeouts() -> Timeouts {
    Timeouts { start: Duration::from_secs(10), call: Duration::from_secs(10), liveness: Duration::from_millis(200) }
}

struct Fixture {
    sup: Supervisor,
    changes: Arc<AtomicUsize>,
}

fn start(configs: Vec<ServerConfig>, timeouts: Timeouts) -> Fixture {
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = changes.clone();
    let sup = Supervisor::start_with(configs, Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }), timeouts);
    Fixture { sup, changes }
}

impl Fixture {
    fn state(&self, name: &str) -> Info {
        self.sup.servers().into_iter().find(|s| s.name == name).expect("server listed")
    }

    /// Waits until the state of `name` satisfies `done`, at most 15 seconds.
    fn wait(&self, name: &str, done: impl Fn(McpState) -> bool) -> Info {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            let info = self.state(name);
            if done(info.state) || Instant::now() > end {
                return info;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn settled(&self, name: &str) -> Info {
        self.wait(name, |s| s != McpState::Starting)
    }

    fn call(&self, id: &str, args: Value) -> mcp::CallResult {
        self.sup.call(id, args, &CancelToken::new())
    }

    fn pid(&self, server: &str) -> u32 {
        let r = self.call(&format!("mcp__{server}__pid"), json!({}));
        r.text.parse().unwrap_or_else(|_| panic!("pid: {r:?}"))
    }
}

/// Whether `pid` is gone (or a zombie), waiting up to three seconds for it.
fn gone(pid: u32) -> bool {
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        let dead = match stat {
            Err(_) => true,
            Ok(s) => s.rsplit_once(") ").is_some_and(|(_, rest)| rest.starts_with('Z')),
        };
        if dead {
            return true;
        }
        if Instant::now() > end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn servers_start_in_parallel_and_failures_are_reported() {
    let mut missing = ServerConfig::new("missing", "/nonexistent/mcp-server");
    missing.env.insert("API_KEY".into(), "very-secret-value".into());
    let mut off = echo("off", &[]);
    off.enabled = false;
    let short = Timeouts { start: Duration::from_millis(1500), ..timeouts() };
    let f = start(
        vec![echo("slowpoke", &[("ECHO_NO_INIT", "1")]), echo("a", &[]), echo("b", &[]), missing, off],
        short,
    );
    let begun = Instant::now();
    assert_eq!(f.settled("a").state, McpState::Ready);
    assert_eq!(f.settled("b").state, McpState::Ready);
    assert!(begun.elapsed() < Duration::from_millis(1400), "a and b waited for slowpoke");
    assert_eq!(f.state("slowpoke").state, McpState::Starting);

    let info = f.settled("slowpoke");
    assert_eq!(info.state, McpState::Failed);
    let error = info.error.unwrap();
    assert!(error.contains("no answer to `initialize` within 1 s"), "{error}");
    assert!(error.contains("echo_server: starting up"), "stderr tail missing: {error}");

    let info = f.settled("missing");
    assert_eq!(info.state, McpState::Failed);
    let error = info.error.unwrap();
    assert!(error.contains("could not start `/nonexistent/mcp-server`"), "{error}");
    assert_eq!(f.state("off").state, McpState::Disabled);

    let a = f.state("a");
    assert_eq!((a.tools, a.source.as_str(), a.error), (6, "config.toml", None));
    let ids: Vec<String> = f.sup.tools().into_iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 12);
    assert_eq!(ids[0], "mcp__a__echo");
    assert!(ids.contains(&"mcp__b__spawn_child".to_string()));
    assert!(f.changes.load(Ordering::SeqCst) >= 4);
    assert!(!format!("{:?}", f.sup.servers()).contains("very-secret-value"));
}

#[test]
fn calls_results_and_approval() {
    let mut cfg = echo("e", &[]);
    cfg.auto_approve = vec!["echo".into()];
    let f = start(vec![cfg], Timeouts { call: Duration::from_millis(400), ..timeouts() });
    assert_eq!(f.settled("e").state, McpState::Ready);
    let tool = f.sup.tools().into_iter().find(|t| t.name == "echo").unwrap();
    assert_eq!((tool.id.as_str(), tool.spec.name.as_str()), ("mcp__e__echo", "mcp__e__echo"));
    assert_eq!(tool.spec.input_schema["properties"]["text"]["type"], "string");

    let r = f.call("mcp__e__echo", json!({"text": "hi there"}));
    assert_eq!((r.text.as_str(), r.is_error), ("hi there", false));
    let r = f.call("mcp__e__echo", json!("{\"text\": \"as a string\"}"));
    assert_eq!(r.text, "as a string");
    let r = f.call("mcp__e__fail", Value::Null);
    assert_eq!((r.text.as_str(), r.is_error), ("something went wrong", true));
    assert!(f.sup.is_auto_approved("mcp__e__echo"));
    assert!(!f.sup.is_auto_approved("mcp__e__fail"));

    let started = Instant::now();
    let r = f.call("mcp__e__slow", json!({"ms": 20_000}));
    assert!(r.is_error && r.text.contains("timed out after 400 ms"), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(3));

    let cancel = CancelToken::new();
    let remote = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        remote.cancel();
    });
    let r = f.sup.call("mcp__e__slow", json!({"ms": 20_000}), &cancel);
    assert!(r.is_error && r.text.contains("cancelled"), "{r:?}");
    assert_eq!(f.call("mcp__e__echo", json!({"text": "still fine"})).text, "still fine");
    assert_eq!(f.state("e").state, McpState::Ready);
}

#[test]
fn a_killed_server_fails_and_reconnects() {
    let f = start(vec![echo("k", &[])], timeouts());
    assert_eq!(f.settled("k").state, McpState::Ready);
    let pid = f.pid("k");
    let killed = Command::new("kill").args(["-KILL", &pid.to_string()]).status().unwrap();
    assert!(killed.success());
    let before = f.changes.load(Ordering::SeqCst);
    let info = f.wait("k", |s| s != McpState::Ready);
    assert_eq!(info.state, McpState::Failed);
    assert!(info.error.as_deref().unwrap().contains("signal: 9"), "{info:?}");
    assert_eq!(info.tools, 0);
    assert!(f.changes.load(Ordering::SeqCst) > before);
    assert!(f.sup.tools().is_empty());
    let r = f.call("mcp__k__echo", json!({"text": "x"}));
    assert!(r.is_error && r.text.contains("is not running"), "{r:?}");

    assert!(f.sup.reconnect("k"));
    assert_eq!(f.settled("k").state, McpState::Ready);
    assert_ne!(f.pid("k"), pid);
    assert_eq!(f.call("mcp__k__echo", json!({"text": "back"})).text, "back");
}

#[test]
fn reconnect_replaces_a_running_server() {
    let f = start(vec![echo("r", &[])], timeouts());
    assert_eq!(f.settled("r").state, McpState::Ready);
    let old = f.pid("r");
    assert!(f.sup.reconnect("r"));
    assert_eq!(f.settled("r").state, McpState::Ready);
    assert_ne!(f.pid("r"), old);
    assert!(gone(old), "the replaced server still runs");
}

#[test]
fn a_dying_call_marks_the_server_failed() {
    let f = start(vec![echo("d", &[])], timeouts());
    assert_eq!(f.settled("d").state, McpState::Ready);
    let r = f.call("mcp__d__exit", json!({"code": 3}));
    assert!(r.is_error && r.text.contains("stopped"), "{r:?}");
    let info = f.wait("d", |s| s != McpState::Ready);
    assert_eq!(info.state, McpState::Failed);
    let error = info.error.unwrap();
    assert!(error.contains("exiting on request"), "{error}");
}

#[test]
fn broken_servers_fail_clearly() {
    let f = start(
        vec![echo("early", &[("ECHO_EXIT_IMMEDIATELY", "1")]), echo("garbage", &[("ECHO_GARBAGE", "1")])],
        Timeouts { start: Duration::from_millis(800), ..timeouts() },
    );
    let early = f.settled("early");
    assert_eq!(early.state, McpState::Failed);
    let error = early.error.unwrap();
    // Either end may notice first: the reader sees EOF, or the writer a broken pipe.
    let closed = error.contains("closed its output") || error.contains("stopped reading its input");
    assert!(closed && error.contains("exit status: 2"), "{error}");
    assert!(error.contains("boom, exiting immediately"), "{error}");

    let garbage = f.settled("garbage");
    assert_eq!(garbage.state, McpState::Failed);
    let error = garbage.error.unwrap();
    assert!(error.contains("no answer to `initialize`"), "{error}");
    assert!(error.contains("ignored a non-JSON line on stdout: garbage instead"), "{error}");
}

#[test]
fn shutdown_leaves_no_process() {
    let f = start(vec![echo("polite", &[]), echo("stubborn", &[("ECHO_IGNORE_EOF", "1")])], timeouts());
    assert_eq!(f.settled("polite").state, McpState::Ready);
    assert_eq!(f.settled("stubborn").state, McpState::Ready);
    let mut pids = vec![f.pid("polite"), f.pid("stubborn")];
    for server in ["polite", "stubborn"] {
        let r = f.call(&format!("mcp__{server}__spawn_child"), json!({}));
        pids.push(r.text.parse().unwrap_or_else(|_| panic!("{r:?}")));
    }
    let started = Instant::now();
    f.sup.shutdown();
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    for pid in pids {
        assert!(gone(pid), "process {pid} survived shutdown");
    }
    let info = f.state("polite");
    assert_eq!((info.state, info.error.as_deref()), (McpState::Failed, Some("Stopped")));
    assert!(f.sup.tools().is_empty());
    assert!(f.call("mcp__polite__echo", json!({})).is_error);
    assert!(!f.sup.reconnect("polite"));
    f.sup.shutdown();
}

#[test]
fn drop_stops_servers_even_while_starting() {
    let f = start(vec![echo("up", &[]), echo("hung", &[("ECHO_NO_INIT", "1")])], timeouts());
    assert_eq!(f.settled("up").state, McpState::Ready);
    let pid = f.pid("up");
    let started = Instant::now();
    drop(f);
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    assert!(gone(pid));
}

#[test]
fn a_configured_supervisor_stays_idle_until_asked_to_start() {
    let changes = Arc::new(AtomicUsize::new(0));
    let counter = changes.clone();
    let mut off = echo("off", &[]);
    off.enabled = false;
    let on_change: mcp::OnChange = Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let f = Fixture { sup: Supervisor::configure_with(vec![echo("lazy", &[]), off], on_change, timeouts()), changes };
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(f.state("lazy").state, McpState::Idle, "nothing starts before it is asked to");
    assert_eq!(f.state("off").state, McpState::Disabled);
    assert!(f.sup.tools().is_empty());
    let r = f.call("mcp__lazy__echo", json!({"text": "x"}));
    assert!(r.is_error && r.text.contains("has not been started yet"), "{r:?}");

    assert_eq!(f.sup.start_idle(), 1, "only the enabled idle server starts");
    assert_eq!(f.wait("lazy", |s| s != McpState::Starting && s != McpState::Idle).state, McpState::Ready);
    assert_eq!(f.sup.start_idle(), 0, "a started server is not started twice");
    assert_eq!(f.state("off").state, McpState::Disabled);
    assert!(f.changes.load(Ordering::SeqCst) >= 2, "the start and the ready state were reported");
}
