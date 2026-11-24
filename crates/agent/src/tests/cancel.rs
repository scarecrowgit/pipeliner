//! Cancelling at every point of a turn, and resuming what an interruption left behind. In
//! each case the stored conversation must be one the next request can be built from.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use llm::{Reply, StreamEvent};
use model::{Message, PermissionDecision, StopReason};
use serde_json::json;

use super::*;
use crate::StopCause;
use crate::history::INTERRUPTED;

#[test]
fn text_streamed_before_a_cancel_is_kept_and_the_next_request_is_valid() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("Let me think about this for a while"));
    let cancel = fx.cancel.clone();
    let mut seen = Vec::new();
    let mut sink = |_: &SessionId, event: AgentEvent| {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            cancel.cancel();
        }
        seen.push(event);
    };
    let outcome = fx.run_into(&AllowAll, say("Think"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert_eq!(outcome.text, "Let ");
    assert_eq!(fx.transcript(), vec![Message::user("Think"), Message::assistant("Let ")]);
    assert_eq!(seen.last(), Some(&AgentEvent::TurnFinished { stop: StopReason::Cancelled }));

    let mut fx = fx;
    fx.cancel = CancelToken::new();
    fx.llm.push(Reply::text("Here is the answer."));
    let (outcome, _) = fx.run(say("Go on"));
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(
        fx.requests()[1].messages,
        vec![Message::user("Think"), Message::assistant("Let "), Message::user("Go on")]
    );
}

#[test]
fn a_call_that_arrived_before_the_cancel_is_answered_as_interrupted_and_not_run() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "touch made.txt"})).and_text("and then some more words"));
    let cancel = fx.cancel.clone();
    let mut finished = Vec::new();
    let mut sink = |_: &SessionId, event: AgentEvent| match event {
        AgentEvent::ToolCallReady { .. } => cancel.cancel(),
        AgentEvent::ToolFinished { id, is_error, .. } => finished.push((id, is_error)),
        _ => {}
    };
    let outcome = fx.run_into(&AllowAll, say("Make it"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(!fx.work().join("made.txt").exists());
    assert_eq!(
        fx.transcript(),
        vec![
            Message::user("Make it"),
            assistant(vec![tool_use("t1", "bash", json!({"command": "touch made.txt"}))]),
            user(vec![error("t1", INTERRUPTED)]),
        ]
    );
    assert_eq!(finished, [("t1".to_string(), true)]);
}

#[test]
fn a_cancel_during_a_tool_keeps_its_result_and_interrupts_the_calls_after_it() {
    let mut fx = Fixture::new();
    let cancel = fx.cancel.clone();
    fx.extra.register(Arc::new(Probe {
        name: "slow",
        run: move |_: &ToolContext<'_>, _: &Value| {
            cancel.cancel();
            "stopped half-way".to_string()
        },
    }));
    fx.llm.push(
        Reply::tool("t1", "slow", json!({}))
            .and_tool("t2", "bash", json!({"command": "touch made.txt"}))
            .and_tool("t3", "spawn_agent", json!({"task": "never"})),
    );
    let (outcome, _) = fx.run(say("Go"));

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert_eq!(outcome.totals.tool_calls, 3);
    assert!(!fx.work().join("made.txt").exists());
    let transcript = fx.transcript();
    assert_eq!(
        transcript[2],
        user(vec![result("t1", "stopped half-way"), error("t2", INTERRUPTED), error("t3", INTERRUPTED)])
    );
    assert_paired(&transcript);
    assert_eq!(fx.llm.call_count(), 1);
}

