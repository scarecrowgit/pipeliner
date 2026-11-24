//! Mentioning an agent of an issue's run: HAR-8 as it happened (the designer finished, the
//! merge after it failed) and what a comment to the designer does then; mentions of library
//! agents answering in the thread; refusals.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use catalog::{Place, SelfEditModels, self_edit_pipeline};
use llm::Reply;
use model::{
    Actor, AgentDef, IssueEvent, IssueEventKind, IssueKey, NodeId, NodeInstance, NodeStatus, Origin, Project, ProjectId,
    ReviewDecision, Run, RunState, Runnable, RunnableKind, RunnableRef, SessionId, SpanKind, StartTriggers, Uses,
};
use serde_json::json;

use super::pipelines::{deploy, pipeline_fx, scripted, wait_run, with_remote};
use super::{Fx, git};
use crate::api::{CatalogApi, ChatApi, InboxApi, IssueApi, NewIssue, OpsApi, RunApi};

const PATIENCE: Duration = Duration::from_secs(30);

pub(super) struct Har8 {
    pub fx: Fx,
    pub mockups: PathBuf,
    pub issue: IssueKey,
    pub run: Run,
}

impl Har8 {
    pub fn designer(&self) -> &NodeInstance {
        self.run.nodes.iter().find(|n| n.node.as_str() == "designer").expect("the designer ran")
    }

    pub fn session(&self) -> SessionId {
        self.designer().session.clone().expect("the designer had a session")
    }
}

/// The self-editing pipeline on an issue: triage says a mockup is needed, the designer
/// changes it, the person ships it, and the merge fails because the main checkout of the
/// mockups has a change the merge would overwrite.
pub(super) fn har8() -> Har8 {
    let (fx, _gh) = pipeline_fx();
    let code = fx.repo("pipeliner", &[("src/lib.rs", "pub fn retry() {}\n")]);
    let mockups = fx.repo("mockups", &[("screens-v2/home.html", "<p>old home</p>\n")]);
    with_remote(&fx, &code);
    let models = SelfEditModels { triage: scripted(), designer: scripted(), orchestrator: scripted(), coder: scripted() };
    let mut graph = self_edit_pipeline(&code, &mockups, &models);
    for workdir in &mut graph.workdirs {
        workdir.setup.clear();
        workdir.env.clear();
    }
    deploy(&fx.engine, &graph);
    // As on HAR-8: the project names no repository; the pipeline names its own.
    fx.engine.save_project(Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() }).expect("project");
    fx.llm.push(Reply::tool("q1", "answer", json!({"answer": "yes", "reason": "It changes a screen."})));
    fx.llm.push(Reply::tool("d1", "write_file", json!({"path": "screens-v2/home.html", "content": "<p>new home</p>\n"})));
    fx.llm.push(Reply::tool("d2", "step_done", json!({"summary": "Moved the retry button."})));
    let issue = fx
        .engine
        .create_issue(NewIssue {
            project: ProjectId::new("har"),
            title: "ai pipeline editor".into(),
            runner: Some(RunnableRef::pipeline("mockup-gpui-build")),
            triggers: StartTriggers { on_assign: true, ..Default::default() },
            ..Default::default()
        })
        .expect("issue");
    let id = fx.engine.issue(&issue.key).unwrap().unwrap().run.expect("run");
    wait_run(&fx.engine, &id, "waiting for review", |r| r.state == RunState::WaitingReview || !r.state.is_open());
    std::fs::write(mockups.join("screens-v2/home.html"), "<p>someone's edit</p>\n").expect("dirty the checkout");
    let review = fx.engine.open_reviews().unwrap().into_iter().find(|r| r.run == id).expect("review");
    fx.engine.decide_review(&review.id, ReviewDecision::Ship { variant: 1 }).expect("ship");
    let run = wait_run(&fx.engine, &id, "failed", |r| !r.state.is_open());
    assert_eq!(run.state, RunState::Failed, "{run:#?}");
    assert!(run.error.as_deref().is_some_and(|e| e.starts_with("Merge mockup failed")), "{:?}", run.error);
    Har8 { fx, mockups, issue: issue.key, run }
}

