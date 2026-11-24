//! Fixes from the independent review: drivers let go, merged issues, unsaved snapshots, one
//! run per issue, the daily limit, test-run leftovers and secrets.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use llm::Reply;
use model::graph::PORT_OUT;
use model::{
    ActionNode, AgentNode, IssueStatus, NodeKind, PipelineId, Project, ProjectId, ReviewDecision, RunState, RunnableRef,
    StartTriggers,
};
use serde_json::json;

use super::pipelines::{deploy, pipeline_fx, scripted, wait_run};
use super::pipelines_more::{graph, input, node, route};
use super::{Fx, git};
use crate::api::{CatalogApi, InboxApi, IssueApi, NewIssue, OpsApi, RunApi};

fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "not {what} in time");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn agent(id: &str) -> model::NodeDef {
    node(id, NodeKind::Agent(AgentNode { role: "You code.".into(), model: Some(scripted()), workdir: "code".into(), variants: 1, ..Default::default() }))
}

/// source → coder, nothing after.
fn single(repo: &std::path::Path, id: &str) -> model::GraphDef {
    let mut g = graph(repo, 1);
    g.id = PipelineId::new(id);
    g.nodes.truncate(2);
    g.edges.truncate(1);
    g
}

fn step_done(id: &str) -> Reply {
    Reply::tool(id, "step_done", json!({"summary": "Done."}))
}

fn project(fx: &Fx) {
    fx.engine.save_project(Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() }).unwrap();
}

fn issue(fx: &Fx, pipeline: &str) -> model::Issue {
    fx.engine
        .create_issue(NewIssue {
            project: ProjectId::new("har"),
            title: "Do it".into(),
            runner: Some(RunnableRef::pipeline(pipeline)),
            triggers: StartTriggers { on_assign: false, ..Default::default() },
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn a_closed_run_lets_go_of_its_driver_and_is_restored_on_demand() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let g = single(&repo, "single");
    deploy(&fx.engine, &g);
    fx.llm.push(step_done("f1"));
    let done = fx.engine.start_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &done.id, "done", |r| r.state == RunState::Done);
    eventually("let go", || !fx.engine.inner.runs.is_driven(&done.id));

    fx.llm.push(Reply::text("slow").with_delay(Duration::from_secs(20)));
    let cancelled = fx.engine.start_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &cancelled.id, "working", |r| r.nodes.iter().any(|n| n.session.is_some()));
    // The slow reply is in the cancelled step's hands, not left for the resumed one.
    eventually("the slow reply taken", || fx.llm.remaining() == 0);
    fx.engine.cancel_run(&cancelled.id).unwrap();
    wait_run(&fx.engine, &cancelled.id, "cancelled", |r| r.state == RunState::Cancelled);
    eventually("let go", || !fx.engine.inner.runs.is_driven(&cancelled.id));
    fx.llm.push(step_done("f2"));
    fx.engine.resume_run(&cancelled.id).expect("resumed from the store");
    wait_run(&fx.engine, &cancelled.id, "done", |r| r.state == RunState::Done);
}

#[test]
fn an_issue_is_done_when_its_branch_was_already_in_the_base() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = single(&repo, "merge-only");
    g.nodes.push(node("merge", NodeKind::Action(ActionNode::Merge { workdir: "code".into() })));
    g.edges.push(route("coder", PORT_OUT, "merge"));
    deploy(&fx.engine, &g);
    project(&fx);
    let issue = issue(&fx, "merge-only");
    // The coder changes nothing: the merge finds the branch in main already.
    fx.llm.push(step_done("f1"));
    fx.engine.start_issue(&issue.key).unwrap();
    let id = fx.engine.issue(&issue.key).unwrap().unwrap().run.unwrap();
    let run = wait_run(&fx.engine, &id, "done", |r| !r.state.is_open());
    assert_eq!(run.state, RunState::Done, "{run:#?}");
    assert!(run.nodes.iter().any(|n| n.node.as_str() == "merge" && n.summary.contains("already has")));
    assert_eq!(fx.engine.issue(&issue.key).unwrap().unwrap().status, IssueStatus::Done);
}

