//! Mentions of a run's agent under pressure: two at once, during a Resume, with a step waiting
//! for the session, turns that fail or ask, a restart with an answer owed, a crashing step.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use llm::{LlmError, Reply};
use model::{
    Actor, IssueEventKind, PipelineId, Project, ProjectId, ReviewDecision, RunState, RunnableRef, StartFrom, StartTriggers,
};
use serde_json::json;

use super::pipelines::{deploy, pipeline_fx, wait_run};
use super::pipelines_more::graph;
use super::wake::{agent_comment, har8, wait_feed};
use super::{Fx, git};
use crate::api::{ChatApi, InboxApi, IssueApi, NewIssue, OpsApi, RunApi};

fn agent_comments(fx: &Fx, key: &model::IssueKey) -> Vec<String> {
    fx.engine.issue_events(key).unwrap().iter().filter_map(|e| agent_comment(e).map(|(_, text, _)| text.to_string())).collect()
}

/// Waits until the scripted model has handed out every reply pushed so far.
fn popped(fx: &Fx) {
    popped_to(fx, 0);
}

/// Waits until the scripted model holds at most `left` replies.
fn popped_to(fx: &Fx, left: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while fx.llm.remaining() > left {
        assert!(Instant::now() < deadline, "the model was not called");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn two_mentions_taken_by_one_turn_get_one_answer_and_a_later_one_its_own() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    fx.llm.push(Reply::text("Looking.").with_delay(Duration::from_millis(400)));
    fx.llm.push(Reply::text("Both answered: the merge is clean now."));
    // The same words twice: two messages, both reach the session.
    fx.engine.comment(&h.issue, "@Mockup designer status?", Vec::new()).expect("first");
    popped_to(fx, 1);
    fx.engine.comment(&h.issue, "@Mockup designer status?", Vec::new()).expect("second");
    wait_feed(fx, &h.issue, "answer", |e| agent_comment(e).is_some());
    fx.settle(&session);
    assert_eq!(agent_comments(fx, &h.issue), ["Both answered: the merge is clean now."], "one turn, one answer");
    let asked = fx.engine.messages(&session, 1, u64::MAX).unwrap();
    let said = format!("{asked:?}");
    assert_eq!(said.matches("@Mockup designer status?").count(), 2, "both comments reached the session");

    fx.llm.push(Reply::text("And this one too."));
    fx.engine.comment(&h.issue, "@Mockup designer status?", Vec::new()).expect("third");
    wait_feed(fx, &h.issue, "second answer", |e| agent_comment(e).is_some_and(|(_, t, _)| t == "And this one too."));
    fx.settle(&session);
    assert_eq!(agent_comments(fx, &h.issue).len(), 2);
}

#[test]
fn a_mention_during_a_resume_answers_and_the_run_still_finishes() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    std::fs::write(h.mockups.join("screens-v2/home.html"), "<p>old home</p>\n").expect("clean the checkout");
    git(&h.mockups, &["status", "--short"]);
    fx.llm.push(Reply::text("I can see the run going again.").with_delay(Duration::from_millis(500)));
    fx.engine.comment(&h.issue, "@Mockup designer the run goes on", Vec::new()).expect("comment");
    popped(fx);
    fx.llm.push(Reply::tool("b1", "bash", json!({"command": "printf 'moved\\n' > src/button.rs"})));
    fx.llm.push(Reply::tool("b2", "step_done", json!({"summary": "Built."})));
    fx.engine.resume_run(&h.run.id).expect("resume");
    let done = wait_run(&fx.engine, &h.run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
    wait_feed(fx, &h.issue, "answer", |e| agent_comment(e).is_some());
    fx.settle(&session);
    assert_eq!(agent_comments(fx, &h.issue), ["I can see the run going again."]);
    let designer = done.nodes.iter().find(|n| n.node.as_str() == "designer").unwrap();
    assert_eq!(designer, h.designer(), "the designer's step is as it was");
}

#[test]
fn a_run_cancelled_while_its_step_waits_for_the_session_stops_at_once() {
    let (fx, _gh) = pipeline_fx();
    // Up to the review, as on HAR-8, then a slow answer holds the designer's session.
    let h = har8_until_review(fx);
    let (fx, issue, run) = (&h.0, &h.1, &h.2);
    let session = fx.engine.run(run).unwrap().unwrap().nodes.iter().find(|n| n.node.as_str() == "designer").unwrap().session.clone().unwrap();
    fx.llm.push(Reply::text("Thinking it over.").with_delay(Duration::from_secs(3)));
    fx.engine.comment(issue, "@Mockup designer wait", Vec::new()).expect("comment");
    popped(fx);
    let review = fx.engine.open_reviews().unwrap().into_iter().find(|r| &r.run == run).expect("review");
    let back = ReviewDecision::SendBack { note: "Bigger.".into(), start: StartFrom::Variant { index: 1 }, variants: 1 };
    fx.engine.decide_review(&review.id, back).expect("send back");
    wait_run(&fx.engine, run, "running", |r| r.state == RunState::Running);
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    fx.engine.cancel_run(run).expect("cancel");
    wait_run(&fx.engine, run, "cancelled", |r| !r.state.is_open());
    assert!(started.elapsed() < Duration::from_secs(2), "the waiting step stopped at once: {:?}", started.elapsed());
    assert!(fx.engine.inner.live.is_running(&session), "the answer is still being written");
    wait_feed(fx, issue, "answer", |e| agent_comment(e).is_some());
}

/// HAR-8's pipeline run up to its review, with the designer's work done.
fn har8_until_review(fx: Fx) -> (Fx, model::IssueKey, model::RunId) {
    let code = fx.repo("pipeliner", &[("src/lib.rs", "\n")]);
    let mockups = fx.repo("mockups", &[("screens-v2/home.html", "<p>old home</p>\n")]);
    let models = catalog::SelfEditModels {
        triage: super::pipelines::scripted(),
        designer: super::pipelines::scripted(),
        orchestrator: super::pipelines::scripted(),
        coder: super::pipelines::scripted(),
    };
    let mut g = catalog::self_edit_pipeline(&code, &mockups, &models);
    for w in &mut g.workdirs {
        w.setup.clear();
        w.env.clear();
    }
    deploy(&fx.engine, &g);
    fx.engine.save_project(Project { id: ProjectId::new("har"), name: "H".into(), key_prefix: "HAR".into(), repos: Vec::new() }).unwrap();
    fx.llm.push(Reply::tool("q1", "answer", json!({"answer": "yes", "reason": "A screen."})));
    fx.llm.push(Reply::tool("d1", "write_file", json!({"path": "screens-v2/home.html", "content": "<p>new</p>\n"})));
    fx.llm.push(Reply::tool("d2", "step_done", json!({"summary": "Moved it."})));
    let issue = fx
        .engine
        .create_issue(NewIssue {
            project: ProjectId::new("har"),
            title: "t".into(),
            runner: Some(RunnableRef::pipeline("mockup-gpui-build")),
            triggers: StartTriggers { on_assign: true, ..Default::default() },
            ..Default::default()
        })
        .unwrap();
    let run = fx.engine.issue(&issue.key).unwrap().unwrap().run.unwrap();
    wait_run(&fx.engine, &run, "waiting for review", |r| r.state == RunState::WaitingReview || !r.state.is_open());
    (fx, issue.key, run)
}

#[test]
fn a_turn_that_fails_or_asks_says_so_in_the_thread() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    fx.llm.push(Reply::error(LlmError::Auth("the key expired".into())));
    fx.engine.comment(&h.issue, "@Mockup designer try", Vec::new()).expect("comment");
    let failed = wait_feed(fx, &h.issue, "failure", |e| agent_comment(e).is_some());
    let (_, text, _) = agent_comment(&failed).unwrap();
    assert!(text.starts_with("Could not answer:") && text.contains("the key expired"), "{text}");
    fx.settle(&session);

    fx.llm.push(Reply::tool("a1", "ask_user", json!({"question": "Abort the merge or commit it?"})));
    fx.engine.comment(&h.issue, "@Mockup designer go on", Vec::new()).expect("comment");
    let asked = wait_feed(fx, &h.issue, "question", |e| agent_comment(e).is_some_and(|(_, t, _)| t.starts_with("Abort")));
    let (_, text, _) = agent_comment(&asked).unwrap();
    assert_eq!(text, "Abort the merge or commit it?\n\nMention Mockup designer in a comment to answer.");
    fx.settle(&session);
    assert!(fx.engine.inner.asks.all().is_empty(), "nobody waits on a dialog");
}

