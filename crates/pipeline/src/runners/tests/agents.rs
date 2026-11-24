use ::agent::{Finish, Input};
use model::{AgentNode, FileMode, FileNode, NodeDef, NodeKind, StartFrom, SubagentNode};

use super::*;
use crate::api::Retry;

fn coder(files_out: Vec<String>) -> NodeKind {
    NodeKind::Agent(AgentNode { role: "You code.".into(), workdir: "code".into(), variants: 2, files_out, ..Default::default() })
}

#[test]
fn each_variant_gets_its_own_branch_and_worktree_and_reports_its_changes() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("src/lib.rs", "fn a() {}\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "fix/")]);
    for _ in 0..2 {
        fx.sessions.then(|env, _| {
            write(env.workdir.as_ref().expect("workdir"), "src/new.rs", "one\ntwo\n");
            finished("Added new.rs. Checked it builds.")
        });
    }
    let (a, notes) = run(&runner, &job("coder", coder(Vec::new()), 1, 1));
    let (b, _) = run(&runner, &job("coder", coder(Vec::new()), 2, 1));

    let JobResult::Done { port, output: a } = a else { panic!("done expected: {a:?}") };
    assert_eq!(port, "out");
    assert_eq!(a.branch.as_deref(), Some("fix/har-7-a"));
    assert_eq!(output(&b).branch.as_deref(), Some("fix/har-7-b"));
    let path = a.worktree.clone().expect("worktree");
    assert_eq!(path, fx.worktrees().join("app").join("fix-har-7-a"));
    assert_ne!(output(&b).worktree, a.worktree);
    assert_eq!(a.summary, "Added new.rs. Checked it builds.");
    assert_eq!(a.files.len(), 1);
    assert_eq!((a.files[0].path.as_str(), a.files[0].added), ("src/new.rs", 2));
    assert_eq!(a.session.as_ref().map(|s| s.as_str()), Some("s1"));
    assert!(notes.iter().any(|n| matches!(n, JobNote::Place { branch: Some(b), .. } if b == "fix/har-7-a")));
    assert!(notes.iter().any(|n| matches!(n, JobNote::Spent { .. })));

    let runs = fx.sessions.runs.lock().unwrap();
    let (spec, env, input) = &runs[0];
    assert!(matches!(spec.finish, Finish::Tool(_)));
    assert_eq!(env.run.as_ref().map(|r| r.as_str()), Some("run1"));
    let Input::Item(task) = input else { panic!("an item task") };
    assert!(task.contains("HAR-7: Fix the retry") && task.contains("It retries twice.") && task.contains("step_done"), "{task}");
    // The main checkout is untouched.
    assert!(!repo.join("src/new.rs").exists());
}

#[test]
fn a_send_back_builds_on_the_kept_branch_or_starts_over() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    fx.sessions.then(|env, _| {
        write(env.workdir.as_ref().unwrap(), "a.txt", "first\n");
        finished("First try.")
    });
    let (first, _) = run(&runner, &job("coder", coder(Vec::new()), 1, 1));
    let first = output(&first).clone();

    fx.sessions.then(|env, _| {
        assert_eq!(std::fs::read_to_string(env.workdir.as_ref().unwrap().join("a.txt")).unwrap(), "first\n");
        finished("Second try on top.")
    });
    let mut kept = job("coder", coder(Vec::new()), 1, 2);
    kept.retry = Some(Retry {
        notes: vec!["Make it shorter".into()],
        start: StartFrom::All,
        from_branch: first.branch.clone(),
        from_worktree: first.worktree.clone(),
        from_session: first.session.clone(),
    });
    let (second, _) = run(&runner, &kept);
    assert_eq!(output(&second).worktree, first.worktree, "{second:?}");
    assert_eq!(output(&second).session, first.session, "the conversation goes on");
    let input = fx.sessions.runs.lock().unwrap()[1].2.clone();
    let Input::User(text) = input else { panic!("feedback as a message") };
    assert!(text.text.contains("Make it shorter"));

    fx.sessions.then(|env, _| {
        assert_eq!(std::fs::read_to_string(env.workdir.as_ref().unwrap().join("a.txt")).unwrap(), "a\n");
        finished("Fresh start.")
    });
    let mut over = kept.clone();
    over.retry.as_mut().unwrap().start = StartFrom::Over;
    let (third, _) = run(&runner, &over);
    assert_eq!(output(&third).branch.as_deref(), Some("har-7-a-r2"), "{third:?}");
    assert_ne!(output(&third).session, first.session);
    let input = fx.sessions.runs.lock().unwrap()[2].2.clone();
    let Input::Item(task) = input else { panic!("a new task") };
    assert!(task.contains("Review feedback") && task.contains("Make it shorter"), "{task}");
}