#[test]
fn a_snapshot_that_cannot_be_stored_stops_the_run_before_its_next_step() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = single(&repo, "merging");
    g.nodes.push(node("merge", NodeKind::Action(ActionNode::Merge { workdir: "code".into() })));
    g.edges.push(route("coder", PORT_OUT, "merge"));
    deploy(&fx.engine, &g);
    fx.llm.push(Reply::tool("w1", "write_file", json!({"path": "hello.txt", "content": "changed\n"})).with_delay(Duration::from_millis(300)));
    fx.llm.push(step_done("f1"));
    let run = fx.engine.start_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &run.id, "working", |r| r.nodes.iter().any(|n| n.session.is_some()));
    fx.engine.inner.fail_snapshots.store(true, Ordering::SeqCst);
    let failed = wait_run(&fx.engine, &run.id, "stopped", |r| !r.state.is_open());
    fx.engine.inner.fail_snapshots.store(false, Ordering::SeqCst);
    assert_eq!(failed.state, RunState::Failed, "{failed:#?}");
    assert!(failed.error.as_deref().unwrap_or_default().contains("could not be saved"), "{failed:#?}");
    assert_eq!(git(&repo, &["show", "main:hello.txt"]), "hello\n", "the merge never ran");
}

#[test]
fn an_issue_has_one_open_run_at_a_time() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    deploy(&fx.engine, &single(&repo, "first"));
    deploy(&fx.engine, &single(&repo, "second"));
    project(&fx);
    let one = issue(&fx, "first");
    fx.llm.push(Reply::text("slow").with_delay(Duration::from_secs(20)));
    fx.engine.start_issue(&one.key).unwrap();
    let error = fx.engine.start_issue_run(&one, &PipelineId::new("second"), "more", false).expect_err("one open run");
    assert!(format!("{error:#}").contains("still open"), "{error:#}");

    let two = issue(&fx, "first");
    fx.llm.push(Reply::text("slow").with_delay(Duration::from_secs(20)));
    fx.llm.push(Reply::text("slow").with_delay(Duration::from_secs(20)));
    let started: Vec<bool> = std::thread::scope(|scope| {
        let calls: Vec<_> = (0..2).map(|_| scope.spawn(|| fx.engine.start_issue(&two.key).is_ok())).collect();
        calls.into_iter().map(|c| c.join().unwrap()).collect()
    });
    assert_eq!(started.iter().filter(|ok| **ok).count(), 1, "{started:?}");
    let runs = fx.engine.runs_for_issue(&two.key).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].number, 1);
    for key in [&one.key, &two.key] {
        for run in fx.engine.runs_for_issue(key).unwrap() {
            let _ = fx.engine.cancel_run(&run.id);
        }
    }
}

#[test]
fn the_daily_limit_holds_wherever_agent_work_would_start() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let asking = single(&repo, "asking");
    deploy(&fx.engine, &asking);
    let mut reviewed = graph(&repo, 1);
    reviewed.id = PipelineId::new("reviewed");
    reviewed.nodes[3] = agent("after");
    reviewed.edges[2] = route("review", "approved", "after");
    deploy(&fx.engine, &reviewed);
    fx.llm.push(Reply::tool("a1", "ask_user", json!({"question": "Which?"})));
    let asked = fx.engine.start_run(&asking.id, input()).unwrap();
    wait_run(&fx.engine, &asked.id, "asking", |r| r.state == RunState::WaitingAnswer);
    fx.llm.push(Reply::tool("w1", "write_file", json!({"path": "hello.txt", "content": "x\n"})));
    fx.llm.push(step_done("f1"));
    let shipped = fx.engine.start_run(&reviewed.id, input()).unwrap();
    wait_run(&fx.engine, &shipped.id, "in review", |r| r.state == RunState::WaitingReview);

    let mut settings = fx.engine.settings().unwrap();
    settings.daily_limit_usd = Some(0.0);
    fx.engine.set_settings(settings).unwrap();
    let limit = |result: anyhow::Result<()>| assert!(format!("{:#}", result.expect_err("blocked")).contains("spend limit"));
    limit(fx.engine.answer_run(&asked.id, None, "This one"));
    limit(fx.engine.raise_run_budget(&asked.id, 1.0));
    let review = fx.engine.open_reviews().unwrap().into_iter().find(|r| r.run == shipped.id).unwrap();
    limit(fx.engine.decide_review(&review.id, ReviewDecision::Ship { variant: 1 }));
    fx.engine.decide_review(&review.id, ReviewDecision::Skip).expect("skipping starts nothing");
    let _ = fx.engine.cancel_run(&asked.id);
}

