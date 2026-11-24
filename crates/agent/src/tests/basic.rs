//! The turn loop itself: answers, tool conversations, limits, failures, what is sent.

use std::sync::Arc;

use llm::{LlmError, Reply};
use model::{AgentEvent, Attachment, BlobId, ContentBlock, FileChange, Message, ModelRef, StopReason, Usage};
use serde_json::json;
use tools::{EditFile, ReadFile};

use super::*;
use crate::notes::note;
use crate::{GateCall, Host, StopCause, UserInput, Verdict};

#[test]
fn a_plain_answer_is_stored_and_reported() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("Hello, I am here."));
    let (outcome, log) = fx.run(say("Hi"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "Hello, I am here.");
    assert_eq!(outcome.payload, None);
    assert_eq!((outcome.totals.steps, outcome.totals.tool_calls), (1, 0));
    assert_eq!(fx.transcript(), vec![Message::user("Hi"), Message::assistant("Hello, I am here.")]);

    let requests = fx.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages, vec![Message::user("Hi")]);
    assert!(requests[0].system.starts_with("You are Build, a coding agent.\n\n# Environment\n"));
    assert_eq!(
        kinds(&log, fx.session()),
        ["message", "turn_started", "text", "text", "text", "text", "message", "turn_finished"]
    );
    assert_eq!(log.events.last().map(|(_, e)| e), Some(&AgentEvent::TurnFinished { stop: StopReason::EndTurn }));
    let stored = fx.host.stored(fx.session());
    assert_eq!(stored.iter().map(|m| m.seq).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(stored[1].model, Some(fx.spec.model.clone()));
    assert_eq!(stored[1].at, NOW);
}

#[test]
fn a_tool_conversation_edits_a_real_file_and_sends_exactly_the_stored_history() {
    let fx = Fixture::new();
    let source = "fn retry() {\n    attempts(3);\n}\n";
    fx.write("src/retry.rs", source);
    let read_input = json!({"path": "src/retry.rs"});
    let edit_input = json!({"path": "src/retry.rs", "old_string": "attempts(3)", "new_string": "attempts(5)"});
    fx.llm.push(Reply::text("I'll read it.").and_tool("t1", "read_file", read_input.clone()));
    fx.llm.push(Reply::tool("t2", "edit_file", edit_input.clone()));
    fx.llm.push(Reply::text("Changed the retry count to 5."));

    let (outcome, log) = fx.run(say("Raise the retry count to 5"));

    assert_eq!(fx.read("src/retry.rs"), "fn retry() {\n    attempts(5);\n}\n");
    let files = [("src/retry.rs", source)];
    let read = fx.twin_output(&ReadFile, &files, read_input.clone());
    let edit = fx.twin_output(&EditFile, &files, edit_input.clone());
    assert!(!read.is_error && !edit.is_error);
    let expected = vec![
        Message::user("Raise the retry count to 5"),
        assistant(vec![text("I'll read it."), tool_use("t1", "read_file", read_input)]),
        user(vec![result("t1", &read.content)]),
        assistant(vec![tool_use("t2", "edit_file", edit_input)]),
        user(vec![result("t2", &edit.content)]),
        Message::assistant("Changed the retry count to 5."),
    ];
    assert_eq!(fx.transcript(), expected);
    assert_paired(&fx.transcript());

    // Each request is the stored conversation up to that point, nothing more.
    let requests = fx.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].messages, expected[..1]);
    assert_eq!(requests[1].messages, expected[..3]);
    assert_eq!(requests[2].messages, expected[..5]);
    assert_eq!(
        tool_names(&requests[0]),
        ["ask_user", "bash", "edit_file", "grep", "list_dir", "read_file", "report_to_inbox", "todo_write", "write_file"]
    );
    assert!(requests.iter().all(|r| r.system == requests[0].system && r.tools == requests[0].tools));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "Changed the retry count to 5.");
    assert_eq!((outcome.totals.steps, outcome.totals.tool_calls), (3, 2));
    let changed = vec![FileChange { path: "src/retry.rs".into(), added: 1, removed: 1 }];
    assert_eq!(outcome.files, changed);
    assert_eq!(fx.host.files(fx.session()).expect("files"), changed);
    assert!(log.events.iter().any(|(_, e)| *e == AgentEvent::FilesChanged { files: changed.clone() }));
    let ready: Vec<_> = log
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::ToolCallReady { id, name, class, .. } => Some((id.as_str(), name.as_str(), *class)),
            _ => None,
        })
        .collect();
    assert_eq!(ready, [("t1", "read_file", ToolClass::Read), ("t2", "edit_file", ToolClass::Write)]);
    let finished = log.events.iter().filter(|(_, e)| matches!(e, AgentEvent::ToolFinished { is_error: false, .. })).count();
    assert_eq!(finished, 2);
}

