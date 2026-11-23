use super::*;
use crate::testing::{blobs, conversation, decode};

fn run(lines: &[&str]) -> Result<Vec<StreamEvent>, LlmError> {
    decode(&mut AnthropicDecoder::default(), lines)
}

/// A transcript that ends a message with `reason`.
fn ending_with(reason: &str) -> Vec<String> {
    vec![
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":3,"output_tokens":1}}}"#.into(),
        String::new(),
        format!(r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{reason}"}},"usage":{{"output_tokens":2}}}}"#),
        String::new(),
        r#"data: {"type":"message_stop"}"#.into(),
    ]
}

#[test]
fn request_body_with_tools_images_thinking_and_cache_breakpoints() {
    let model = ModelRef::parse("nr/cc/claude-opus-5-5");
    let body = request_body(&model, &conversation(), &blobs());
    let cache = json!({"type": "ephemeral"});
    assert_eq!(
        body,
        json!({
            "model": "cc/claude-opus-5-5",
            "max_tokens": 1024,
            "stream": true,
            "system": [{"type": "text", "text": "Be brief.", "cache_control": cache}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "What is in these?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "UE5HIQ=="}},
                    {"type": "text", "text": "[image b.png is no longer available]"},
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "Look closer.", "signature": "sig-a"},
                    {"type": "text", "text": "Checking."},
                    {"type": "tool_use", "id": "call_a", "name": "zoom", "input": {"x": 1}},
                    {"type": "tool_use", "id": "call_b", "name": "now", "input": {}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_a", "content": "a cat"},
                    {"type": "tool_result", "tool_use_id": "call_b", "content": "clock broken", "is_error": true},
                    {"type": "text", "text": "hurry", "cache_control": cache},
                ]},
            ],
            "tools": [
                {"name": "zoom", "description": "Zoom in", "input_schema": {"type": "object"}},
                {"name": "now", "description": "Time", "input_schema": {"type": "object"}, "cache_control": cache},
            ],
        })
    );
}