#[test]
fn test_runs_leave_nothing_behind_once_closed_or_discarded() {
    let (fx, _) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = single(&repo, "trial");
    fx.engine.save_pipeline(&catalog::Place::Library, g.clone()).unwrap();
    let tests_dir = fx.paths.worktrees_dir().join("_test");
    let test_branches = || git(&repo, &["branch", "--list", "harness-test/*"]).trim().to_string();

    fx.llm.push(Reply::text("slow").with_delay(Duration::from_secs(20)));
    let cancelled = fx.engine.test_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &cancelled.id, "working", |r| r.nodes.iter().any(|n| n.worktree.is_some()));
    assert!(!test_branches().is_empty());
    fx.engine.cancel_run(&cancelled.id).unwrap();
    eventually("cleaned up", || test_branches().is_empty());

    g.nodes.push(node(
        "fail",
        NodeKind::Action(ActionNode::Command { workdir: "code".into(), command: "exit 1".into(), tests: false, parse: String::new(), timeout_secs: 30 }),
    ));
    g.edges.push(route("coder", PORT_OUT, "fail"));
    fx.engine.save_pipeline(&catalog::Place::Library, g.clone()).unwrap();
    fx.llm.push(step_done("f1"));
    let failed = fx.engine.test_run(&g.id, input()).unwrap();
    wait_run(&fx.engine, &failed.id, "failed", |r| r.state == RunState::Failed);
    assert!(!test_branches().is_empty(), "a failed run can still be resumed");
    fx.engine.discard_run(&failed.id).unwrap();
    assert!(test_branches().is_empty());
    assert!(std::fs::read_dir(&tests_dir).map_or(true, |mut d| d.next().is_none()));

    deploy(&fx.engine, &single(&repo, "real"));
    fx.llm.push(step_done("f2"));
    let real = fx.engine.start_run(&PipelineId::new("real"), input()).unwrap();
    wait_run(&fx.engine, &real.id, "done", |r| r.state == RunState::Done);
    assert!(fx.engine.discard_run(&real.id).is_err(), "real runs keep their branches");
}

#[test]
fn configured_secrets_stay_out_of_the_run_log() {
    let mut options = super::options();
    options.secrets = vec!["sekrit-provider-value".into()];
    let fx = Fx::with(options);
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.id = PipelineId::new("leaky");
    g.nodes = vec![
        g.nodes[0].clone(),
        node(
            "leak",
            NodeKind::Action(ActionNode::Command {
                workdir: "code".into(),
                command: "echo key=sekrit-provider-value; exit 2".into(),
                tests: false,
                parse: String::new(),
                timeout_secs: 30,
            }),
        ),
    ];
    g.edges = vec![route("source", PORT_OUT, "leak")];
    deploy(&fx.engine, &g);
    let run = fx.engine.start_run(&g.id, input()).unwrap();
    let failed = wait_run(&fx.engine, &run.id, "failed", |r| r.state == RunState::Failed);
    let log = fx.engine.run_log(&run.id, 0, 500).unwrap();
    assert!(log.iter().any(|l| l.text.contains("[redacted]")), "{log:#?}");
    assert!(log.iter().all(|l| !l.text.contains("sekrit")));
    assert!(!format!("{failed:?}").contains("sekrit"));
    assert!(fx.engine.inbox(false).unwrap().iter().all(|i| !format!("{i:?}").contains("sekrit")));
}