#[test]
fn an_answer_owed_when_the_app_stops_is_noted_after_the_restart() {
    let h = har8();
    let fx = &h.fx;
    fx.llm.push(Reply::text("Never finished.").with_delay(Duration::from_secs(30)));
    fx.engine.comment(&h.issue, "@Mockup designer anything?", Vec::new()).expect("comment");
    popped(fx);
    // As after a crash: another engine opens the same data while the turn never ended.
    let (engine, _llm) = fx.reopen();
    let events = engine.issue_events(&h.issue).unwrap();
    let note = events.iter().find_map(|e| match (&e.actor, &e.kind) {
        (Actor::Runnable { name, .. }, IssueEventKind::Note { text }) => Some((name.clone(), text.clone())),
        _ => None,
    });
    let (name, text) = note.expect("a line in the thread");
    assert_eq!(name, "Mockup designer");
    assert!(text.starts_with("Mockup designer was interrupted before answering"), "{text}");
    assert!(engine.inner.store.markers_with_prefix("owed-reply:").unwrap().is_empty(), "nothing stays owed");
    fx.engine.inner.shutdown();
}

#[test]
fn a_step_that_crashes_lets_go_of_its_session() {
    let (fx, _gh) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.id = PipelineId::new("single");
    g.nodes.truncate(2);
    g.edges.truncate(1);
    deploy(&fx.engine, &g);
    fx.engine.inner.panic_in_steps.store(true, Ordering::SeqCst);
    let run = fx.engine.start_run(&g.id, super::pipelines_more::input()).unwrap();
    let crashed = wait_run(&fx.engine, &run.id, "stopped", |r| !r.state.is_open() || r.state == RunState::Interrupted);
    let session = crashed.nodes.iter().find_map(|n| n.session.clone()).expect("the step had opened its session");
    assert!(!fx.engine.inner.live.is_running(&session), "the claim went with the crash");
    fx.engine.inner.panic_in_steps.store(false, Ordering::SeqCst);
    fx.llm.push(Reply::tool("f1", "step_done", json!({"summary": "Done."})));
    fx.engine.resume_run(&run.id).expect("resume");
    let done = wait_run(&fx.engine, &run.id, "done", |r| !r.state.is_open());
    assert_eq!(done.state, RunState::Done, "{done:#?}");
}

