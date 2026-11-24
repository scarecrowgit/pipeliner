//! Context management: compaction by threshold, on demand and after a request that did not
//! fit, and long tool output.

use llm::{LlmError, Reasoning, Reply};
use model::{Message, TodoItem, TodoStatus, Usage};
use serde_json::json;
use tools::ReadFile;

use super::*;
use crate::history::summary_note;
use crate::{Host, StopCause};

fn chat(turns: usize) -> Vec<Message> {
    (1..=turns).flat_map(|n| [Message::user(format!("Question {n}")), Message::assistant(format!("Answer {n}"))]).collect()
}

#[test]
fn a_request_that_does_not_fit_is_compacted_once_and_sent_again() {
    let fx = Fixture::new();
    fx.host.seed(fx.session(), chat(3));
    fx.llm.push(Reply::error(LlmError::ContextTooLong("215000 tokens > 200000".into())));
    fx.llm.push(Reply::text("## Goal\nAnswer questions."));
    fx.llm.push(Reply::text("Answer 4"));
    let (outcome, log) = fx.run(say("Question 4"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "Answer 4");
    assert_eq!(outcome.totals.steps, 1);
    let requests = fx.requests();
    assert_eq!(requests.len(), 3);

    // The summary call: no tools, one message holding the old turns as text.
    let summary_call = &requests[1];
    assert!(summary_call.tools.is_empty());
    assert_eq!(summary_call.reasoning, Reasoning::Low);
    assert!(summary_call.system.starts_with("You compress the conversation of a coding agent"));
    assert_eq!(summary_call.messages.len(), 1);
    let asked = summary_call.messages[0].plain_text();
    assert!(asked.starts_with("<conversation>\n### User\nQuestion 1\n\n### Assistant\nAnswer 1\n\n### User\nQuestion 2\n\n### Assistant\nAnswer 2\n</conversation>\n\nWrite a summary"), "{asked}");
    assert!(asked.contains("## Goal\n") && asked.contains("## Next steps\n") && asked.contains("## Facts to keep\n"));

    // The retried request: the summary, then the three newest messages as they were.
    let summary = "## Goal\nAnswer questions.";
    assert_eq!(
        requests[2].messages,
        vec![
            user(vec![text(&summary_note(summary)), text("Question 3")]),
            Message::assistant("Answer 3"),
            Message::user("Question 4"),
        ]
    );
    assert_eq!(requests[2].system, requests[0].system);
    assert_eq!(fx.host.summary(fx.session()).expect("summary").as_deref(), Some(summary));
    let stored = fx.host.stored(fx.session());
    assert_eq!(stored.iter().map(|m| m.compacted).collect::<Vec<_>>(), [true, true, true, true, false, false, false, false]);
    assert_eq!(kinds(&log, fx.session()), ["message", "turn_started", "compaction_started", "compacted", "turn_started", "text", "text", "message", "turn_finished"]);
    assert!(log.events.iter().any(|(_, e)| matches!(e, AgentEvent::Compacted { summarised: 4, .. })));

    // The next run starts from the summary.
    fx.llm.push(Reply::text("Answer 5"));
    fx.run(say("Question 5"));
    let last = fx.requests().pop().expect("request");
    assert_eq!(last.messages.len(), 5);
    assert_eq!(last.messages[0], requests[2].messages[0]);
}

#[test]
fn a_request_that_still_does_not_fit_after_compaction_fails_clearly() {
    let fx = Fixture::new();
    fx.host.seed(fx.session(), chat(3));
    fx.llm.push(Reply::error(LlmError::ContextTooLong("too long".into())));
    fx.llm.push(Reply::text("Summary."));
    fx.llm.push(Reply::error(LlmError::ContextTooLong("still too long".into())));
    let (outcome, _) = fx.run(say("Question 4"));
    assert_eq!(
        outcome.stop,
        StopCause::Failed { error: "The conversation does not fit the model's context even after compaction: still too long".into() }
    );
    assert_eq!(fx.llm.call_count(), 3);
}

#[test]
fn compaction_at_the_threshold_keeps_a_call_with_its_result_and_the_task_list() {
    let mut fx = Fixture::new();
    fx.spec.limits.compact_at = 0.5;
    fx.spec.limits.keep_recent = 1;
    fx.write("a.txt", "one\n");
    let todo = json!({"items": [{"text": "Read a.txt", "status": "in_progress"}, {"text": "Report", "status": "pending"}]});
    let read = json!({"path": "a.txt"});
    let small = Usage { input: 10_000, output: 100, ..Default::default() };
    let big = Usage { input: 100_000, output: 100, ..Default::default() };
    fx.llm.push(Reply::tool("t1", "todo_write", todo).with_usage(small));
    fx.llm.push(Reply::tool("t2", "read_file", read.clone()).with_usage(big));
    fx.llm.push(Reply::text("## Goal\nRead a.txt and report.").with_usage(small));
    fx.llm.push(Reply::text("It says one.").with_usage(small));
    let (outcome, log) = fx.run(say("Read a.txt and report"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.totals.steps, 3, "the summary call is not a step");
    let requests = fx.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].tools.is_empty(), "the third call is the summary");

    // Keeping one message would have kept t2's result without its call: both stay.
    let read_output = fx.twin_output(&ReadFile, &[("a.txt", "one\n")], read.clone());
    let summary = "## Goal\nRead a.txt and report.\n\n## Task list at this point\n[~] Read a.txt\n[ ] Report";
    assert_eq!(
        requests[3].messages,
        vec![
            Message::user(summary_note(summary)),
            assistant(vec![tool_use("t2", "read_file", read)]),
            user(vec![result("t2", &read_output.content)]),
        ]
    );
    assert_paired(&requests[3].messages[1..]);
    let stored = fx.host.stored(fx.session());
    assert_eq!(stored.iter().map(|m| m.compacted).collect::<Vec<_>>(), [true, true, true, false, false, false]);
    assert!(log.events.iter().any(|(_, e)| matches!(e, AgentEvent::Compacted { summarised: 3, .. })));
    // The summary call is paid for like any other.
    assert_eq!(fx.host.ledger().len(), 4);
    // The list itself is untouched.
    assert_eq!(
        fx.host.todos(fx.session()).expect("todos"),
        vec![
            TodoItem { text: "Read a.txt".into(), status: TodoStatus::InProgress },
            TodoItem { text: "Report".into(), status: TodoStatus::Pending },
        ]
    );
}