#[test]
fn the_system_prompt_is_byte_identical_across_the_turns_and_runs_of_a_session() {
    let fx = Fixture::with_skills(&[("style", "House style", "Use tabs.")]);
    fx.write("AGENTS.md", "Run `make check` before you finish.");
    fx.write("a.txt", "one\n");
    fx.llm.push(Reply::tool("t1", "write_file", json!({"path": "new.txt", "content": "made\n"})));
    fx.llm.push(Reply::tool("t2", "todo_write", json!({"items": [{"text": "Check", "status": "in_progress"}]})));
    fx.llm.push(Reply::text("Done."));
    fx.run(say("Make a file"));
    fx.llm.push(Reply::text("Again."));
    fx.clock.advance(3_600_000);
    fx.run(say("And once more"));

    let requests = fx.requests();
    assert_eq!(requests.len(), 4);
    let system = &requests[0].system;
    assert!(requests.iter().all(|r| &r.system == system), "the prompt changed between requests");
    assert!(system.contains(&format!("- Working folder: {}.", fx.work().display())));
    assert!(system.contains("- Platform: Linux\n- Date: "));
    assert!(system.contains("23 November 2026"), "{system}");
    assert!(system.contains("<project-notes>\nRun `make check` before you finish.\n</project-notes>"));
    assert!(system.contains("- style: House style"));
    // Nothing of the moving state is in it.
    assert!(!system.contains("Check") && !system.contains("new.txt") && !system.contains("12:"));
}

#[test]
fn usage_goes_to_the_ledger_the_message_the_event_and_the_totals() {
    let mut fx = Fixture::new();
    fx.spec.model = ModelRef::new("scripted", "claude-sonnet-5");
    let usage = Usage { input: 1_000, output: 200, cache_read: 0, cache_write: 0 };
    fx.llm.push(Reply::tool("t1", "todo_write", json!({"items": []})).with_usage(usage));
    fx.llm.push(Reply::text("Done.").with_usage(usage));
    let (outcome, log) = fx.run(say("Go"));

    let cost = 0.004;
    assert!((outcome.totals.cost_usd - 2.0 * cost).abs() < 1e-12);
    assert_eq!(outcome.totals.usage, Usage { input: 2_000, output: 400, cache_read: 0, cache_write: 0 });
    let ledger = fx.host.ledger();
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0].session.as_ref(), Some(fx.session()));
    assert_eq!((ledger[0].runnable.as_str(), ledger[0].name.as_str()), ("agent:build", "Build"));
    assert_eq!(ledger[0].model, fx.spec.model);
    assert_eq!(ledger[0].usage, usage);
    assert!((ledger[0].cost_usd - cost).abs() < 1e-12);
    assert!(!ledger[0].imported && ledger[0].created_by.is_none());
    assert!((fx.host.spent(fx.session()).expect("spent") - 2.0 * cost).abs() < 1e-12);

    let stored = fx.host.stored(fx.session());
    assert_eq!(stored[1].usage, usage);
    assert!((stored[1].cost_usd - cost).abs() < 1e-12);
    assert_eq!(stored[2].usage, Usage::default(), "tool results cost nothing");
    let reported = log.events.iter().filter(|(_, e)| matches!(e, AgentEvent::Usage { context_tokens: 1_000, context_window: 1_000_000, .. })).count();
    assert_eq!(reported, 2);
}

#[test]
fn an_unknown_tool_is_answered_with_the_tools_that_exist() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "delete_everything", json!({"path": "/"})));
    fx.llm.push(Reply::text("Sorry."));
    let (outcome, _) = fx.run(say("Go"));
    assert_eq!(outcome.stop, StopCause::Finished);
    let (content, is_error) = result_of(&fx.transcript(), "t1");
    assert!(is_error);
    assert!(content.starts_with("There is no tool `delete_everything`. The tools you can call: ask_user, bash, "), "{content}");
    assert_paired(&fx.transcript());
}

