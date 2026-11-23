use super::*;
use std::fs;
use std::sync::Arc;

fn run(cmd: Cmd) -> Output {
    cmd.run(&CancelToken::new()).expect("command should start")
}

/// Whether the process is gone or only a zombie that nobody has reaped yet.
fn dead(pid: i32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')),
    }
}

fn wait_dead(pid: i32) -> bool {
    for _ in 0..100 {
        if dead(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn captures_stdout_stderr_and_exit_code() {
    let out = run(Cmd::shell("echo out; echo err >&2; exit 3"));
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
    assert_eq!(out.code, Some(3));
    assert!(!out.success());
    assert!(!out.timed_out && !out.cancelled && !out.truncated);
    assert_eq!(out.combined(), "out\nerr\n");
}

#[test]
fn success_for_zero_exit() {
    let out = run(Cmd::new("true"));
    assert!(out.success());
    assert_eq!(out.code, Some(0));
    assert_eq!(out.combined(), "");
}

#[test]
fn combined_separates_streams() {
    let out = Output { stdout: "a".into(), stderr: "b".into(), ..Output::default() };
    assert_eq!(out.combined(), "a\nb");
    let out = Output { stdout: String::new(), stderr: "b".into(), ..Output::default() };
    assert_eq!(out.combined(), "b");
    let out = Output { stdout: "a".into(), ..Output::default() };
    assert_eq!(out.combined(), "a");
}

#[test]
fn args_cwd_and_env() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(Cmd::new("sh").args(["-c", "printf '%s|%s|' \"$1\" \"$HARNESS_TEST\"; pwd", "sh", "arg"])
        .cwd(dir.path())
        .env("HARNESS_TEST", "value"));
    let real = fs::canonicalize(dir.path()).expect("canonical");
    assert_eq!(out.stdout, format!("arg|value|{}\n", real.display()));
}

#[test]
fn env_remove_hides_inherited_variable() {
    let script = "printf '%s' \"${HOME-unset}\"";
    assert_eq!(run(Cmd::shell(script).env_remove("HOME")).stdout, "unset");
    if let Some(home) = std::env::var_os("HOME") {
        assert_eq!(run(Cmd::shell(script)).stdout, home.to_string_lossy());
    }
    // Added variables win over a removal of the same name.
    assert_eq!(run(Cmd::shell(script).env_remove("HOME").env("HOME", "/x")).stdout, "/x");
}

#[test]
fn stdin_text_reaches_the_child() {
    let out = run(Cmd::new("cat").stdin("hello\nworld"));
    assert_eq!(out.stdout, "hello\nworld");
    assert!(out.success());
}

#[test]
fn large_stdin_does_not_deadlock() {
    let text = "x".repeat(1 << 20);
    let out = run(Cmd::new("cat").stdin(text.clone()).capture_limit(16, 16));
    assert!(out.success());
    assert!(out.truncated);
    assert_eq!(out.stdout.len(), 32);
}

#[test]
fn stdin_is_closed_when_not_given() {
    let out = run(Cmd::new("cat").timeout(Duration::from_secs(10)));
    assert!(out.success());
    assert!(!out.timed_out);
    assert_eq!(out.stdout, "");
}

#[test]
fn child_that_ignores_stdin_is_fine() {
    let out = run(Cmd::shell("echo done").stdin("x".repeat(1 << 20)));
    assert_eq!(out.stdout, "done\n");
    assert!(out.success());
}

#[test]
fn invalid_utf8_is_replaced() {
    let out = run(Cmd::shell("printf 'a\\377\\376b'; printf '\\300' >&2"));
    assert_eq!(out.stdout, "a\u{fffd}\u{fffd}b");
    assert_eq!(out.stderr, "\u{fffd}");
    assert!(out.success());
}

#[test]
fn missing_program_is_an_error() {
    let err = Cmd::new("definitely-not-a-program-7f3a").run(&CancelToken::new()).expect_err("must fail");
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

#[test]
fn missing_cwd_is_an_error() {
    let result = Cmd::new("true").cwd("/definitely/not/here").run(&CancelToken::new());
    assert!(result.is_err());
}

#[test]
fn chatty_child_never_blocks() {
    // Far more than a pipe buffer on both streams at once.
    let out = run(Cmd::shell("yes out | head -c 400000; yes err | head -c 400000 >&2")
        .timeout(Duration::from_secs(30))
        .capture_limit(100, 100));
    assert!(out.success(), "{out:?}");
    assert!(out.truncated);
    assert_eq!(out.stdout.len(), 200);
    assert_eq!(out.stderr.len(), 200);
}

#[test]
fn capture_keeps_head_and_tail() {
    let out = run(Cmd::shell("printf 'HEAD'; printf '%s' '................'; printf 'TAIL'")
        .capture_limit(4, 4));
    assert_eq!(out.stdout, "HEADTAIL");
    assert!(out.truncated);
}

#[test]
fn capture_under_limit_is_not_truncated() {
    let out = run(Cmd::shell("printf 'HEADTAIL'").capture_limit(4, 4));
    assert_eq!(out.stdout, "HEADTAIL");
    assert!(!out.truncated);
}

#[test]
fn capture_zero_tail_keeps_only_head() {
    let out = run(Cmd::shell("printf 'abcdefgh'").capture_limit(3, 0));
    assert_eq!(out.stdout, "abc");
    assert!(out.truncated);
}

#[test]
fn capture_limit_applies_per_stream() {
    let out = run(Cmd::shell("printf 'aaaa'; printf 'bbbb' >&2").capture_limit(4, 0));
    assert_eq!(out.stdout, "aaaa");
    assert_eq!(out.stderr, "bbbb");
    assert!(!out.truncated);
}

#[test]
fn default_limit_is_15kb_each_side() {
    let out = run(Cmd::shell("head -c 100000 /dev/zero | tr '\\0' x"));
    assert!(out.truncated);
    assert_eq!(out.stdout.len(), 30 * 1024);
}

#[test]
fn timeout_kills_the_child() {
    let start = Instant::now();
    let out = run(Cmd::new("sleep").arg("30").timeout(Duration::from_millis(200)));
    assert!(out.timed_out);
    assert!(!out.cancelled);
    assert!(!out.success());
    assert_eq!(out.code, None);
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    assert!(out.elapsed >= Duration::from_millis(200));
}

#[test]
fn timeout_keeps_output_written_so_far() {
    let out = run(Cmd::shell("echo started; sleep 30").timeout(Duration::from_millis(400)));
    assert!(out.timed_out);
    assert_eq!(out.stdout, "started\n");
}

#[test]
fn finishing_before_the_timeout_is_not_a_timeout() {
    let out = run(Cmd::shell("echo quick").timeout(Duration::from_secs(30)));
    assert!(!out.timed_out);
    assert!(out.success());
}

#[test]
fn child_that_ignores_sigterm_is_killed() {
    let start = Instant::now();
    let out = run(Cmd::shell("trap '' TERM; echo ready; while :; do sleep 1; done")
        .timeout(Duration::from_millis(300)));
    assert!(out.timed_out);
    assert_eq!(out.stdout, "ready\n");
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(700), "SIGKILL came before the grace period: {took:?}");
    assert!(took < Duration::from_secs(5), "took {took:?}");
}

#[test]
fn cancel_is_noticed_quickly() {
    let token = CancelToken::new();
    let remote = token.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        remote.cancel();
    });
    let start = Instant::now();
    let out = Cmd::new("sleep").arg("30").run(&token).expect("start");
    canceller.join().expect("canceller");
    assert!(out.cancelled);
    assert!(!out.timed_out);
    assert!(!out.success());
    assert!(start.elapsed() < Duration::from_secs(3), "took {:?}", start.elapsed());
}

