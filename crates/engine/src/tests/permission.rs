//! Permissions: a person allows once, rejects, always allows (kept in the store and honoured
//! next turn), or cancels while the question is open.

use std::time::{Duration, Instant};

use llm::Reply;
use model::{ContentBlock, Event, PermissionDecision, PermissionRequest, SessionState};
use serde_json::json;

use super::Fx;
use crate::api::{ChatApi, OpsApi};

fn asked(fx: &Fx) -> PermissionRequest {
    match fx.wait_for("a permission request", |e| matches!(e, Event::PermissionRequested { .. })) {
        Event::PermissionRequested { request } => request,
        _ => unreachable!(),
    }
}

fn edit(from: &str, to: &str) -> Reply {
    Reply::tool("t1", "edit_file", json!({"path": "a.txt", "old_string": from, "new_string": to}))
}

fn requests(fx: &Fx) -> usize {
    fx.log.lock().expect("log").iter().filter(|e| matches!(e, Event::PermissionRequested { .. })).count()
}

#[test]
fn a_rejected_call_is_not_run_and_the_model_is_told() {
    let fx = Fx::new();
    let repo = fx.repo("r", &[("a.txt", "one\n")]);
    let session = fx.chat_in(&repo);
    fx.llm.push(edit("one", "two"));
    fx.llm.push(Reply::text("Understood."));
    fx.say(&session.id, "Change it");
    let request = asked(&fx);
    fx.engine.answer_permission(&request.id, PermissionDecision::Reject { note: "not now".into() }).expect("answer");
    let done = fx.settle(&session.id);

    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).expect("read"), "one\n");
    assert_eq!(done.state, SessionState::Idle);
    let messages = fx.engine.messages(&session.id, 1, u64::MAX).expect("messages");
    let rejected = messages.iter().flat_map(|m| &m.message.content).any(|b| {
        matches!(b, ContentBlock::ToolResult { content, is_error: true, .. } if content.contains("rejected") && content.contains("not now"))
    });
    assert!(rejected, "{messages:#?}");
    assert!(fx.engine.answer_permission(&request.id, PermissionDecision::AllowOnce).is_err(), "answered once only");
}

#[test]
fn always_allow_is_kept_in_the_store_and_honoured_in_the_next_turn() {
    let fx = Fx::new();
    let repo = fx.repo("r", &[("a.txt", "one\n")]);
    let session = fx.chat_in(&repo);
    fx.llm.push(edit("one", "two"));
    fx.llm.push(Reply::text("Done."));
    fx.say(&session.id, "Change it");
    let request = asked(&fx);
    let rule = request.always_rule.clone().expect("an always rule is offered");
    fx.engine.answer_permission(&request.id, PermissionDecision::AllowAlways { rule: rule.clone() }).expect("answer");
    fx.settle(&session.id);
    assert_eq!(fx.engine.permission_rules().expect("rules"), vec![rule.clone()]);

    fx.llm.push(edit("two", "three"));
    fx.llm.push(Reply::text("Done again."));
    fx.say(&session.id, "Once more");
    let done = fx.settle(&session.id);
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).expect("read"), "three\n");
    assert_eq!(requests(&fx), 1, "the second edit did not ask");
    assert_eq!(done.state, SessionState::Idle);

    // A new engine over the same store still knows the rule.
    let (engine, _) = fx.reopen();
    assert_eq!(engine.permission_rules().expect("rules"), vec![rule.clone()]);
    engine.remove_permission_rule(&rule).expect("remove");
    assert!(fx.engine.permission_rules().expect("rules").is_empty());
}

#[test]
fn cancelling_while_a_question_is_open_ends_the_turn_promptly() {
    let fx = Fx::new();
    let repo = fx.repo("r", &[("a.txt", "one\n")]);
    let session = fx.chat_in(&repo);
    fx.llm.push(edit("one", "two"));
    fx.say(&session.id, "Change it");
    let request = asked(&fx);

    let started = Instant::now();
    fx.engine.interrupt(&session.id);
    let done = fx.settle(&session.id);
    assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    assert_eq!(done.state, SessionState::Idle);
    assert!(fx.log.lock().expect("log").iter().any(|e| matches!(e, Event::PermissionResolved { id } if *id == request.id)));
    assert!(fx.engine.answer_permission(&request.id, PermissionDecision::AllowOnce).is_err());
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).expect("read"), "one\n");
    assert!(fx.engine.needs_you().expect("needs").is_empty());

    fx.llm.push(Reply::text("Back."));
    fx.say(&session.id, "Are you there?");
    let done = fx.settle(&session.id);
    assert_eq!(done.state, SessionState::Idle);
    let last = fx.engine.messages(&session.id, 1, u64::MAX).expect("messages").pop().expect("last");
    assert_eq!(last.message.plain_text(), "Back.");
}
