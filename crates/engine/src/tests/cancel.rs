//! Cancelling a turn while the model streams and while a long tool runs.

use std::time::{Duration, Instant};

use llm::Reply;
use model::{AgentEvent, Event, PermissionRule, RuleDecision, RuleScope, SessionState};
use serde_json::json;

use super::Fx;
use crate::api::ChatApi;

/// Interrupts once `ready` is seen, then checks the turn ended within a second and the
/// session takes the next message.
fn cancel_then_continue(fx: &Fx, id: &model::SessionId, ready: impl Fn(&Event) -> bool) {
    fx.wait_for("the moment to cancel", ready);
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    fx.engine.interrupt(id);
    let done = fx.settle(id);
    assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    assert_eq!(done.state, SessionState::Idle);
    assert!(fx.agent_events(id).contains(&AgentEvent::TurnFinished { stop: model::StopReason::Cancelled }));

    fx.llm.push(Reply::text("Next answer."));
    fx.say(id, "Next");
    let done = fx.settle(id);
    assert_eq!(done.state, SessionState::Idle);
    let last = fx.engine.messages(id, 1, u64::MAX).expect("messages").pop().expect("last");
    assert_eq!(last.message.plain_text(), "Next answer.");
}

#[test]
fn cancel_during_streaming_returns_at_once_and_the_chat_goes_on() {
    let fx = Fx::new();
    let session = fx.chat_in(&fx.root());
    fx.llm.push(Reply::text("This takes forever.").with_delay(Duration::from_secs(30)));
    fx.say(&session.id, "Talk slowly");
    cancel_then_continue(&fx, &session.id, |e| matches!(e, Event::Agent { event: AgentEvent::TurnStarted, .. }));
}

#[test]
fn cancel_during_a_long_tool_returns_at_once_and_the_chat_goes_on() {
    let fx = Fx::new();
    let repo = fx.repo("r", &[("a.txt", "one\n")]);
    let session = fx.chat_in(&repo);
    let rule = PermissionRule { tool: "bash".into(), pattern: "sleep".into(), decision: RuleDecision::Allow, scope: RuleScope::Global };
    fx.engine.inner.store.add_permission_rule(&rule).expect("rule");
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "sleep 30"})));
    fx.say(&session.id, "Wait a while");
    cancel_then_continue(&fx, &session.id, |e| {
        matches!(e, Event::Agent { event: AgentEvent::ToolCallReady { name, .. }, .. } if name == "bash")
    });
}
