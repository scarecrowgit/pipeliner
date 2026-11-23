use std::time::{Duration, Instant};

use model::ToolDisplay;
use serde_json::json;

use super::Fixture;
use crate::Bash;
use crate::api::Tool;

/// Whether a process is gone (no entry, or a zombie waiting to be reaped).
fn gone(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat.rsplit(") ").next().is_some_and(|rest| rest.starts_with('Z')),
    }
}

fn wait_gone(pid: u32) -> bool {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        if gone(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn read_pid(f: &Fixture) -> u32 {
    f.read("pid").trim().parse().expect("pid file")
}

#[test]
fn runs_in_the_root_with_env_and_reports_exit_code() {
    let mut f = Fixture::new();
    f.env.insert("HARNESS_TEST_VAR".into(), "hello".into());
    f.write("marker.txt", "");
    let out = f.run(&Bash, json!({"command": "ls; echo $HARNESS_TEST_VAR; echo oops >&2"}));
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("marker.txt") && out.content.contains("hello\n") && out.content.contains("oops"));
    assert!(out.content.ends_with("[Exit code 0]\n"), "{}", out.content);
    assert!(matches!(out.display, ToolDisplay::Command { exit_code: Some(0), .. }));

    let failed = f.run(&Bash, json!({"command": "exit 3"}));
    assert!(failed.is_error && failed.content.contains("(no output)") && failed.content.contains("[Exit code 3]"));
    assert!(matches!(failed.display, ToolDisplay::Command { exit_code: Some(3), .. }));
}

#[test]
fn subject_and_preview_are_the_command() {
    let f = Fixture::new();
    let input = json!({"command": "  bun test src  "});
    assert_eq!(Bash.subject(&input), "bun test src");
    let preview = Bash.preview(&f.ctx(), &input);
    assert_eq!(preview, ToolDisplay::Command { command: "bun test src".into(), output: String::new(), exit_code: None });
    assert!(!f.path("anything").exists());
}

#[test]
fn bad_input_is_explained() {
    let f = Fixture::new();
    let missing = f.run(&Bash, json!({}));
    assert!(missing.is_error && missing.content.contains("`command`"));
    let empty = f.run(&Bash, json!({"command": "   "}));
    assert!(empty.is_error && empty.content.contains("must not be empty"));
    let bad_timeout = f.run(&Bash, json!({"command": "true", "timeout_secs": "soon"}));
    assert!(bad_timeout.is_error && bad_timeout.content.contains("timeout_secs"));
}

#[test]
fn output_is_capped_head_and_tail() {
    let f = Fixture::new();
    let out = f.run(&Bash, json!({"command": "echo FIRST; seq 1 100000; echo LAST"}));
    assert!(out.content.contains("FIRST") && out.content.contains("LAST"));
    assert!(out.content.len() < 40 * 1024, "{}", out.content.len());
    assert!(out.content.contains("only its start and its end"));
}

#[test]
fn timeout_kills_the_whole_tree() {
    let f = Fixture::new();
    let started = Instant::now();
    let out = f.run(&Bash, json!({"command": "sleep 30 & echo $! > pid; wait", "timeout_secs": 1}));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(out.is_error && out.content.contains("Timed out after 1 s"), "{}", out.content);
    assert!(matches!(out.display, ToolDisplay::Command { exit_code: None, .. }));
    assert!(wait_gone(read_pid(&f)), "background sleep survived the timeout");
}

#[test]
fn cancel_stops_the_whole_tree() {
    let f = Fixture::new();
    let token = f.cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        token.cancel();
    });
    let started = Instant::now();
    let out = f.run(&Bash, json!({"command": "sleep 30 & echo $! > pid; wait"}));
    canceller.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(out.is_error && out.content.contains("Cancelled"), "{}", out.content);
    assert!(wait_gone(read_pid(&f)), "background sleep survived the cancel");
}

#[test]
fn already_cancelled_runs_nothing() {
    let f = Fixture::new();
    f.cancel.cancel();
    let out = f.run(&Bash, json!({"command": "touch made"}));
    assert!(out.is_error && out.content.contains("Cancelled"));
    assert!(!f.path("made").exists());
}