#[test]
fn a_session_that_was_full_last_time_compacts_before_its_first_request() {
    let mut fx = Fixture::new();
    fx.spec.limits.keep_recent = 2;
    fx.host.seed(fx.session(), chat(3));
    // The last answer of the earlier run reported a nearly full window.
    let full = model::StoredMessage {
        seq: 0,
        at: 0,
        message: Message::assistant("Answer 4"),
        compacted: false,
        model: None,
        usage: Usage { input: 120_000, output: 500, ..Default::default() },
        cost_usd: 0.0,
    };
    fx.host.seed(fx.session(), [Message::user("Question 4")]);
    fx.host.append(fx.session(), full).expect("append");
    fx.llm.push(Reply::text("Summary so far."));
    fx.llm.push(Reply::text("Answer 5"));
    fx.run(say("Question 5"));

    let requests = fx.requests();
    assert!(requests[0].tools.is_empty(), "the summary came first");
    assert_eq!(
        requests[1].messages,
        vec![user(vec![text(&summary_note("Summary so far."))]), Message::assistant("Answer 4"), Message::user("Question 5")]
    );
}

#[test]
fn compact_on_demand_summarises_without_a_conversation_turn() {
    let fx = Fixture::new();
    let (outcome, log) = fx.run(Input::Compact);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.llm.call_count(), 0);
    assert!(log.events.iter().any(|(_, e)| *e == AgentEvent::Notice { text: "There is nothing to compact yet.".into() }));

    fx.host.seed(fx.session(), chat(2));
    fx.llm.push(Reply::text("Two questions were answered."));
    let (outcome, log) = fx.run(Input::Compact);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.llm.call_count(), 1);
    assert_eq!(kinds(&log, fx.session()), ["compaction_started", "compacted", "turn_finished"]);
    assert_eq!(fx.transcript(), vec![Message::user("Question 2"), Message::assistant("Answer 2")]);
    assert_eq!(fx.host.summary(fx.session()).expect("summary").as_deref(), Some("Two questions were answered."));

    // A second summary is told what the first one said.
    fx.host.seed(fx.session(), chat(1));
    fx.llm.push(Reply::text("Three questions were answered."));
    fx.run(Input::Compact);
    let asked = fx.requests().pop().expect("request").messages[0].plain_text();
    assert!(asked.starts_with("<conversation>\n### Summary of what came before\nTwo questions were answered.\n\n### User\nQuestion 2"), "{asked}");
}

#[test]
fn a_failed_summary_leaves_the_conversation_as_it_was() {
    let fx = Fixture::new();
    fx.host.seed(fx.session(), chat(2));
    fx.llm.push(Reply::thinking("no words"));
    let (outcome, _) = fx.run(Input::Compact);
    assert_eq!(
        outcome.stop,
        StopCause::Failed { error: "The model returned an empty summary; the conversation was left as it is.".into() }
    );
    assert_eq!(fx.transcript(), chat(2));
    assert_eq!(fx.host.summary(fx.session()).expect("summary"), None);
}