#[test]
fn invalid_tool_json_is_answered_and_the_tool_does_not_run() {
    let fx = Fixture::new();
    fx.llm.push(Reply::new().and_raw_tool("t1", "write_file", r#"{"path": "x.txt", "content": "half"#));
    fx.llm.push(Reply::text("Let me retry."));
    let (outcome, log) = fx.run(say("Write x"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert!(!fx.work().join("x.txt").exists(), "the tool ran on broken input");
    let transcript = fx.transcript();
    assert_eq!(transcript[1], assistant(vec![tool_use("t1", "write_file", json!({}))]));
    let (content, is_error) = result_of(&transcript, "t1");
    assert!(is_error);
    assert!(content.starts_with("The input of `write_file` could not be read, so the tool was not run: "), "{content}");
    assert!(content.contains("not valid JSON"));
    assert_paired(&transcript);
    assert_eq!(fx.requests()[1].messages, transcript[..3]);
    assert!(log.events.iter().any(|(_, e)| matches!(e, AgentEvent::ToolFinished { id, is_error: true, .. } if id == "t1")));
}

#[test]
fn a_panicking_tool_becomes_an_error_result_and_the_run_goes_on() {
    let mut fx = Fixture::new();
    fx.extra.register(Arc::new(Probe { name: "explode", run: |_: &ToolContext<'_>, _: &Value| -> String { panic!("boom {}", 42) } }));
    fx.llm.push(Reply::tool("t1", "explode", json!({})).and_tool("t2", "list_dir", json!({"path": "."})));
    fx.llm.push(Reply::text("That tool is broken."));
    let (outcome, _) = fx.run(say("Go"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.totals.tool_calls, 2);
    let transcript = fx.transcript();
    let (content, is_error) = result_of(&transcript, "t1");
    assert!(is_error);
    assert_eq!(content, "`explode` crashed (boom 42). It may have done part of its work; check the state before you try again.");
    assert!(!result_of(&transcript, "t2").1, "the call after the crash still ran");
    assert_paired(&transcript);
}

/// A gate whose rule store is broken.
struct BrokenGate;

impl PermissionGate for BrokenGate {
    fn check(&self, call: &GateCall<'_>) -> Verdict {
        panic!("no rules for {}", call.tool)
    }

    fn ask(&self, _request: PermissionRequest, _cancel: &CancelToken) -> Option<PermissionDecision> {
        None
    }
}

#[test]
fn a_panic_anywhere_under_a_call_is_that_calls_error_result() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "list_dir", json!({})).and_tool("t2", "todo_write", json!({"items": []})));
    fx.llm.push(Reply::text("The folder cannot be listed."));
    let (outcome, _) = fx.run_gated(&BrokenGate, say("Go"));

    assert_eq!(outcome.stop, StopCause::Finished);
    let transcript = fx.transcript();
    assert_eq!(
        result_of(&transcript, "t1"),
        (
            "`list_dir` crashed (no rules for list_dir). It may have done part of its work; check the state before \
             you try again."
                .to_string(),
            true
        )
    );
    assert!(!result_of(&transcript, "t2").1, "a session tool does not pass the gate");
    assert_paired(&transcript);
}

#[test]
fn a_result_that_cannot_be_booked_is_still_stored_before_the_run_fails() {
    let fx = Fixture::new();
    fx.host.fail_files(true);
    fx.llm.push(
        Reply::tool("t1", "write_file", json!({"path": "a.txt", "content": "one\n"})).and_tool("t2", "list_dir", json!({})),
    );
    let (outcome, _) = fx.run(say("Write it"));

    assert_eq!(outcome.stop, StopCause::Failed { error: "the file list is not writable".into() });
    assert_eq!(fx.read("a.txt"), "one\n");
    let transcript = fx.transcript();
    assert!(outcome.files.is_empty(), "the run reports a file list the store does not hold");
    assert_eq!(transcript.len(), 3, "both calls are answered: the conversation can go on");
    assert!(!result_of(&transcript, "t1").1 && !result_of(&transcript, "t2").1);
    assert_paired(&transcript);
}

#[test]
fn tool_call_ids_a_provider_repeats_are_made_unique_in_the_conversation() {
    let fx = Fixture::new();
    // A gateway that numbers the calls of every response from zero.
    let todo = json!({"items": []});
    fx.llm.push(Reply::tool("call_0", "todo_write", todo.clone()));
    fx.llm.push(Reply::tool("call_0", "todo_write", todo.clone()).and_tool("call_1", "todo_write", todo));
    fx.llm.push(Reply::text("Done."));
    let (outcome, log) = fx.run(say("Go"));

    assert_eq!(outcome.stop, StopCause::Finished);
    let transcript = fx.transcript();
    let ids: Vec<&str> = transcript.iter().flat_map(|m| m.tool_uses()).map(|(id, _, _)| id).collect();
    assert_eq!(ids, ["call_0", "call_1", "call_2"]);
    assert_paired(&transcript);
    assert_eq!(fx.requests()[2].messages, transcript[..5]);
    let finished: Vec<&str> = log
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::ToolFinished { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(finished, ids);
}

#[test]
fn the_step_limit_stops_the_run_and_continue_carries_on() {
    let mut fx = Fixture::new();
    fx.spec.max_steps = 2;
    let todo = json!({"items": [{"text": "Work", "status": "in_progress"}]});
    fx.llm.push(Reply::tool("t1", "todo_write", todo.clone()));
    fx.llm.push(Reply::tool("t2", "todo_write", todo.clone()));
    let (outcome, log) = fx.run(say("Go"));
    assert_eq!(outcome.stop, StopCause::StepLimit);
    assert_eq!(outcome.totals.steps, 2);
    assert_eq!(fx.llm.call_count(), 2);
    assert!(log.events.iter().any(|(_, e)| *e == AgentEvent::Notice { text: "Stopped after 2 steps. Send a message to let it continue.".into() }));
    assert_eq!(log.events.last().map(|(_, e)| e), Some(&AgentEvent::TurnFinished { stop: StopReason::Other }));
    assert_paired(&fx.transcript());

    fx.llm.push(Reply::text("Finished."));
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.requests()[2].messages, fx.transcript()[..5]);
}

#[test]
fn the_budget_is_checked_before_every_model_call() {
    let mut fx = Fixture::new();
    fx.spec.model = ModelRef::new("scripted", "claude-sonnet-5");
    fx.spec.budget_usd = Some(0.005);
    let usage = Usage { input: 1_000, output: 200, cache_read: 0, cache_write: 0 };
    let todo = json!({"items": []});
    fx.llm.push(Reply::tool("t1", "todo_write", todo.clone()).with_usage(usage));
    fx.llm.push(Reply::tool("t2", "todo_write", todo.clone()).with_usage(usage));
    let (outcome, log) = fx.run(say("Go"));
    assert_eq!(outcome.stop, StopCause::Budget);
    assert_eq!(fx.llm.call_count(), 2, "the third call was not made");
    assert_paired(&fx.transcript());
    assert!(log.events.iter().any(|(_, e)| matches!(e, AgentEvent::Notice { text } if text.starts_with("Stopped: the budget of "))));

    // Already spent before the run starts: no call at all, the message is still stored.
    let (outcome, _) = fx.run(say("More"));
    assert_eq!(outcome.stop, StopCause::Budget);
    assert_eq!(fx.llm.call_count(), 2);
    assert_eq!(fx.transcript().last(), Some(&Message::user("More")));
}

#[test]
fn a_retryable_failure_is_retried_and_the_broken_attempt_leaves_nothing_behind() {
    let fx = Fixture::new();
    fx.llm.push(Reply::error(LlmError::Overloaded("busy".into())));
    fx.llm.push(Reply::text("Half an answer that").then_fail(LlmError::Network("reset".into())));
    fx.llm.push(Reply::text("The whole answer."));
    let (outcome, log) = fx.run(say("Hi"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "The whole answer.");
    assert_eq!(outcome.totals.steps, 1, "retries are not steps");
    assert_eq!(fx.transcript(), vec![Message::user("Hi"), Message::assistant("The whole answer.")]);
    let retries: Vec<_> = log
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::Retrying { attempt, reason } => Some((*attempt, reason.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(retries, [(1, "the provider is overloaded: busy".to_string()), (2, "network error: reset".to_string())]);
    let requests = fx.requests();
    assert!(requests.iter().all(|r| r.messages == requests[0].messages));
}

#[test]
fn retries_are_bounded_and_then_the_run_fails() {
    let mut fx = Fixture::new();
    fx.spec.limits.retries = 2;
    for _ in 0..3 {
        fx.llm.push(Reply::error(LlmError::RateLimited { retry_after: None, message: "slow down".into() }));
    }
    let (outcome, log) = fx.run(say("Hi"));
    assert_eq!(outcome.stop, StopCause::Failed { error: "rate limited: slow down (after 3 attempts)".into() });
    assert_eq!(fx.llm.call_count(), 3);
    assert_eq!(
        log.events.last().map(|(_, e)| e),
        Some(&AgentEvent::Failed { error: "rate limited: slow down (after 3 attempts)".into() })
    );
}

#[test]
fn a_failure_that_cannot_be_retried_ends_the_run_with_its_message() {
    let fx = Fixture::new();
    fx.llm.push(Reply::error(LlmError::Auth("invalid x-api-key".into())));
    let (outcome, log) = fx.run(say("Hi"));
    let error = "the provider rejected the API key: invalid x-api-key".to_string();
    assert_eq!(outcome.stop, StopCause::Failed { error: error.clone() });
    assert_eq!(fx.llm.call_count(), 1);
    assert_eq!(kinds(&log, fx.session()), ["message", "turn_started", "failed"]);
    assert_eq!(log.events.last().map(|(_, e)| e), Some(&AgentEvent::Failed { error }));
    // The person's message is kept, so a retry sends it again.
    assert_eq!(fx.transcript(), vec![Message::user("Hi")]);
    fx.llm.push(Reply::text("Now it works."));
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.requests()[1].messages, vec![Message::user("Hi")]);
}

#[test]
fn a_store_that_cannot_be_written_fails_the_run() {
    let fx = Fixture::new();
    fx.host.fail_appends(true);
    let (outcome, log) = fx.run(say("Hi"));
    assert_eq!(outcome.stop, StopCause::Failed { error: "the store is not writable".into() });
    assert_eq!(fx.llm.call_count(), 0);
    assert_eq!(kinds(&log, fx.session()), ["failed"]);
}

#[test]
fn messages_sent_while_the_agent_is_busy_arrive_at_its_next_turn() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "list_dir", json!({"path": "."})));
    fx.llm.push(Reply::text("Listed."));
    fx.llm.push(Reply::text("And the tests are fine."));
    let host = &fx.host;
    let session = fx.session().clone();
    let mut queued = 0;
    let mut sink = |_: &SessionId, event: AgentEvent| {
        // One message arrives while the tool runs, one while the last answer streams.
        let busy = matches!(event, AgentEvent::ToolFinished { .. });
        let answering = matches!(&event, AgentEvent::TextDelta { text } if text.starts_with("Listed"));
        if busy || answering {
            queued += 1;
            host.queue(&session, UserInput::text(format!("Extra {queued}")));
        }
    };
    let outcome = fx.run_into(&AllowAll, say("List the folder"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "And the tests are fine.");
    let transcript = fx.transcript();
    let listing = result_of(&transcript, "t1").0;
    let requests = fx.requests();
    assert_eq!(
        requests[1].messages,
        vec![
            Message::user("List the folder"),
            assistant(vec![tool_use("t1", "list_dir", json!({"path": "."}))]),
            user(vec![result("t1", &listing), text("Extra 1")]),
        ]
    );
    assert_eq!(requests[2].messages[3..], [Message::assistant("Listed."), Message::user("Extra 2")]);
    assert_paired(&transcript);
}

#[test]
fn images_are_sent_as_blocks_of_the_user_message() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("A cat."));
    let image = Attachment { blob: BlobId::new("blob-9"), name: "cat.png".into(), media_type: "image/png".into(), bytes: 4 };
    fx.run(Input::User(UserInput { text: "What is this?".into(), images: vec![image] }));
    let expected = user(vec![
        text("What is this?"),
        ContentBlock::Image { blob: BlobId::new("blob-9"), media_type: "image/png".into(), name: "cat.png".into(), bytes: 4 },
    ]);
    assert_eq!(fx.requests()[0].messages, vec![expected]);
}

#[test]
fn a_reply_cut_off_at_the_output_limit_is_asked_to_continue() {
    let fx = Fixture::new();
    fx.llm.push(Reply::text("Part one,").with_stop(StopReason::MaxTokens));
    fx.llm.push(Reply::text("part two."));
    let (outcome, _) = fx.run(say("Write a lot"));
    assert_eq!(outcome.stop, StopCause::Finished);
    let transcript = fx.transcript();
    assert_eq!(transcript.len(), 4);
    assert_eq!(transcript[1], Message::assistant("Part one,"));
    assert!(crate::notes::is_note(&transcript[2].plain_text()));
    assert!(transcript[2].plain_text().contains("cut off at the output limit"));
    assert_eq!(transcript[3], Message::assistant("part two."));
}

#[test]
fn an_empty_reply_stores_nothing_and_continue_needs_a_conversation() {
    let fx = Fixture::new();
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Failed { error: "There is no conversation to continue.".into() });

    fx.llm.push(Reply::thinking("Nothing to add."));
    let (outcome, _) = fx.run(say("Hi"));
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "");
    assert_eq!(fx.transcript(), vec![Message::user("Hi")]);

    // Continuing after a finished answer needs something for the model to answer.
    fx.llm.push(Reply::text("Hello."));
    fx.run(say("Hi again"));
    fx.llm.push(Reply::text("Still here."));
    fx.run(Input::Continue);
    let last = fx.requests().pop().expect("request");
    assert_eq!(last.messages.last(), Some(&Message::user(note("Continue from where you stopped."))));
}

#[test]
fn pull_request_links_are_reported_once() {
    let fx = Fixture::new();
    fx.llm.push(Reply::tool("t1", "bash", json!({"command": "echo https://github.com/acme/pay/pull/7"})));
    fx.llm.push(Reply::text("Opened https://github.com/acme/pay/pull/7 for you."));
    let (_, log) = fx.run(say("Open a PR"));
    let found: Vec<_> = log
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            AgentEvent::PrDetected { url } => Some(url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(found, ["https://github.com/acme/pay/pull/7"]);
}

#[test]
fn a_reply_the_ledger_did_not_take_is_stored_before_the_run_fails() {
    let fx = Fixture::new();
    fx.host.fail_ledger(true);
    let usage = Usage { input: 10, output: 5, ..Default::default() };
    fx.llm.push(Reply::text("Reading.").and_tool("t1", "list_dir", json!({})).with_usage(usage));
    let (outcome, _) = fx.run(say("Hi"));

    assert_eq!(
        outcome.stop,
        StopCause::Failed {
            error: "A model call could not be added to the spend ledger: the ledger is not writable".into()
        }
    );
    // The reply was paid for and streamed to the person: it is kept, and its call is answered.
    assert_eq!(outcome.text, "Reading.");
    assert_eq!(outcome.totals.usage, usage);
    let transcript = fx.transcript();
    assert_eq!(
        transcript,
        vec![
            Message::user("Hi"),
            assistant(vec![text("Reading."), tool_use("t1", "list_dir", json!({}))]),
            user(vec![error("t1", crate::history::INTERRUPTED)]),
        ]
    );
    assert_paired(&transcript);
}

#[test]
fn queued_messages_that_cannot_be_stored_stay_queued_in_their_order() {
    let fx = Fixture::new();
    let host = &fx.host;
    let session = fx.session().clone();
    let mut sink = |_: &SessionId, event: AgentEvent| {
        // Two messages arrive, and the store breaks before the loop has kept them.
        if matches!(event, AgentEvent::MessageCommitted { .. }) {
            host.queue(&session, UserInput::text("First"));
            host.queue(&session, UserInput::text("Second"));
            host.fail_appends(true);
        }
    };
    let outcome = fx.run_into(&AllowAll, say("Hi"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Failed { error: "the store is not writable".into() });
    assert_eq!(fx.host.queued(fx.session()), [UserInput::text("First"), UserInput::text("Second")]);

    // Once the store works again they arrive, before what was sent since.
    fx.host.queue(fx.session(), UserInput::text("Third"));
    fx.host.fail_appends(false);
    fx.llm.push(Reply::text("All three read."));
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(
        fx.requests()[0].messages,
        vec![user(vec![text("Hi"), text("First"), text("Second"), text("Third")])]
    );

    // What is put back goes ahead of what was queued meanwhile.
    fx.host.queue(fx.session(), UserInput::text("C"));
    fx.host.requeue(fx.session(), vec![UserInput::text("A"), UserInput::text("B")]);
    let texts: Vec<String> = fx.host.take_queued(fx.session()).into_iter().map(|input| input.text).collect();
    assert_eq!(texts, ["A", "B", "C"]);
}
