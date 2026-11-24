//! Pipelines end to end: the self-editing graph from an issue to a pull request, on both of
//! its branches, against the scripted model, temp repos, a fake `gh` and a fake renderer.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use base::cancel::CancelToken;
use catalog::{Place, SelfEditModels, self_edit_pipeline};
use llm::Reply;
use model::{
    GraphDef, InboxKind, IssueStatus, ModelRef, Project, ProjectId, ReviewDecision, ReviewState, Run, RunId, RunState,
    RunnableRef, StartTriggers,
};
use pipeline::runners::{Picture, Renderer};
use serde_json::json;
use vcs::pr::{Gh, GhOutput};

use super::{Fx, git, options};
use crate::Engine;
use crate::api::{CatalogApi, InboxApi, IssueApi, NewIssue, OpsApi, RunApi};

pub(crate) const PR_URL: &str = "https://github.com/acme/pipeliner/pull/42";
const PATIENCE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct FakeGh(pub Mutex<Vec<Vec<String>>>);

impl Gh for FakeGh {
    fn run(&self, _: Option<&Path>, args: &[String], _: Option<&str>, _: &CancelToken) -> Result<GhOutput> {
        self.0.lock().expect("gh").push(args.to_vec());
        let stdout = if args.get(1).map(String::as_str) == Some("list") { "[]".into() } else { format!("{PR_URL}\n") };
        Ok(GhOutput { success: true, stdout, stderr: String::new() })
    }
}

/// Renders a file as its own bytes.
pub(crate) struct FakeRenderer;

impl Renderer for FakeRenderer {
    fn render(&self, html: &Path, _: &CancelToken) -> Result<Picture> {
        Ok(Picture { png: std::fs::read(html)?, width: 640, height: 480 })
    }
}

/// An engine with the fake `gh` and renderer.
pub(crate) fn pipeline_fx() -> (Fx, Arc<FakeGh>) {
    let gh = Arc::new(FakeGh::default());
    let mut options = options();
    options.gh = Some(gh.clone());
    options.renderer = Some(Arc::new(FakeRenderer));
    (Fx::with(options), gh)
}

pub(crate) fn scripted() -> ModelRef {
    ModelRef::parse("scripted/scripted-model")
}

/// Saves the graph as the draft and deploys it.
pub(crate) fn deploy(engine: &Engine, graph: &GraphDef) {
    engine.save_pipeline(&Place::Library, graph.clone()).expect("save");
    engine.deploy_pipeline(&graph.id, "test").expect("deploy");
}