#[test]
fn a_folder_left_where_the_worktree_was_is_refused_not_used() {
    let h = har8();
    let fx = &h.fx;
    let worktree = h.designer().worktree.clone().expect("worktree");
    std::fs::remove_dir_all(&worktree).expect("delete the worktree");
    std::fs::create_dir_all(&worktree).expect("a plain folder");
    std::fs::write(worktree.join("notes.txt"), "mine\n").expect("write");
    fx.engine.comment(&h.issue, "@Mockup designer here?", Vec::new()).expect("comment");
    let failed = wait_feed(fx, &h.issue, "refusal", |e| matches!(e.kind, IssueEventKind::WakeFailed { .. }));
    let IssueEventKind::WakeFailed { reason, .. } = &failed.kind else { unreachable!() };
    assert!(reason.contains("is no longer the worktree of `design/"), "{reason}");
    assert_eq!(std::fs::read_to_string(worktree.join("notes.txt")).unwrap(), "mine\n", "the folder is left alone");
}

#[test]
fn a_mention_turn_spends_on_its_session_not_on_the_run() {
    let h = har8();
    let fx = &h.fx;
    let session = h.session();
    let store = &fx.engine.inner.store;
    let (run_calls, session_calls) = (store.spend_for_run(&h.run.id).unwrap().calls, store.spend_for_session(&session).unwrap().calls);
    fx.llm.push(Reply::text("Cheap answer.").with_usage(model::Usage { input: 100, output: 10, ..Default::default() }));
    fx.engine.comment(&h.issue, "@Mockup designer cost?", Vec::new()).expect("comment");
    wait_feed(fx, &h.issue, "answer", |e| agent_comment(e).is_some());
    fx.settle(&session);
    assert_eq!(store.spend_for_run(&h.run.id).unwrap().calls, run_calls, "the run's spend is the run's steps only");
    assert_eq!(store.spend_for_session(&session).unwrap().calls, session_calls + 1);
    assert_eq!(store.spend_for_session_outside_runs(&session).unwrap().calls, 1);
}

#[test]
fn a_persons_message_into_a_step_session_without_its_graph_still_works_as_a_chat() {
    let fx = Fx::new();
    let folder = fx.repo("app", &[("a.txt", "a\n")]);
    let chat = fx.chat_in(&folder);
    let origin = model::SessionOrigin::Run { run: model::RunId::new("gone"), node: model::NodeId::new("coder"), variant: 0 };
    fx.engine.inner.update_session(&chat.id, |s| s.origin = origin).unwrap();
    fx.llm.push(Reply::text("Still here."));
    fx.say(&chat.id, "hello");
    let after = fx.settle(&chat.id);
    assert_ne!(after.state, model::SessionState::Failed, "{:#?}", fx.agent_events(&chat.id));
    let said = format!("{:?}", fx.engine.messages(&chat.id, 1, u64::MAX).unwrap());
    assert!(said.contains("Still here."), "{said}");
}

#[test]
fn a_message_sent_while_a_step_holds_the_session_is_not_left_behind() {
    let (fx, _gh) = pipeline_fx();
    let repo = fx.repo("app", &[("hello.txt", "hello\n")]);
    let mut g = graph(&repo, 1);
    g.id = PipelineId::new("single");
    g.nodes.truncate(2);
    g.edges.truncate(1);
    deploy(&fx.engine, &g);
    fx.llm.push(Reply::tool("f1", "step_done", json!({"summary": "Done."})).with_delay(Duration::from_millis(400)));
    let run = fx.engine.start_run(&g.id, super::pipelines_more::input()).unwrap();
    let working = wait_run(&fx.engine, &run.id, "working", |r| r.nodes.iter().any(|n| n.session.is_some()));
    let session = working.nodes.iter().find_map(|n| n.session.clone()).unwrap();
    popped(&fx);
    fx.llm.push(Reply::text("Seen after the step."));
    fx.say(&session, "one more thing");
    wait_run(&fx.engine, &run.id, "done", |r| !r.state.is_open());
    let deadline = Instant::now() + Duration::from_secs(10);
    while fx.engine.inner.live.is_running(&session) || !fx.engine.inner.live.queued(&session).is_empty() {
        assert!(Instant::now() < deadline, "the message stayed queued");
        std::thread::sleep(Duration::from_millis(5));
    }
    let said = format!("{:?}", fx.engine.messages(&session, 1, u64::MAX).unwrap());
    assert!(said.contains("one more thing"), "{said}");
}