#[test]
fn long_tool_output_is_cut_in_the_message_and_kept_whole_as_a_blob() {
    let mut fx = Fixture::new();
    fx.spec.limits.tool_output_max = 300;
    let body: String = (0..200).map(|n| format!("line {n}\n")).collect();
    fx.write("big.txt", &body);
    let input = json!({"path": "big.txt"});
    fx.llm.push(Reply::tool("t1", "read_file", input.clone()).and_tool("t2", "list_dir", json!({"path": "."})));
    fx.llm.push(Reply::text("Long file."));
    fx.run(say("Read it"));

    let full = fx.twin_output(&ReadFile, &[("big.txt", body.as_str())], input).content;
    assert!(full.len() > 1_000);
    let transcript = fx.transcript();
    let (content, is_error) = result_of(&transcript, "t1");
    assert!(!is_error);
    assert!(content.len() < 700, "{}", content.len());
    assert!(content.starts_with(&full[..200]));
    assert!(content.ends_with(&full[full.len() - 100..]));
    assert!(content.contains("only its start and end are shown; the full output is kept as blob blob-1."), "{content}");
    assert_eq!(fx.host.blob(&model::BlobId::new("blob-1")), Some(full.into_bytes()));
    // Short output is left alone.
    assert!(!result_of(&transcript, "t2").0.contains("left out"));
    assert_eq!(fx.requests()[1].messages, transcript[..3]);
}

#[test]
fn a_cancel_during_a_compaction_leaves_the_conversation_as_it_was() {
    let fx = Fixture::new();
    fx.host.seed(fx.session(), chat(3));
    fx.llm.push(Reply::error(LlmError::ContextTooLong("too long".into())));
    fx.llm.push(Reply::text("Never written.").with_delay(std::time::Duration::from_secs(30)));
    let cancel = fx.cancel.clone();
    let mut log = EventLog::new();
    let mut sink = |session: &SessionId, event: AgentEvent| {
        if event == AgentEvent::CompactionStarted {
            cancel.cancel();
        }
        log.events.push((session.clone(), event));
    };
    let started = std::time::Instant::now();
    let outcome = fx.run_into(&AllowAll, say("Question 4"), &mut sink);

    assert_eq!(outcome.stop, StopCause::Cancelled);
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(kinds(&log, fx.session()), ["message", "turn_started", "compaction_started", "turn_finished"]);
    assert_eq!(fx.host.summary(fx.session()).expect("summary"), None);
    let mut expected = chat(3);
    expected.push(Message::user("Question 4"));
    assert_eq!(fx.transcript(), expected);

    let mut fx = fx;
    fx.cancel = CancelToken::new();
    fx.llm.push(Reply::text("Answer 4"));
    let (outcome, _) = fx.run(Input::Continue);
    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(fx.requests()[2].messages, expected);
}

#[test]
fn a_compaction_at_the_threshold_that_fails_is_skipped_and_the_run_goes_on() {
    let mut fx = Fixture::new();
    fx.spec.limits.compact_at = 0.5;
    fx.spec.limits.keep_recent = 1;
    let big = Usage { input: 100_000, output: 100, ..Default::default() };
    let todo = json!({"items": []});
    fx.llm.push(Reply::tool("t1", "todo_write", todo.clone()).with_usage(big));
    // The summary comes back empty.
    fx.llm.push(Reply::thinking("no words"));
    fx.llm.push(Reply::tool("t2", "todo_write", todo).with_usage(big));
    fx.llm.push(Reply::text("Done.").with_usage(big));
    let (outcome, log) = fx.run(say("Go"));

    assert_eq!(outcome.stop, StopCause::Finished);
    assert_eq!(outcome.text, "Done.");
    let requests = fx.requests();
    assert_eq!(requests.len(), 4, "the summary was asked for once, not before every step");
    assert!(requests[1].tools.is_empty());
    assert!(!requests[2].tools.is_empty() && !requests[3].tools.is_empty());
    // Nothing was replaced, and what is sent is still the whole conversation.
    assert_eq!(fx.host.summary(fx.session()).expect("summary"), None);
    assert!(fx.host.stored(fx.session()).iter().all(|m| !m.compacted));
    assert_eq!(requests[3].messages, fx.transcript()[..5]);
    assert_paired(&fx.transcript());
    let events = log.of(fx.session());
    assert!(events.iter().any(|e| matches!(e, AgentEvent::Compacted { summarised: 0, .. })));
    assert!(events.iter().any(|e| {
        **e == AgentEvent::Notice {
            text: "Compaction was skipped: The model returned an empty summary; the conversation was left as it is."
                .into(),
        }
    }));
}