#[test]
fn a_running_command_is_stopped_by_a_cancel_from_another_thread() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "sleep 30"})));
    let running = AtomicBool::new(false);
    let cancel = fx.cancel.clone();
    let cancelled_at = Mutex::new(None);
    let outcome = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !running.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_millis(150));
            *lock(&cancelled_at) = Some(Instant::now());
            cancel.cancel();
        });
        let mut sink = |_: &SessionId, event: AgentEvent| {
            if matches!(event, AgentEvent::ToolCallReady { .. }) {
                running.store(true, Ordering::SeqCst);
            }
        };
        fx.run_into(&AllowAll, say("Wait"), &mut sink)
    });

    assert_eq!(outcome.stop, StopCause::Cancelled);
    let took = lock(&cancelled_at).expect("the cancel happened").elapsed();
    assert!(took < Duration::from_secs(1), "the command was not stopped at once: {took:?}");
    let transcript = fx.transcript();
    assert_eq!(transcript.len(), 3);
    assert_paired(&transcript);
}

#[test]
fn a_cancel_while_a_person_is_asked_answers_the_call_as_interrupted() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "touch made.txt"})).and_tool("t2", "list_dir", json!({})));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let cancel = fx.cancel.clone();
    // The person never answers: the engine's callback returns once the token fires.
    let gate = asker.gate(move |_| {
        cancel.cancel();
        None::<PermissionDecision>
    });
    let (outcome, _) = fx.run_gated(&gate, say("Make it"));

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert_eq!(asker.asked(), 1);
    assert!(!fx.work().join("made.txt").exists());
    let transcript = fx.transcript();
    assert_eq!(transcript[2], user(vec![error("t1", INTERRUPTED), error("t2", INTERRUPTED)]));
    assert_paired(&transcript);
}

#[test]
fn a_cancel_before_the_first_request_keeps_the_message() {
    let fx = Fixture::new();
    fx.cancel.cancel();
    let (outcome, log) = fx.run(say("Hi"));
    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert_eq!(fx.llm.call_count(), 0);
    assert_eq!(fx.transcript(), vec![Message::user("Hi")]);
    assert_eq!(kinds(&log, fx.session()), ["message", "turn_finished"]);
}

#[test]
fn a_cancel_during_a_retry_pause_stops_at_once() {
    let mut fx = Fixture::new();
    fx.spec.limits.retry_base = Duration::from_secs(60);
    fx.spec.limits.retry_max = Duration::from_secs(60);
    fx.llm.push(Reply::error(llm::LlmError::Overloaded("busy".into())));
    let cancel = fx.cancel.clone();
    let mut sink = |_: &SessionId, event: AgentEvent| {
        if matches!(event, AgentEvent::Retrying { .. }) {
            cancel.cancel();
        }
    };
    let started = Instant::now();
    let outcome = fx.run_into(&AllowAll, say("Hi"), &mut sink);
    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(fx.transcript(), vec![Message::user("Hi")]);
    assert_eq!(fx.llm.call_count(), 1, "no attempt after the cancel");
}

#[test]
fn a_session_resumed_with_unanswered_calls_answers_them_before_anything_else() {
    let dangling = || {
        vec![
            Message::user("Do it"),
            assistant(vec![
                text("Running both."),
                tool_use("t1", "bash", json!({"command": "touch made.txt"})),
                tool_use("t2", "read_file", json!({"path": "a.txt"})),
            ]),
        ]
    };
    let answered = user(vec![error("t1", INTERRUPTED), error("t2", INTERRUPTED)]);

    // Continue: the results are stored first, then the model is asked.
    let fx = Fixture::new();
    fx.host.seed(fx.session(), dangling());
    fx.llm.push(Reply::text("They were interrupted; running again is safe."));
    let (outcome, log) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    let mut expected = dangling();
    expected.push(answered.clone());
    assert_eq!(fx.requests()[0].messages, expected);
    assert_eq!(fx.transcript()[..3], expected[..]);
    assert!(!fx.work().join("made.txt").exists(), "an interrupted call was run again");
    assert_eq!(kinds(&log, fx.session())[..3], ["tool_finished", "tool_finished", "message"]);
    assert_paired(&fx.transcript());

    // A new message: the results come first in the same user turn.
    let fx = Fixture::new();
    fx.host.seed(fx.session(), dangling());
    fx.llm.push(Reply::text("Hello."));
    fx.run(say("Are you there?"));
    let mut expected = dangling();
    expected.push(user(vec![error("t1", INTERRUPTED), error("t2", INTERRUPTED), text("Are you there?")]));
    assert_eq!(fx.requests()[0].messages, expected);
    assert_paired(&fx.transcript());
    assert_eq!(fx.transcript()[2], answered);
}

