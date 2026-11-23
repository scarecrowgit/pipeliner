use model::{IssueStatus, NodeId, NodeStatus, RunState, RunnableRef, SpanKind};

use super::*;
use crate::testing::{issue, node, run};

fn with_nodes(state: RunState, nodes: Vec<model::NodeInstance>) -> Run {
    let mut r = run("r1", state);
    r.nodes = nodes;
    r
}

#[test]
fn no_run_is_idle_or_starting_soon() {
    let mut i = issue(IssueStatus::Backlog);
    assert_eq!(card_state(&i, None), CardState::Idle);
    i.status = IssueStatus::Todo;
    assert_eq!(card_state(&i, None), CardState::Idle);
    i.runner = Some(RunnableRef::agent("codex"));
    assert_eq!(card_state(&i, None), CardState::Starting { text: "Starts soon".into() });
    i.triggers.on_assign = false;
    assert_eq!(card_state(&i, None), CardState::Idle);
    i.triggers.on_todo = true;
    assert_eq!(card_state(&i, None), CardState::Starting { text: "Starts soon".into() });
    i.status = IssueStatus::InProgress;
    assert_eq!(card_state(&i, None), CardState::Idle);
}

#[test]
fn queued_is_starting() {
    let i = issue(IssueStatus::InProgress);
    assert_eq!(card_state(&i, Some(&run("r", RunState::Queued))), CardState::Starting { text: "Queued".into() });
}

#[test]
fn running_names_the_agent_and_shows_progress() {
    let i = issue(IssueStatus::InProgress);
    let r = with_nodes(
        RunState::Running,
        vec![node("Router", NodeStatus::Done), node("Reproducer", NodeStatus::Running), node("Fixer", NodeStatus::Pending), node("Review", NodeStatus::Pending)],
    );
    assert_eq!(card_state(&i, Some(&r)), CardState::Running { text: "Reproducer running".into(), spawned: None, percent: Some(25) });
}

#[test]
fn running_shows_the_spawned_agent() {
    let i = issue(IssueStatus::InProgress);
    let mut log = node("Log digger", NodeStatus::Running);
    log.kind = SpanKind::Spawned;
    log.spawned_by = Some(NodeId::new("reproducer"));
    let r = with_nodes(RunState::Running, vec![node("Reproducer", NodeStatus::Running), log]);
    assert_eq!(
        card_state(&i, Some(&r)),
        CardState::Running { text: "Reproducer running".into(), spawned: Some("Log digger".into()), percent: Some(0) }
    );
}

#[test]
fn running_with_several_agents_or_none() {
    let i = issue(IssueStatus::InProgress);
    let mut variants: Vec<_> = (1..=3).map(|v| { let mut n = node("Fixer", NodeStatus::Running); n.variant = v; n }).collect();
    let r = with_nodes(RunState::Running, variants.clone());
    assert!(matches!(card_state(&i, Some(&r)), CardState::Running { text, .. } if text == "Fixer ×3 running"));
    variants.push(node("Analyst", NodeStatus::Running));
    let r = with_nodes(RunState::Running, variants);
    assert!(matches!(card_state(&i, Some(&r)), CardState::Running { text, .. } if text == "4 agents running"));
    let r = run("r", RunState::Running);
    assert_eq!(card_state(&i, Some(&r)), CardState::Running { text: "Running".into(), spawned: None, percent: None });
}

#[test]
fn waiting_for_review_counts_variants() {
    let i = issue(IssueStatus::InReview);
    let nodes = (1..=3).map(|v| { let mut n = node("Fixer", NodeStatus::Waiting); n.variant = v; n }).collect();
    let r = with_nodes(RunState::WaitingReview, nodes);
    assert_eq!(card_state(&i, Some(&r)), CardState::Waiting { text: "3 variants to review".into() });
    let mut one = node("Fixer", NodeStatus::Waiting);
    one.variant = 1;
    let r = with_nodes(RunState::WaitingReview, vec![one]);
    assert_eq!(card_state(&i, Some(&r)), CardState::Waiting { text: "1 variant to review".into() });
    let r = with_nodes(RunState::WaitingReview, vec![node("Spec", NodeStatus::Waiting)]);
    assert_eq!(card_state(&i, Some(&r)), CardState::Waiting { text: "Ready to review".into() });
}

#[test]
fn waiting_for_an_answer_shows_the_question() {
    let i = issue(IssueStatus::InProgress);
    let mut asking = node("Auditor", NodeStatus::Waiting);
    asking.summary = "\nwhich log sinks are in scope?\nmore".into();
    let r = with_nodes(RunState::WaitingAnswer, vec![node("Router", NodeStatus::Done), asking]);
    assert_eq!(card_state(&i, Some(&r)), CardState::Waiting { text: "asks: which log sinks are in scope?".into() });
    let r = with_nodes(RunState::WaitingAnswer, vec![node("Auditor", NodeStatus::Waiting)]);
    assert_eq!(card_state(&i, Some(&r)), CardState::Waiting { text: "asks you a question".into() });
}

#[test]
fn done_names_the_pull_request() {
    let mut i = issue(IssueStatus::Done);
    let mut r = run("r", RunState::Done);
    assert_eq!(card_state(&i, Some(&r)), CardState::Done { text: "Finished".into() });
    r.pr_url = Some("https://github.com/acme/pay/pull/2280".into());
    assert_eq!(card_state(&i, Some(&r)), CardState::Done { text: "PR #2280 merged".into() });
    i.status = IssueStatus::InReview;
    assert_eq!(card_state(&i, Some(&r)), CardState::Done { text: "PR #2280 open".into() });
    r.pr_url = None;
    i.pr_url = Some("https://example.com/pr/".into());
    assert_eq!(card_state(&i, Some(&r)), CardState::Done { text: "PR open".into() });
    assert_eq!(pr_number("https://gitlab.com/a/b/-/merge_requests/12/"), Some(12));
    assert_eq!(pr_number("nonsense"), None);
}

#[test]
fn failures_and_stops() {
    let i = issue(IssueStatus::InProgress);
    let mut r = run("r", RunState::Failed);
    assert_eq!(card_state(&i, Some(&r)), CardState::Failed { text: "Failed".into() });
    r.error = Some("\ntests failed\ntrace".into());
    assert_eq!(card_state(&i, Some(&r)), CardState::Failed { text: "Failed · tests failed".into() });
    assert_eq!(card_state(&i, Some(&run("r", RunState::Interrupted))), CardState::Failed { text: "Interrupted · resume it".into() });
    assert_eq!(card_state(&i, Some(&run("r", RunState::Cancelled))), CardState::Idle);
}
