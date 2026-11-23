use super::*;
use crate::NoBlobs;
use model::Message;
use serde_json::json;
use std::time::Instant;

fn run(llm: &ScriptedLlm, request: &ChatRequest, cancel: &CancelToken) -> (Result<(), LlmError>, Vec<StreamEvent>) {
    let mut events = Vec::new();
    let model = ModelRef::parse("scripted/scripted-model");
    let result = llm.stream(&model, request, &NoBlobs, cancel, &mut |e| events.push(e));
    (result, events)
}

fn text_of(events: &[StreamEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn plays_replies_in_order_and_records_requests() {
    let llm = ScriptedLlm::new()
        .reply(Reply::text("Hello there, friend").with_usage(Usage { input: 10, output: 3, ..Default::default() }))
        .reply(Reply::text("Second"));
    let request = ChatRequest { messages: vec![Message::user("hi")], ..Default::default() };
    let (result, events) = run(&llm, &request, &CancelToken::new());
    result.unwrap();
    let deltas = events.iter().filter(|e| matches!(e, StreamEvent::TextDelta(_))).count();
    assert_eq!(deltas, 3);
    assert_eq!(text_of(&events), "Hello there, friend");
    assert_eq!(events[events.len() - 2], StreamEvent::Usage(Usage { input: 10, output: 3, ..Default::default() }));
    assert_eq!(events.last(), Some(&StreamEvent::Stop(StopReason::EndTurn)));

    let (_, events) = run(&llm, &ChatRequest::default(), &CancelToken::new());
    assert_eq!(text_of(&events), "Second");
    assert_eq!(llm.call_count(), 2);
    assert_eq!(llm.remaining(), 0);
    assert_eq!(llm.requests()[0].messages, vec![Message::user("hi")]);
    assert_eq!(llm.calls()[1].0, ModelRef::new("scripted", "scripted-model"));
    assert!(llm.last_request().unwrap().messages.is_empty());
}

#[test]
fn tools_thinking_and_default_stop() {
    let llm = ScriptedLlm::new().reply(
        Reply::new()
            .and_signed_thinking("Need the file.", "sig-1")
            .and_tool("t1", "read_file", json!({"path": "a.rs"}))
            .and_raw_tool("t2", "bash", r#"{"command": "ls"#),
    );
    let (result, events) = run(&llm, &ChatRequest::default(), &CancelToken::new());
    result.unwrap();
    assert!(events.contains(&StreamEvent::ThinkingSignature("sig-1".into())));
    assert!(events.contains(&StreamEvent::ToolUse { id: "t1".into(), name: "read_file".into(), input: json!({"path": "a.rs"}), error: None }));
    let t2 = events.iter().find(|e| matches!(e, StreamEvent::ToolUse { id, .. } if id == "t2")).unwrap();
    assert!(
        matches!(t2, StreamEvent::ToolUse { input, error: Some(e), .. } if *input == json!({}) && e.contains("not valid JSON")),
        "{t2:?}"
    );
    let raw: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolUseInputDelta { id, json } if id == "t2" => Some(json.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(raw, r#"{"command": "ls"#);
    assert_eq!(events.last(), Some(&StreamEvent::Stop(StopReason::ToolUse)));
}

#[test]
fn errors_before_and_after_content() {
    let llm = ScriptedLlm::new()
        .reply(Reply::error(LlmError::Overloaded("busy".into())))
        .reply(Reply::text("partial answer").then_fail(LlmError::Network("reset".into())));
    let (result, events) = run(&llm, &ChatRequest::default(), &CancelToken::new());
    assert_eq!(result, Err(LlmError::Overloaded("busy".into())));
    assert!(events.is_empty());
    let (result, events) = run(&llm, &ChatRequest::default(), &CancelToken::new());
    assert_eq!(result, Err(LlmError::Network("reset".into())));
    assert_eq!(text_of(&events), "partial answer");
    assert!(!events.iter().any(|e| matches!(e, StreamEvent::Stop(_))));
}

#[test]
fn delay_honours_cancel() {
    let llm = ScriptedLlm::new().reply(Reply::text("late").with_delay(Duration::from_secs(30)));
    let cancel = CancelToken::new();
    let remote = cancel.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        remote.cancel();
    });
    let start = Instant::now();
    let (result, events) = run(&llm, &ChatRequest::default(), &cancel);
    assert_eq!(result, Err(LlmError::Cancelled));
    assert!(events.is_empty());
    assert!(start.elapsed() < Duration::from_secs(5));
    handle.join().unwrap();
}

#[test]
fn cancelling_from_a_callback_stops_the_stream() {
    let llm = ScriptedLlm::new().reply(Reply::text("one two three four"));
    let cancel = CancelToken::new();
    let mut events = Vec::new();
    let result = llm.stream(&ModelRef::parse("x/y"), &ChatRequest::default(), &NoBlobs, &cancel, &mut |e| {
        events.push(e);
        cancel.cancel();
    });
    assert_eq!(result, Err(LlmError::Cancelled));
    assert_eq!(events.len(), 1);
}

#[test]
#[should_panic(expected = "has no scripted reply")]
fn an_empty_queue_fails_loudly() {
    let llm = ScriptedLlm::new();
    let _ = run(&llm, &ChatRequest { messages: vec![Message::user("again?")], ..Default::default() }, &CancelToken::new());
}

#[test]
fn providers_and_models() {
    let llm = ScriptedLlm::new();
    assert_eq!(llm.default_model(), Some(ModelRef::new("scripted", "scripted-model")));
    assert_eq!(llm.list_models("scripted").unwrap(), vec!["scripted-model".to_string()]);
    assert!(matches!(llm.list_models("nope"), Err(LlmError::UnknownModel(_))));
    let llm = llm.with_default_model(None).with_providers(vec![]);
    assert_eq!(llm.default_model(), None);
    assert!(llm.providers().is_empty());
}