/// Waits until the stored run satisfies `done`.
pub(crate) fn wait_run(engine: &Engine, id: &RunId, what: &str, done: impl Fn(&Run) -> bool) -> Run {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let run = engine.run(id).expect("run").expect("stored run");
        if done(&run) {
            return run;
        }
        if Instant::now() > deadline {
            let log: Vec<String> = engine.run_log(id, 0, 500).unwrap_or_default().into_iter().map(|l| l.text).collect();
            panic!("the run is not {what}: {run:#?}\nlog: {log:#?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A bare remote the repo pushes to.
pub(crate) fn with_remote(fx: &Fx, repo: &Path) -> std::path::PathBuf {
    let remote = fx.root().join(format!("{}-remote.git", repo.file_name().unwrap().to_string_lossy()));
    git(&fx.root(), &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(repo, &["push", "-q", "origin", "main"]);
    remote
}

struct SelfEdit {
    fx: Fx,
    gh: Arc<FakeGh>,
    code: std::path::PathBuf,
    mockups: std::path::PathBuf,
    remote: std::path::PathBuf,
}

fn self_edit() -> SelfEdit {
    let (fx, gh) = pipeline_fx();
    let code = fx.repo("pipeliner", &[("src/lib.rs", "pub fn retry() {}\n")]);
    let mockups = fx.repo("mockups", &[("screens-v2/home.html", "<p>old home</p>\n")]);
    let remote = with_remote(&fx, &code);
    let models = SelfEditModels { triage: scripted(), designer: scripted(), orchestrator: scripted(), coder: scripted() };
    let mut graph = self_edit_pipeline(&code, &mockups, &models);
    for workdir in &mut graph.workdirs {
        workdir.setup.clear();
        workdir.env.clear();
    }
    deploy(&fx.engine, &graph);
    fx.engine
        .save_project(Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: vec![code.clone()] })
        .expect("project");
    SelfEdit { fx, gh, code, mockups, remote }
}

fn new_issue(engine: &Engine, title: &str) -> model::Issue {
    engine
        .create_issue(NewIssue {
            project: ProjectId::new("har"),
            title: title.into(),
            body: "The retry runs twice.".into(),
            runner: Some(RunnableRef::pipeline("mockup-gpui-build")),
            triggers: StartTriggers { on_assign: true, ..Default::default() },
            ..Default::default()
        })
        .expect("issue")
}

fn answer(id: &str, yes: bool) -> Reply {
    Reply::tool(id, "answer", json!({"answer": if yes { "yes" } else { "no" }, "reason": "Decided."}))
}

fn step_done(id: &str, summary: &str) -> Reply {
    Reply::tool(id, "step_done", json!({ "summary": summary }))
}

#[test]
fn an_issue_without_a_mockup_goes_from_triage_to_an_opened_pull_request() {
    let s = self_edit();
    s.fx.llm.push(answer("q1", false));
    s.fx.llm.push(Reply::tool("b1", "bash", json!({"command": "printf 'pub fn once() {}\\n' > src/once.rs"})));
    s.fx.llm.push(step_done("b2", "Made the retry run once. Built and tested."));
    let issue = new_issue(&s.fx.engine, "Retry runs twice");

    let issue = s.fx.engine.issue(&issue.key).expect("issue").expect("stored");
    let id = issue.run.clone().expect("the trigger started a run");
    let run = wait_run(&s.fx.engine, &id, "done", |r| !r.state.is_open());
    assert_eq!(run.state, RunState::Done, "{run:#?}");
    assert_eq!(run.pr_url.as_deref(), Some(PR_URL));
    assert_eq!(run.version, 1);
    let triage = run.nodes.iter().find(|n| n.node.as_str() == "triage").expect("triage");
    assert_eq!(triage.port.as_deref(), Some("else"));
    assert!(run.nodes.iter().all(|n| n.node.as_str() != "designer" || n.status == model::NodeStatus::Skipped));

    let calls = s.gh.0.lock().unwrap().clone();
    let create = calls.iter().find(|c| c.get(1).map(String::as_str) == Some("create")).expect("gh pr create");
    assert!(create.contains(&"--draft".to_string()) && create.iter().any(|a| a.starts_with("HAR-1")), "{create:?}");
    assert!(git(&s.remote, &["branch", "--list"]).contains("har-1"), "the branch was pushed");
    assert!(!s.code.join("src/once.rs").exists(), "the checkout is untouched");

    let issue = s.fx.engine.issue(&issue.key).unwrap().unwrap();
    assert_eq!(issue.status, IssueStatus::InReview);
    assert_eq!(issue.pr_url.as_deref(), Some(PR_URL));
    assert_eq!(issue.run_count, 1);
    let inbox = s.fx.engine.inbox(false).unwrap();
    assert!(inbox.iter().any(|i| i.kind == InboxKind::Report && i.links.iter().any(|l| l.url == PR_URL)), "{inbox:#?}");
    let events = s.fx.engine.issue_events(&issue.key).unwrap();
    assert!(events.iter().any(|e| matches!(&e.kind, model::IssueEventKind::Woken { number: 1, .. })), "{events:#?}");
    let log = s.fx.engine.run_log(&id, 0, 500).unwrap();
    assert!(log.iter().any(|l| l.level == model::LogLevel::Tool), "tool lines reach the log");
    assert!(s.fx.engine.home_stats(Default::default()).is_ok());
}

#[test]
fn a_mockup_change_is_rendered_reviewed_shipped_merged_and_built() {
    let s = self_edit();
    s.fx.llm.push(answer("q1", true));
    s.fx.llm.push(Reply::tool("d1", "write_file", json!({"path": "screens-v2/home.html", "content": "<p>new home</p>\n"})));
    s.fx.llm.push(step_done("d2", "Moved the retry button. Rebuilt the screens."));
    let issue = new_issue(&s.fx.engine, "Move the retry button");
    let id = s.fx.engine.issue(&issue.key).unwrap().unwrap().run.expect("run");

    let run = wait_run(&s.fx.engine, &id, "waiting for review", |r| r.state == RunState::WaitingReview || !r.state.is_open());
    assert_eq!(run.state, RunState::WaitingReview, "{run:#?}");
    let reviews = s.fx.engine.open_reviews().unwrap();
    let review = reviews.iter().find(|r| r.run == id).expect("an open review");
    assert_eq!(review.renders.len(), 1);
    assert_eq!(review.renders[0].path, "screens-v2/home.html");
    assert!(review.renders[0].before.is_some());
    let diff = s.fx.engine.variant_diff(&review.id, 1, "screens-v2/home.html").expect("diff");
    assert!(!diff.hunks.is_empty());
    let inbox = s.fx.engine.inbox(false).unwrap();
    assert!(inbox.iter().any(|i| i.kind == InboxKind::Review { review: review.id.clone() }));
    assert_eq!(s.fx.engine.issue(&issue.key).unwrap().unwrap().status, IssueStatus::InReview);

    s.fx.llm.push(Reply::tool("b1", "bash", json!({"command": "printf 'moved\\n' > src/button.rs"})));
    s.fx.llm.push(step_done("b2", "Built the new layout."));
    s.fx.engine.decide_review(&review.id, ReviewDecision::Ship { variant: 1 }).expect("ship");
    let run = wait_run(&s.fx.engine, &id, "done", |r| !r.state.is_open());
    assert_eq!(run.state, RunState::Done, "{run:#?}");
    assert_eq!(git(&s.mockups, &["show", "main:screens-v2/home.html"]), "<p>new home</p>\n", "the mockup was merged");
    assert_eq!(run.pr_url.as_deref(), Some(PR_URL));
    assert_eq!(s.fx.engine.review(&review.id).unwrap().unwrap().state, ReviewState::Shipped);
    let inbox = s.fx.engine.inbox(false).unwrap();
    assert!(!inbox.iter().any(|i| i.kind == InboxKind::Review { review: review.id.clone() }), "the review left the Inbox");
    // The build was told about the approved mockups.
    let requests = s.fx.llm.requests();
    assert!(requests.iter().any(|r| format!("{r:?}").contains("Context: Mockups")));
}
