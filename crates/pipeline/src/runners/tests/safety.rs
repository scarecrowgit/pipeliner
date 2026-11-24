//! Fixes from the independent review: setup, vanished worktrees, merges, test runs in place,
//! the registry, renders, odd file names, secrets and paths that leave the worktree.

use model::{ActionNode, AgentNode, NodeKind};

use super::*;
use crate::runners::sweep_test_runs;

fn coder(files_out: &[&str]) -> NodeKind {
    NodeKind::Agent(AgentNode {
        role: "You code.".into(),
        workdir: "code".into(),
        files_out: files_out.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    })
}

fn action(action: ActionNode) -> NodeKind {
    NodeKind::Action(action)
}

fn after(node: &str, kind: NodeKind, up: &NodeOutput) -> Job {
    let mut job = job(node, kind, 0, 1);
    job.upstream = vec![upstream("coder", 0, up)];
    job
}

/// Runs a coder that writes `file`, returning its output.
fn code(fx: &Fx, runner: &Runner, file: &'static str, text: &'static str) -> NodeOutput {
    fx.sessions.then(move |env, _| {
        write(env.workdir.as_ref().unwrap(), file, text);
        finished("Changed it.")
    });
    let (result, _) = run(runner, &job("coder", coder(&[]), 0, 1));
    assert!(matches!(result, JobResult::Done { .. }), "{result:?}");
    output(&result).clone()
}

fn error_of(result: &JobResult) -> &str {
    match result {
        JobResult::Failed { error, .. } => error,
        other => panic!("a failure expected: {other:?}"),
    }
}

#[test]
fn a_setup_that_failed_runs_again_when_the_worktree_is_reused() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let mut wd = workdir("code", &repo, "");
    wd.setup = "if [ -f \"$FLAG\" ]; then touch ready; else touch \"$FLAG\"; exit 1; fi".into();
    wd.env.insert("FLAG".into(), fx.root().join("flag").display().to_string());
    let runner = fx.runner(vec![wd]);
    let (first, _) = run(&runner, &job("coder", coder(&[]), 0, 1));
    assert!(error_of(&first).contains("setup"), "{first:?}");

    fx.sessions.then(|env, _| {
        assert!(env.workdir.as_ref().unwrap().join("ready").exists(), "the setup ran again");
        finished("ok")
    });
    let (second, _) = run(&runner, &job("coder", coder(&[]), 0, 1));
    assert!(matches!(second, JobResult::Done { .. }), "{second:?}");
}

#[test]
fn a_registered_worktree_that_is_gone_fails_the_job_instead_of_starting_fresh() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "b.txt", "b\n");
    std::fs::remove_dir_all(coded.worktree.as_ref().unwrap()).expect("remove");
    let command = action(ActionNode::Command {
        workdir: "code".into(),
        command: "test -f b.txt".into(),
        tests: true,
        parse: String::new(),
        timeout_secs: 30,
    });
    let (tests, _) = run(&runner, &after("tests", command, &coded));
    assert!(error_of(&tests).contains("worktree of coder is gone"), "{tests:?}");
    let render = action(ActionNode::Render { workdir: "code".into(), files: "*.html".into() });
    assert!(error_of(&run(&runner, &after("render", render, &coded)).0).contains("is gone"));
    let mut resumed = job("coder", coder(&[]), 0, 1);
    resumed.resume_session = coded.session.clone();
    assert!(error_of(&run(&runner, &resumed).0).contains("is gone"));
}

#[test]
fn a_merge_found_done_already_still_counts_as_merged() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "b.txt", "b\n");
    let merge = action(ActionNode::Merge { workdir: "code".into() });
    let (first, _) = run(&runner, &after("merge", merge.clone(), &coded));
    assert!(output(&first).merged, "{first:?}");
    let (again, _) = run(&runner, &after("merge", merge, &coded));
    let JobResult::Done { output, .. } = &again else { panic!("done: {again:?}") };
    assert!(output.summary.contains("already has"), "{}", output.summary);
    assert!(output.merged);
}

#[test]
fn a_test_run_never_works_in_the_checkout_itself() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let mut wd = workdir("code", &repo, "");
    wd.worktree = false;
    let plain = fx.root().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let mut other = workdir("plain", &plain, "");
    other.worktree = false;
    let runner = fx.runner(vec![wd, other]);
    fx.sessions.then(|env, _| {
        write(env.workdir.as_ref().unwrap(), "a.txt", "changed\n");
        finished("ok")
    });
    let mut test = job("coder", coder(&[]), 0, 1);
    test.test = true;
    let (done, _) = run(&runner, &test);
    let path = output(&done).worktree.clone().expect("a throwaway worktree");
    assert!(path.starts_with(fx.worktrees().join("_test")), "{path:?}");
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a\n", "the checkout is untouched");

    let mut refused = job("plain", NodeKind::Agent(AgentNode { role: "x".into(), workdir: "plain".into(), ..Default::default() }), 0, 1);
    refused.test = true;
    assert!(error_of(&run(&runner, &refused).0).contains("not a git repository"));
}