#[test]
fn child_token_cancels_the_run() {
    let parent = CancelToken::new();
    let child = parent.child();
    let canceller = std::thread::spawn({
        let parent = parent.clone();
        move || {
            std::thread::sleep(Duration::from_millis(100));
            parent.cancel();
        }
    });
    let out = Cmd::new("sleep").arg("30").run(&child).expect("start");
    canceller.join().expect("canceller");
    assert!(out.cancelled);
}

#[test]
fn already_cancelled_token_starts_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("ran");
    let token = CancelToken::new();
    token.cancel();
    let out = Cmd::shell(&format!("touch {}", marker.display())).run(&token).expect("ok");
    assert!(out.cancelled);
    assert_eq!(out.code, None);
    assert!(!marker.exists());
}

#[test]
fn cancel_kills_grandchildren() {
    // `sh` starts a background `sleep` and waits for it. Killing only `sh` would leave the
    // sleep running (the cargo-under-nix-shell bug of the old runner).
    let token = Arc::new(CancelToken::new());
    let remote = Arc::clone(&token);
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        remote.cancel();
    });
    let out = Cmd::shell("sleep 30 & echo $!; wait").run(&token).expect("start");
    canceller.join().expect("canceller");
    assert!(out.cancelled);
    let pid: i32 = out.stdout.trim().parse().expect("grandchild pid on stdout");
    assert!(pid > 1);
    assert!(wait_dead(pid), "grandchild {pid} survived the cancel");
}

#[test]
fn timeout_kills_grandchildren() {
    let out = run(Cmd::shell("sleep 30 & echo $!; wait").timeout(Duration::from_millis(400)));
    assert!(out.timed_out);
    let pid: i32 = out.stdout.trim().parse().expect("grandchild pid on stdout");
    assert!(wait_dead(pid), "grandchild {pid} survived the timeout");
}

#[test]
fn leftover_background_process_does_not_hang_the_run() {
    // The shell exits at once but its background job keeps stdout open.
    let start = Instant::now();
    let out = run(Cmd::shell("sleep 30 & echo $!; echo hi"));
    assert!(out.success());
    assert!(out.stdout.ends_with("hi\n"));
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
    let pid: i32 = out.stdout.lines().next().expect("pid line").parse().expect("pid");
    assert!(wait_dead(pid), "leftover {pid} was not cleaned up");
}

#[test]
fn runs_in_its_own_process_group() {
    let out = run(Cmd::shell("echo $$; cat /proc/$$/stat"));
    let mut lines = out.stdout.lines();
    let pid: i32 = lines.next().expect("pid line").trim().parse().expect("pid");
    let stat = lines.next().expect("stat line");
    // Fields after the parenthesised name: state, ppid, pgrp.
    let after = stat.rsplit(')').next().expect("stat tail");
    let pgrp: i32 = after.split_whitespace().nth(2).expect("pgrp").parse().expect("number");
    assert_eq!(pgrp, pid, "the child must lead its own group");
}

#[test]
fn elapsed_is_measured() {
    let out = run(Cmd::shell("sleep 0.2"));
    assert!(out.elapsed >= Duration::from_millis(150));
    assert!(out.elapsed < Duration::from_secs(5));
}