#[test]
fn files_out_and_write_file_nodes_must_exist_before_the_node_is_done() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    fx.sessions.then(|_, _| finished("Did nothing."));
    let (missing, _) = run(&runner, &job("coder", coder(vec!["out/{key}/notes.md".into()]), 0, 1));
    let JobResult::Failed { error, .. } = &missing else { panic!("failed expected: {missing:?}") };
    assert!(error.contains("out/HAR-7/notes.md"), "{error}");

    let mut spec_job = job("writer", coder(Vec::new()), 0, 1);
    spec_job.attached = vec![
        NodeDef {
            id: "spec".into(),
            name: "Spec".into(),
            pos: Default::default(),
            kind: NodeKind::File(FileNode { workdir: "code".into(), path: "spec.md".into(), mode: FileMode::Write }),
        },
        NodeDef {
            id: "ctx".into(),
            name: "Docs".into(),
            pos: Default::default(),
            kind: NodeKind::File(FileNode { workdir: "code".into(), path: "*.txt".into(), mode: FileMode::Read }),
        },
        NodeDef {
            id: "helper".into(),
            name: "Helper".into(),
            pos: Default::default(),
            kind: NodeKind::Subagent(SubagentNode { role: "You help. Briefly.".into(), per_run: 2, ..Default::default() }),
        },
    ];
    fx.sessions.then(|env, _| {
        write(env.workdir.as_ref().unwrap(), "spec.md", "# Spec\n");
        finished("Wrote the spec.")
    });
    let (done, _) = run(&runner, &spec_job);
    assert!(matches!(done, JobResult::Done { .. }), "{done:?}");
    let runs = fx.sessions.runs.lock().unwrap();
    let (spec, _, Input::Item(task)) = &runs[1] else { panic!("an item task") };
    assert!(task.contains("`spec.md`") && task.contains("a.txt"), "{task}");
    assert_eq!(spec.templates.len(), 1);
    assert_eq!((spec.templates[0].name.as_str(), spec.templates[0].per_run), ("Helper", 2));
    assert!(spec.spawn.enabled);
}

#[test]
fn questions_budgets_and_failures_map_to_their_results() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    fx.sessions.then(|_, _| AgentOutcome { stop: StopCause::Waiting { question: "Which API?".into() }, ..finished("") });
    let (asked, _) = run(&runner, &job("coder", coder(Vec::new()), 0, 1));
    let JobResult::Ask { question, output: out } = &asked else { panic!("ask expected: {asked:?}") };
    assert_eq!(question, "Which API?");

    fx.sessions.then(|_, input| {
        assert!(matches!(input, Input::User(u) if u.text == "The v2 one"));
        AgentOutcome { stop: StopCause::Budget, ..finished("") }
    });
    let mut answered = job("coder", coder(Vec::new()), 0, 1);
    answered.answer = Some("The v2 one".into());
    answered.resume_session = out.session.clone();
    answered.budget_left_usd = Some(1.5);
    let (over, _) = run(&runner, &answered);
    assert!(matches!(over, JobResult::OverBudget { .. }), "{over:?}");
    assert_eq!(fx.sessions.runs.lock().unwrap()[1].0.budget_usd, Some(1.5));
    assert_eq!(output(&over).worktree, out.worktree, "the same place is found again");

    fx.sessions.then(|_, _| AgentOutcome { payload: None, ..finished("") });
    let (no_tool, _) = run(&runner, &job("other", coder(Vec::new()), 0, 1));
    assert!(matches!(&no_tool, JobResult::Failed { error, .. } if error.contains("step_done")), "{no_tool:?}");
}

#[test]
fn discard_removes_the_worktree_and_branch_but_never_a_folder_used_in_place() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "")]);
    fx.sessions.then(|_, _| finished("ok"));
    let (done, _) = run(&runner, &job("coder", coder(Vec::new()), 2, 1));
    let out = output(&done).clone();
    let path = out.worktree.clone().unwrap();
    assert!(path.is_dir());
    runner.discard(&RunId::new("run1"), &out);
    assert!(!path.exists());
    assert!(git(&repo, &["branch", "--list", "har-7-b"]).trim().is_empty());

    let in_place = NodeOutput { worktree: Some(repo.clone()), ..Default::default() };
    runner.discard(&RunId::new("run1"), &in_place);
    assert!(repo.join("a.txt").exists());
}

#[test]
fn a_test_run_works_in_a_throwaway_worktree_that_cleanup_removes() {
    let fx = Fx::new();
    let repo = fx.repo("app", &[("a.txt", "a\n")]);
    let runner = fx.runner(vec![workdir("code", &repo, "fix/")]);
    fx.sessions.then(|_, _| finished("ok"));
    let mut test = job("coder", coder(Vec::new()), 1, 1);
    test.test = true;
    let (done, _) = run(&runner, &test);
    let out = output(&done).clone();
    assert_eq!(out.branch.as_deref(), Some("harness-test/run1/har-7-a"));
    let path = out.worktree.unwrap();
    assert!(path.starts_with(fx.worktrees().join("_test")));
    runner.cleanup(&RunId::new("run1"));
    assert!(!path.exists());
    assert!(git(&repo, &["branch", "--list", "harness-test/*"]).trim().is_empty());
}
