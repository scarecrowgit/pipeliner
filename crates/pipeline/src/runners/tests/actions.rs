use model::{ActionNode, AgentNode, IssueStatus, NodeKind, TestResult};

use super::*;

fn action(action: ActionNode) -> NodeKind {
    NodeKind::Action(action)
}

/// A coder that changes `file` in its worktree; returns what it left behind.
fn code(fx: &Fx, runner: &Runner, file: &'static str, text: &'static str, test: bool) -> NodeOutput {
    fx.sessions.then(move |env, _| {
        write(env.workdir.as_ref().unwrap(), file, text);
        finished("Changed it.")
    });
    let kind = NodeKind::Agent(AgentNode { role: "You code.".into(), workdir: "code".into(), ..Default::default() });
    let mut job = job("coder", kind, 0, 1);
    job.test = test;
    let (result, _) = run(runner, &job);
    assert!(matches!(result, JobResult::Done { .. }), "{result:?}");
    output(&result).clone()
}

fn after(node: &str, kind: NodeKind, upstream_of: &NodeOutput) -> Job {
    let mut job = job(node, kind, 0, 1);
    job.upstream = vec![super::upstream("coder", 0, upstream_of)];
    job
}

#[test]
fn a_tests_command_reports_counts_and_a_failing_run_is_a_result() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let script = "echo 'test result: ok. 3 passed; 0 failed; 0 ignored'; echo 'test result: FAILED. 4 passed; 2 failed; 1 ignored'; exit 101";
    let command = |parse: &str, tests: bool| {
        action(ActionNode::Command { workdir: "code".into(), command: script.into(), tests, parse: parse.into(), timeout_secs: 30 })
    };
    let (result, notes) = run(&runner, &job("tests", command("cargo-test", true), 0, 1));
    let JobResult::Done { output, .. } = &result else { panic!("a tests result: {result:?}") };
    assert_eq!(output.tests, Some(TestResult { passed: 7, total: 9 }));
    assert_eq!(output.tests_ok, Some(false));
    assert!(output.summary.contains("7 of 9"), "{}", output.summary);
    assert!(output.worktree.as_ref().is_some_and(|w| w.starts_with(fx.worktrees())), "it runs in a worktree");
    assert!(notes.iter().any(|n| matches!(n, JobNote::Log { text, .. } if text.contains("FAILED"))));

    let (regex, _) = run(&runner, &job("tests", command(r"(?P<passed>\d+) passed; (?P<failed>\d+) failed", true), 0, 1));
    assert_eq!(super::output(&regex).tests, Some(TestResult { passed: 7, total: 9 }));

    let (plain, _) = run(&runner, &job("build", command("", false), 0, 1));
    assert!(matches!(&plain, JobResult::Failed { error, .. } if error.contains("101")), "{plain:?}");
}

#[test]
fn merge_lands_the_branch_on_the_base_or_says_what_conflicted() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx, &runner, "b.txt", "b\n", false);
    let merge = action(ActionNode::Merge { workdir: "code".into() });
    let (merged, _) = run(&runner, &after("merge", merge.clone(), &coded));
    assert!(matches!(&merged, JobResult::Done { output, .. } if output.summary.contains("Merged")), "{merged:?}");
    assert_eq!(git(&repo, &["show", "main:b.txt"]), "b\n");

    // Main moves on in the same file another branch changes.
    let fx2 = Fx::new();
    let repo = fx2.repo("app", &[("a.txt", "a\n")]);
    let runner = fx2.runner(vec![workdir("code", &repo, "")]);
    let coded = code(&fx2, &runner, "a.txt", "theirs\n", false);
    write(&repo, "a.txt", "ours\n");
    git(&repo, &["commit", "-qam", "ours"]);
    let (conflict, _) = run(&runner, &after("merge", merge, &coded));
    assert!(matches!(&conflict, JobResult::Failed { error, .. } if error.contains("conflicts in a.txt")), "{conflict:?}");
    assert_eq!(git(&repo, &["show", "main:a.txt"]), "ours\n");
}