/// Waits for the feed to hold an entry that `found` accepts.
pub(super) fn wait_feed(fx: &Fx, key: &IssueKey, what: &str, found: impl Fn(&IssueEvent) -> bool) -> IssueEvent {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let events = fx.engine.issue_events(key).expect("feed");
        if let Some(event) = events.iter().find(|e| found(e)) {
            return event.clone();
        }
        assert!(Instant::now() < deadline, "no {what} in the feed: {events:#?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn agent_comment(event: &IssueEvent) -> Option<(&str, &str, Option<&SessionId>)> {
    match (&event.actor, &event.kind) {
        (Actor::Runnable { name, session, .. }, IssueEventKind::Comment { text, .. }) => Some((name.as_str(), text.as_str(), session.as_ref())),
        _ => None,
    }
}

fn wakes_of(fx: &Fx, key: &IssueKey, target: &RunnableRef) -> usize {
    fx.engine.issue_events(key).unwrap().iter().filter(|e| matches!(&e.kind, IssueEventKind::Woken { target: t, .. } if t == target)).count()
}

#[test]
fn a_mentioned_designer_fixes_the_failed_merge_in_its_own_session_and_answers_in_the_thread() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    let agents = fx.engine.issue_agents(&h.issue).expect("agents");
    assert_eq!(agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Mockup designer"], "{agents:#?}");
    let worktree = h.designer().worktree.clone().expect("worktree");
    let messages_before = fx.engine.messages(&session, 1, u64::MAX).unwrap().len();

    // The designer cleans the main checkout with the shell, then answers; its answer names
    // agents, which must wake nobody.
    let clean = format!("git -C {} checkout -- screens-v2/home.html", h.mockups.display());
    fx.llm.push(Reply::tool("f1", "bash", json!({ "command": clean })));
    fx.llm.push(Reply::text("Fixed: the main checkout had an uncommitted edit; I threw it away. @Mockup designer"));
    fx.engine.comment(&h.issue, "@Mockup designer the merge failed, fix it", Vec::new()).expect("comment");

    let reply = wait_feed(fx, &h.issue, "answer", |e| agent_comment(e).is_some());
    let (name, text, from) = agent_comment(&reply).expect("an agent's comment");
    assert_eq!(name, "Mockup designer");
    assert!(text.starts_with("Fixed: the main checkout"), "{text}");
    assert_eq!(from, Some(&session), "the answer opens the node's own session");
    fx.settle(&session);
    assert_eq!(std::fs::read_to_string(h.mockups.join("screens-v2/home.html")).unwrap(), "<p>old home</p>\n");

    // Same session, same worktree, the comment with the run's failure as its next message.
    let stored = fx.engine.inner.session(&session).unwrap();
    assert_eq!(stored.workdir.as_ref().map(|w| w.path.clone()), Some(worktree.clone()));
    let messages = fx.engine.messages(&session, 1, u64::MAX).unwrap();
    assert!(messages.len() > messages_before, "the conversation went on");
    let told = format!("{:?}", fx.llm.requests().last().expect("a request"));
    assert!(told.contains("at the Merge mockup step"), "{told}");
    assert!(told.contains(&format!("is checked out in {}", h.mockups.display())), "the full error names the checkout: {told}");
    assert!(told.contains("design/") && told.contains(&worktree.display().to_string()), "{told}");
    assert!(told.contains("the merge failed, fix it"));

    // Nobody else woke, and the run did not move.
    let target = RunnableRef::session(&session);
    assert_eq!(wakes_of(fx, &h.issue, &target), 1);
    let events = fx.engine.issue_events(&h.issue).unwrap();
    assert!(events.iter().all(|e| !matches!(&e.kind, IssueEventKind::WakeFailed { .. })), "{events:#?}");
    let woken = events.iter().find_map(|e| match &e.kind {
        IssueEventKind::Woken { target: t, cause, run, .. } if t == &target => Some((cause.clone(), run.clone())),
        _ => None,
    });
    assert_eq!(woken, Some(("@mention · continues its work from run 1 in its worktree".to_string(), None)));
    let after = fx.engine.run(&h.run.id).unwrap().unwrap();
    assert_eq!(after.state, RunState::Failed);
    assert_eq!(after.nodes, h.run.nodes, "the mention leaves the run's steps as they were");

    // Resume runs the merge again, now that the checkout is clean, and the build after it.
    fx.llm.push(Reply::tool("b1", "bash", json!({"command": "printf 'moved\\n' > src/button.rs"})));
    fx.llm.push(Reply::tool("b2", "step_done", json!({"summary": "Built the new layout."})));
    fx.engine.resume_run(&h.run.id).expect("resume");
    let done = wait_run(&fx.engine, &h.run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    assert_eq!(git(&h.mockups, &["show", "main:screens-v2/home.html"]), "<p>new home</p>\n", "the mockup was merged");
}

#[test]
fn a_deleted_worktree_is_put_back_from_its_branch_and_refused_once_the_branch_is_gone() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    let worktree = h.designer().worktree.clone().expect("worktree");
    let branch = h.designer().branch.clone().expect("branch");
    std::fs::remove_dir_all(&worktree).expect("delete the worktree");

    // A file tool outside the worktree is refused at once, with a reason the agent can pass on:
    // nobody is asked, nothing hangs.
    let outside = h.mockups.join("screens-v2/outside.html");
    fx.llm.push(Reply::tool("w1", "write_file", json!({"path": outside.display().to_string(), "content": "x"})));
    fx.llm.push(Reply::text("The worktree is back; I could not write outside it."));
    fx.engine.comment(&h.issue, "@Mockup designer are you there?", Vec::new()).expect("comment");
    wait_feed(fx, &h.issue, "answer", |e| agent_comment(e).is_some());
    fx.settle(&session);
    assert!(!outside.exists(), "file tools stay inside the worktree");
    let refused = format!("{:?}", fx.llm.requests().last().expect("a request"));
    assert!(refused.contains("outside the workspace"), "{refused}");
    assert!(fx.engine.inner.asks.all().is_empty(), "nobody was asked");
    assert!(worktree.join("screens-v2/home.html").is_file(), "the worktree was checked out again");
    assert_eq!(git(&worktree, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), branch);

    std::fs::remove_dir_all(&worktree).expect("delete the worktree");
    git(&h.mockups, &["worktree", "prune"]);
    git(&h.mockups, &["branch", "-D", &branch]);
    fx.engine.comment(&h.issue, "@Mockup designer again", Vec::new()).expect("comment");
    let failed = wait_feed(fx, &h.issue, "refusal", |e| matches!(e.kind, IssueEventKind::WakeFailed { .. }));
    let IssueEventKind::WakeFailed { reason, name, .. } = &failed.kind else { unreachable!() };
    assert_eq!(name, "Mockup designer");
    assert!(reason.contains("no longer exists"), "{reason}");
}

fn bare_run(key: &IssueKey, status: NodeStatus) -> Run {
    Run {
        id: model::RunId::new("run-busy"),
        pipeline: "p".into(),
        version: 1,
        issue: Some(key.clone()),
        number: 3,
        title: "t".into(),
        input: String::new(),
        state: RunState::Running,
        test: false,
        started: 1,
        finished: None,
        cost_usd: 0.0,
        usage: Default::default(),
        error: None,
        pr_url: None,
        nodes: vec![NodeInstance {
            node: NodeId::new("mockup"),
            name: "Mockup".into(),
            variant: 0,
            attempt: 1,
            status,
            kind: SpanKind::Agent,
            port: None,
            session: Some(SessionId::new("s-busy")),
            worktree: None,
            branch: None,
            started: Some(1),
            finished: None,
            cost_usd: 0.0,
            summary: String::new(),
            spawned_by: None,
            question: None,
        }],
    }
}

#[test]
fn an_agent_whose_step_is_running_is_not_woken() {
    let fx = Fx::new();
    fx.engine.save_project(Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() }).expect("project");
    let issue = fx.engine.create_issue(NewIssue { project: ProjectId::new("har"), title: "Busy".into(), ..Default::default() }).expect("issue");
    fx.engine.inner.store.upsert_run(&bare_run(&issue.key, NodeStatus::Running)).expect("run");
    let agents = fx.engine.issue_agents(&issue.key).expect("agents");
    assert!(agents.len() == 1 && agents[0].busy, "{agents:#?}");
    fx.engine.comment(&issue.key, "@Mockup look at this", Vec::new()).expect("comment");
    let events = fx.engine.issue_events(&issue.key).unwrap();
    let comment = events.iter().find_map(|e| match &e.kind {
        IssueEventKind::Comment { mentions, .. } => Some(mentions.clone()),
        _ => None,
    });
    assert_eq!(comment, Some(vec![RunnableRef::session(&SessionId::new("s-busy"))]));
    assert!(
        events.iter().any(|e| matches!(&e.kind, IssueEventKind::WakeFailed { name, reason, .. } if name == "Mockup" && reason.contains("running right now"))),
        "{events:#?}"
    );
}

fn fixer(model: model::ModelRef) -> Runnable {
    Runnable {
        reference: RunnableRef::agent("fixer"),
        kind: RunnableKind::Agent,
        name: "Fixer".into(),
        description: "Fixes bugs".into(),
        origin: Origin::Library,
        uses: Uses::default(),
        paused: false,
        archived: false,
        agent: Some(AgentDef { role: "You fix bugs.".into(), model, ..Default::default() }),
        path: None,
    }
}

#[test]
fn a_mentioned_library_agent_works_where_the_pipeline_does_and_answers_in_the_thread() {
    let (fx, _gh) = pipeline_fx();
    let code = fx.repo("pipeliner", &[("src/lib.rs", "\n")]);
    let mockups = fx.repo("mockups", &[("screens-v2/home.html", "\n")]);
    let models = SelfEditModels { triage: scripted(), designer: scripted(), orchestrator: scripted(), coder: scripted() };
    deploy(&fx.engine, &self_edit_pipeline(&code, &mockups, &models));
    fx.engine.save_runnable(&Place::Library, fixer(scripted())).expect("fixer");
    fx.engine.save_project(Project { id: ProjectId::new("har"), name: "Harness".into(), key_prefix: "HAR".into(), repos: Vec::new() }).expect("project");
    let issue = fx
        .engine
        .create_issue(NewIssue {
            project: ProjectId::new("har"),
            title: "Retry".into(),
            runner: Some(RunnableRef::pipeline("mockup-gpui-build")),
            triggers: StartTriggers { on_assign: false, ..Default::default() },
            ..Default::default()
        })
        .expect("issue");

    fx.llm.push(Reply::text("Looked: the retry loops on a 409."));
    fx.engine.comment(&issue.key, "@Fixer have a look", Vec::new()).expect("comment");
    let reply = wait_feed(&fx, &issue.key, "answer", |e| agent_comment(e).is_some());
    let (name, text, session) = agent_comment(&reply).expect("answer");
    assert_eq!((name, text), ("Fixer", "Looked: the retry loops on a 409."));
    let session = fx.engine.inner.session(session.expect("its chat")).unwrap();
    let source = session.workdir.as_ref().and_then(|w| w.source.clone());
    assert_eq!(source.as_deref(), Some(mockups.as_path()), "a project without repos works in the pipeline's: {session:#?}");
    let _: &Path = &code;
}