#[test]
fn zero_max_tokens_asks_for_the_models_maximum_and_empty_parts_are_left_out() {
    let model = ModelRef::parse("anthropic/claude-opus-5-5");
    let request = ChatRequest {
        messages: vec![model::Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("done"),
                ContentBlock::Thinking { text: String::new(), signature: Some("s".into()) },
            ],
        }],
        ..Default::default()
    };
    let body = request_body(&model, &request, &crate::NoBlobs);
    assert_eq!(body["max_tokens"], json!(pricing::max_output(&model)));
    assert!(body.get("system").is_none() && body.get("tools").is_none() && body.get("temperature").is_none());
    // The breakpoint skips the trailing thinking block.
    assert_eq!(body["messages"][0]["content"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert!(body["messages"][0]["content"][1].get("cache_control").is_none());
}

#[test]
fn text_with_usage_and_cache_counts() {
    let events = run(&[
        "event: message_start",
        r#"data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":25,"cache_creation_input_tokens":3,"cache_read_input_tokens":100,"output_tokens":1}}}"#,
        "",
        "event: content_block_start",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        ": comment",
        "event: ping",
        r#"data: {"type": "ping"}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", world"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
        "",
    ])
    .unwrap();
    let start = Usage { input: 25, output: 1, cache_read: 100, cache_write: 3 };
    assert_eq!(
        events,
        vec![
            StreamEvent::Usage(start),
            StreamEvent::TextDelta("Hello".into()),
            StreamEvent::TextDelta(", world".into()),
            StreamEvent::Usage(Usage { output: 15, ..start }),
            StreamEvent::Stop(StopReason::EndTurn),
        ]
    );
}

#[test]
fn thinking_with_signature_then_parallel_tool_calls() {
    let events = run(&[
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":1}}}"#,
        "",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Need weather."}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"def"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"xyz"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"city\": \"Pa"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"ris\"}"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":2}"#,
        "",
        r#"data: {"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"toolu_02","name":"now","input":{}}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":3}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":89}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
    ])
    .unwrap();
    let usage = Usage { input: 10, output: 1, ..Default::default() };
    assert_eq!(
        events,
        vec![
            StreamEvent::Usage(usage),
            StreamEvent::ThinkingDelta("Need weather.".into()),
            StreamEvent::ThinkingSignature("abcdef".into()),
            StreamEvent::ToolUseStart { id: "toolu_01".into(), name: "get_weather".into() },
            StreamEvent::ToolUseInputDelta { id: "toolu_01".into(), json: r#"{"city": "Pa"#.into() },
            StreamEvent::ToolUseInputDelta { id: "toolu_01".into(), json: r#"ris"}"#.into() },
            StreamEvent::ToolUse { id: "toolu_01".into(), name: "get_weather".into(), input: json!({"city": "Paris"}), error: None },
            StreamEvent::ToolUseStart { id: "toolu_02".into(), name: "now".into() },
            StreamEvent::ToolUse { id: "toolu_02".into(), name: "now".into(), input: json!({}), error: None },
            StreamEvent::Usage(Usage { output: 89, ..usage }),
            StreamEvent::Stop(StopReason::ToolUse),
        ]
    );
}

#[test]
fn omitted_thinking_still_delivers_its_signature() {
    let events = run(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":"s1"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
    ])
    .unwrap();
    assert_eq!(events, vec![StreamEvent::ThinkingSignature("s1".into()), StreamEvent::Stop(StopReason::EndTurn)]);
}

#[test]
fn every_stop_reason() {
    for (wire, stop) in [
        ("end_turn", StopReason::EndTurn),
        ("stop_sequence", StopReason::EndTurn),
        ("tool_use", StopReason::ToolUse),
        ("max_tokens", StopReason::MaxTokens),
        ("model_context_window_exceeded", StopReason::MaxTokens),
        ("refusal", StopReason::Other),
        ("pause_turn", StopReason::Other),
    ] {
        let lines = ending_with(wire);
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let events = run(&lines).unwrap();
        assert_eq!(events.last(), Some(&StreamEvent::Stop(stop)), "{wire}");
    }
}

#[test]
fn malformed_tool_input_becomes_an_empty_object() {
    let events = run(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
    ])
    .unwrap();
    assert!(events.contains(&StreamEvent::ToolUseInputDelta { id: "t".into(), json: r#"{"a":"#.into() }));
    let tool_use = events.iter().find(|e| matches!(e, StreamEvent::ToolUse { .. })).unwrap();
    assert!(
        matches!(tool_use, StreamEvent::ToolUse { id, input, error: Some(error), .. }
            if id == "t" && *input == json!({}) && error.contains("not valid JSON")),
        "{tool_use:?}"
    );
    assert_eq!(events.last(), Some(&StreamEvent::Stop(StopReason::MaxTokens)));
}

#[test]
fn error_events_and_truncated_streams_fail() {
    let err = run(&[
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":1}}}"#,
        "",
        "event: error",
        r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        "",
    ])
    .unwrap_err();
    assert_eq!(err, LlmError::Overloaded("Overloaded (overloaded_error)".into()));

    let err = run(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
    ])
    .unwrap_err();
    assert!(matches!(err, LlmError::Network(_)), "{err:?}");

    let err = run(&["data: {not json", ""]).unwrap_err();
    assert!(matches!(err, LlmError::Other(_)), "{err:?}");
}

#[test]
fn a_stream_cut_after_the_stop_reason_still_stops() {
    let events = run(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        "",
    ])
    .unwrap();
    assert_eq!(events[events.len() - 2], StreamEvent::ToolUse { id: "t".into(), name: "x".into(), input: json!({}), error: None });
    assert_eq!(events.last(), Some(&StreamEvent::Stop(StopReason::ToolUse)));
}

/// The reasoning and sampling parts of a request for `model` at `reasoning`.
fn settings(model: &str, reasoning: Reasoning, max_tokens: u32) -> Value {
    let request = ChatRequest {
        messages: vec![model::Message::user("hi")],
        max_tokens,
        temperature: Some(0.2),
        reasoning,
        ..Default::default()
    };
    let body = request_body(&ModelRef::parse(model), &request, &crate::NoBlobs);
    json!({
        "thinking": body.get("thinking"),
        "output_config": body.get("output_config"),
        "temperature": body.get("temperature"),
    })
}

#[test]
fn reasoning_levels_on_a_model_that_always_thinks() {
    let opus = "anthropic/claude-opus-5-5";
    let summarized = json!({"type": "adaptive", "display": "summarized"});
    assert_eq!(settings(opus, Reasoning::Default, 1000), json!({"thinking": null, "output_config": null, "temperature": null}));
    assert_eq!(
        settings(opus, Reasoning::Off, 1000),
        json!({"thinking": null, "output_config": {"effort": "low"}, "temperature": null})
    );
    for (level, effort) in [(Reasoning::Low, "low"), (Reasoning::Medium, "medium"), (Reasoning::High, "high")] {
        assert_eq!(
            settings(opus, level, 1000),
            json!({"thinking": summarized, "output_config": {"effort": effort}, "temperature": null}),
            "{level:?}"
        );
    }
}

#[test]
fn reasoning_off_switches_and_temperature_per_model() {
    // Sonnet 5.5 turns thinking off with between_tools; Sonnet 5 with disabled.
    assert_eq!(settings("a/claude-sonnet-5-5", Reasoning::Off, 1000)["thinking"], json!({"type": "between_tools"}));
    assert_eq!(settings("a/claude-sonnet-5", Reasoning::Off, 1000)["thinking"], json!({"type": "disabled"}));
    // Sonnet 4.6 summarises by default and keeps temperature unless it thinks.
    assert_eq!(
        settings("a/claude-sonnet-4-6", Reasoning::Off, 1000),
        json!({"thinking": {"type": "disabled"}, "output_config": null, "temperature": 0.2f32})
    );
    assert_eq!(
        settings("a/claude-sonnet-4-6", Reasoning::High, 1000),
        json!({"thinking": {"type": "adaptive"}, "output_config": {"effort": "high"}, "temperature": null})
    );
    assert_eq!(settings("a/claude-sonnet-4-6", Reasoning::Default, 1000)["temperature"], json!(0.2f32));
}

#[test]
fn budgeted_thinking_fits_under_max_tokens() {
    let haiku = "a/claude-haiku-4-5";
    assert_eq!(
        settings(haiku, Reasoning::Medium, 64_000),
        json!({"thinking": {"type": "enabled", "budget_tokens": 16_000}, "output_config": null, "temperature": null})
    );
    assert_eq!(settings(haiku, Reasoning::High, 8_000)["thinking"], json!({"type": "enabled", "budget_tokens": 7_999}));
    // Too small for the minimum budget, and Off: no thinking, temperature kept.
    assert_eq!(settings(haiku, Reasoning::High, 1_000)["thinking"], Value::Null);
    assert_eq!(settings(haiku, Reasoning::Off, 64_000), json!({"thinking": null, "output_config": null, "temperature": 0.2f32}));
}

#[test]
fn models_without_thinking_ignore_reasoning() {
    for level in [Reasoning::Off, Reasoning::Low, Reasoning::High] {
        assert_eq!(
            settings("a/claude-3-5-haiku", level, 1000),
            json!({"thinking": null, "output_config": null, "temperature": 0.2f32}),
            "{level:?}"
        );
    }
}