#[test]
fn a_silent_model_stream_is_cut_by_a_cancel_from_another_thread() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("Never said.").with_delay(Duration::from_secs(30)));
    let cancel = fx.cancel.clone();
    let cancelled_at = Mutex::new(None);
    let (outcome, log) = std::thread::scope(|scope| {
        scope.spawn(|| {
            while fx.llm.call_count() == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            *lock(&cancelled_at) = Some(Instant::now());
            cancel.cancel();
        });
        fx.run(say("Hi"))
    });

    assert_eq!(outcome.stop, StopCause::Cancelled);
    let took = lock(&cancelled_at).expect("the cancel happened").elapsed();
    assert!(took < Duration::from_secs(1), "{took:?}");
    assert_eq!(fx.transcript(), vec![Message::user("Hi")]);
    assert_eq!(kinds(&log, fx.session()), ["message", "turn_started", "turn_finished"]);
}

#[test]
fn a_cancel_between_the_reply_and_its_first_tool_runs_nothing() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "touch made.txt"})).and_tool("t2", "list_dir", json!({})));
    let cancel = fx.cancel.clone();
    let mut sink = |_: &SessionId, event: AgentEvent| {
        // The reply arrived whole and is stored; no tool has started.
        if matches!(&event, AgentEvent::MessageCommitted { message } if message.message.role == Role::Assistant) {
            cancel.cancel();
        }
    };
    let outcome = fx.run_into(&AllowAll, say("Make it"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(!fx.work().join("made.txt").exists());
    let transcript = fx.transcript();
    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[2], user(vec![error("t1", INTERRUPTED), error("t2", INTERRUPTED)]));
    assert_paired(&transcript);
    assert_eq!(fx.llm.call_count(), 1);

    // What is stored is what the next request sends.
    let mut fx = fx;
    fx.cancel = CancelToken::new();
    fx.llm.push(Reply::text("Nothing was made."));
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.requests()[1].messages, transcript);
}

#[test]
fn an_approval_that_crosses_the_cancel_starts_nothing() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "touch made.txt"})));
    let rules = MemoryRules::default();
    let asker = Asker::new(&rules);
    let cancel = fx.cancel.clone();
    // The person clicks Allow in the moment the run is stopped.
    let gate = asker.gate(move |_| {
        cancel.cancel();
        Some(PermissionDecision::AllowOnce)
    });
    let (outcome, _) = fx.run_gated(&gate, say("Make it"));

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(!fx.work().join("made.txt").exists(), "a call started after the cancel");
    assert_eq!(fx.transcript()[2], user(vec![error("t1", INTERRUPTED)]));
}

#[test]
fn a_stream_that_ends_without_its_stop_after_a_cancel_counts_as_cancelled() {
    let fx = Fixture::new();
    // A client that returns quietly when the token fires, instead of reporting it.
    let llm = FnLlm(|_: &ChatRequest, cancel: &CancelToken, on_event: &mut dyn FnMut(StreamEvent)| {
        on_event(StreamEvent::TextDelta("Half an".into()));
        on_event(StreamEvent::ToolUseStart { id: "t1".into(), name: "bash".into() });
        cancel.cancel();
        Ok(())
    });
    let mut log = EventLog::new();
    let outcome = run(&fx.spec, &fx.env, &fx.deps(&llm, &AllowAll), say("Hi"), &mut log, &fx.cancel);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert_eq!(fx.transcript(), vec![Message::user("Hi"), Message::assistant("Half an")]);
    assert_eq!(log.events.last().map(|(_, e)| e), Some(&AgentEvent::TurnFinished { stop: StopReason::Cancelled }));
}