#[test]
fn a_damaged_registry_is_an_error_and_writes_leave_no_temp_files() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "b.txt", "b\n");
    let dir = fx.root().join("state/runs/run1");
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
    std::fs::write(dir.join("places.json"), "[{\"workdir\": ").unwrap();
    let mut resumed = job("coder", coder(&[]), 0, 1);
    resumed.resume_session = coded.session.clone();
    assert!(error_of(&run(&runner, &resumed).0).contains("damaged"));
    assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 2, "no new worktree was made");
}

#[test]
fn a_render_against_a_base_that_does_not_exist_fails() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("home.html", "<p>old</p>")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "home.html", "<p>new</p>");
    let mut wrong = workdir("code", &repo, "");
    wrong.base_branch = "no-such-base".into();
    let other = fx.runner(vec![wrong]);
    let render = action(ActionNode::Render { workdir: "code".into(), files: "*.html".into() });
    let (result, _) = run(&other, &after("render", render, &coded));
    assert!(error_of(&result).contains("no-such-base"), "{result:?}");
}

#[test]
fn files_with_non_ascii_names_are_rendered_by_their_real_name() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("screens/главная.html", "<p>old</p>")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "screens/главная.html", "<p>new</p>");
    write(coded.worktree.as_ref().unwrap(), "screens/экран.html", "<p>fresh</p>");
    let render = action(ActionNode::Render { workdir: "code".into(), files: "screens/*.html".into() });
    let (result, _) = run(&runner, &after("render", render, &coded));
    let paths: Vec<&str> = output(&result).renders.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["screens/главная.html", "screens/экран.html"], "{result:?}");
    assert!(output(&result).renders[0].before.is_some());
}

#[test]
fn configured_secrets_never_leave_a_job() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let mut config = fx.config();
    config.secrets = vec!["hunter2-provider-key".into()];
    let runner = fx.runner_with(vec![workdir("code", &repo, "")], config);
    let command = action(ActionNode::Command {
        workdir: "code".into(),
        command: "echo token=hunter2-provider-key; exit 3".into(),
        tests: false,
        parse: String::new(),
        timeout_secs: 30,
    });
    let (result, notes) = run(&runner, &job("cmd", command, 0, 1));
    assert!(!error_of(&result).contains("hunter2") && error_of(&result).contains("[redacted]"), "{result:?}");
    assert!(notes.iter().all(|n| !format!("{n:?}").contains("hunter2")), "{notes:?}");

    let mut report = job("report", action(ActionNode::Report), 0, 1);
    report.item.title = "Leaked hunter2-provider-key".into();
    run(&runner, &report);
    let posts = fx.records.posts.lock().unwrap();
    assert_eq!(posts[0].title, "HAR-7: Leaked [redacted]");
}

#[test]
fn required_files_must_stay_inside_the_worktree() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let (climbs, _) = run(&runner, &job("coder", coder(&["../outside.txt"]), 0, 1));
    assert!(error_of(&climbs).contains("inside the worktree"), "{climbs:?}");
    let (absolute, _) = run(&runner, &job("other", coder(&["/etc/passwd"]), 0, 1));
    assert!(error_of(&absolute).contains("inside the worktree"), "{absolute:?}");

    let outside = fx.root().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.clone();
    fx.sessions.then(move |env, _| {
        let dir = env.workdir.as_ref().unwrap();
        std::os::unix::fs::symlink(&target, dir.join("link")).unwrap();
        std::fs::write(target.join("out.txt"), "x").unwrap();
        finished("Wrote it.")
    });
    let (linked, _) = run(&runner, &job("third", coder(&["link/out.txt"]), 0, 1));
    assert!(error_of(&linked).contains("leads outside"), "{linked:?}");
}

#[test]
fn leftovers_of_test_runs_that_no_longer_exist_are_swept() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    fx.sessions.then(|_, _| finished("ok"));
    let mut test = job("coder", coder(&[]), 0, 1);
    test.test = true;
    test.run = RunId::new("t1");
    let path = output(&run(&runner, &test).0).worktree.clone().unwrap();
    // A real run's records are never swept, even when its run is gone.
    let real = code(&fx, &runner, "b.txt", "b\n");

    assert!(sweep_test_runs(&fx.config(), &|_| true).is_empty());
    assert!(path.is_dir());
    let swept = sweep_test_runs(&fx.config(), &|_| false);
    assert_eq!(swept, [RunId::new("t1")], "only the run of test branches goes");
    assert!(!path.exists());
    assert!(git(&repo, &["branch", "--list", "harness-test/*"]).trim().is_empty());
    assert!(real.worktree.unwrap().is_dir());
}