#[test]
fn a_pull_request_pushes_and_opens_through_gh_but_a_test_run_leaves_the_remote_alone() {
    let fx = Fx::new();
    let remote = fx.root().join("remote.git");
    git(&fx.root(), &["init", "-q", "--bare", remote.to_str().unwrap()]);
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&repo, &["push", "-q", "origin", "main"]);
    let runner = fx.runner(vec![workdir("code", &repo, "fix/")]);
    let pr = action(ActionNode::PullRequest { workdir: "code".into(), draft: true, push: true });

    let coded = code(&fx, &runner, "b.txt", "b\n", true);
    let mut simulated = after("pr", pr.clone(), &coded);
    simulated.test = true;
    let (result, notes) = run(&runner, &simulated);
    assert!(matches!(&result, JobResult::Done { output, .. } if output.pr_url.is_none() && output.summary.starts_with("Test run")), "{result:?}");
    assert!(notes.iter().any(|n| matches!(n, JobNote::Log { text, .. } if text.contains("push"))));
    assert!(git(&remote, &["branch", "--list"]).trim().ends_with("main"), "only main on the remote");
    assert!(fx.gh.calls.lock().unwrap().is_empty());
    assert!(fx.records.posts.lock().unwrap().is_empty());
    runner.cleanup(&RunId::new("run1"));

    let coded = code(&fx, &runner, "b.txt", "b\n", false);
    let (opened, _) = run(&runner, &after("pr", pr, &coded));
    let JobResult::Done { output: out, .. } = &opened else { panic!("opened: {opened:?}") };
    assert_eq!(out.pr_url.as_deref(), Some(PR_URL));
    assert!(git(&remote, &["branch", "--list"]).contains("fix/har-7"));
    let calls = fx.gh.calls.lock().unwrap();
    let create = calls.iter().find(|c| c[1] == "create").expect("gh pr create");
    assert!(create.contains(&"--draft".to_string()) && create.contains(&"HAR-7: Fix the retry".to_string()), "{create:?}");
    let posts = fx.records.posts.lock().unwrap();
    assert_eq!(posts[0].links[0].url, PR_URL);
    assert_eq!(fx.tracker.prs.lock().unwrap().as_slice(), [PR_URL.to_string()]);
}

#[test]
fn render_makes_before_and_after_pictures_of_changed_files() {
    let fx = Fx::new();
    let repo = fx.repo("mockups", &[("screens/home.html", "<p>old</p>"), ("screens/other.html", "<p>x</p>")]);
    let runner = fx.runner(vec![workdir("code", &repo, "design/")]);
    let coded = code(&fx, &runner, "screens/home.html", "<p>new</p>", false);
    write(coded.worktree.as_ref().unwrap(), "screens/fresh.html", "<p>fresh</p>");
    let render = action(ActionNode::Render { workdir: "code".into(), files: "screens/*.html".into() });
    let (result, _) = run(&runner, &after("render", render, &coded));
    let JobResult::Done { output, .. } = &result else { panic!("rendered: {result:?}") };
    let paths: Vec<&str> = output.renders.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["screens/fresh.html", "screens/home.html"]);
    assert!(output.renders[0].before.is_none(), "a new file has no before");
    let home = &output.renders[1];
    let blobs = fx.records.blobs.lock().unwrap();
    let blob = |id: &BlobId| blobs[id.as_str().trim_start_matches("blob").parse::<usize>().unwrap() - 1].clone();
    assert_eq!(blob(&home.after), b"<p>new</p>");
    assert_eq!(blob(home.before.as_ref().unwrap()), b"<p>old</p>");
}

#[test]
fn the_before_picture_of_a_large_file_is_made_from_the_whole_file() {
    // Mockups weigh over 100 KB; a command's captured output keeps only its head and tail.
    let old = format!("<style>{}</style><p>old</p>", "a{color:red}".repeat(12_000));
    let fx = Fx::new();
    let repo = fx.repo("mockups", &[("screens/home.html", &old)]);
    let runner = fx.runner(vec![workdir("code", &repo, "design/")]);
    let coded = code(&fx, &runner, "screens/home.html", "<p>new</p>", false);
    let render = action(ActionNode::Render { workdir: "code".into(), files: "screens/*.html".into() });
    let (result, _) = run(&runner, &after("render", render, &coded));
    let JobResult::Done { output, .. } = &result else { panic!("rendered: {result:?}") };
    let blobs = fx.records.blobs.lock().unwrap();
    let before = output.renders[0].before.as_ref().expect("a before picture");
    let blob = &blobs[before.as_str().trim_start_matches("blob").parse::<usize>().unwrap() - 1];
    assert_eq!(blob.len(), old.len(), "the base file arrived whole");
    assert!(blob == old.as_bytes());
}

#[test]
fn reports_and_issue_updates_go_to_their_services_except_in_test_runs() {
    let fx = Fx::new();
    let runner = fx.runner(Vec::new());
    let update = action(ActionNode::UpdateIssue { status: Some(IssueStatus::InReview), comment: "Ready: {key}".into() });
    let (moved, _) = run(&runner, &job("move", update.clone(), 0, 1));
    assert!(matches!(moved, JobResult::Done { .. }));
    assert_eq!(fx.tracker.updates.lock().unwrap()[0], (IssueKey::new("HAR-7"), Some(IssueStatus::InReview), "Ready: HAR-7".into()));
    let mut test = job("move", update, 0, 1);
    test.test = true;
    run(&runner, &test);
    assert_eq!(fx.tracker.updates.lock().unwrap().len(), 1);

    let (reported, _) = run(&runner, &job("report", action(ActionNode::Report), 0, 1));
    assert!(matches!(reported, JobResult::Done { .. }));
    assert_eq!(fx.records.posts.lock().unwrap()[0].title, "HAR-7: Fix the retry");
}

#[test]
fn keys_are_redacted_from_bodies() {
    let text = crate::runners::publish::redact("key sk-ant-REDACTME0123456789abcdef here", &[]);
    assert_eq!(text, "key [redacted] here");
}
