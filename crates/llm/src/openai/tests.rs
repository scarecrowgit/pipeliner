use super::*;
use crate::testing::{blobs, conversation, decode};

const STANDARD: Wire = Wire { max_tokens: MaxTokensField::MaxTokens, effort: EffortField::ReasoningEffort };

fn run(lines: &[&str]) -> Result<Vec<StreamEvent>, LlmError> {
    decode(&mut OpenAiDecoder::default(), lines)
}

#[test]
fn request_body_with_tools_images_and_dropped_thinking() {
    let model = ModelRef::parse("openrouter/openai/gpt-5");
    let body = request_body(&model, &conversation(), &blobs(), STANDARD);
    assert_eq!(
        body,
        json!({
            "model": "openai/gpt-5",
            "stream": true,
            "stream_options": {"include_usage": true},
            "max_tokens": 1024,
            "messages": [
                {"role": "system", "content": "Be brief."},
                {"role": "user", "content": [
                    {"type": "text", "text": "What is in these?"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,UE5HIQ=="}},
                    {"type": "text", "text": "[image b.png is no longer available]"},
                ]},
                {"role": "assistant", "content": "Checking.", "tool_calls": [
                    {"id": "call_a", "type": "function", "function": {"name": "zoom", "arguments": "{\"x\":1}"}},
                    {"id": "call_b", "type": "function", "function": {"name": "now", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": "call_a", "content": "a cat"},
                {"role": "tool", "tool_call_id": "call_b", "content": "Error: clock broken"},
                {"role": "user", "content": "hurry"},
            ],
            "tools": [
                {"type": "function", "function": {"name": "zoom", "description": "Zoom in", "parameters": {"type": "object"}}},
                {"type": "function", "function": {"name": "now", "description": "Time", "parameters": {"type": "object"}}},
            ],
        })
    );
}

#[test]
fn the_output_cap_field_can_be_swapped_or_left_out() {
    let model = ModelRef::parse("openai/gpt-5");
    let wire = Wire { max_tokens: MaxTokensField::MaxCompletionTokens, ..STANDARD };
    let mut body = request_body(&model, &conversation(), &blobs(), wire);
    assert_eq!(body["max_completion_tokens"], json!(1024));
    assert!(body.get("max_tokens").is_none());
    swap_max_tokens_field(&mut body, MaxTokensField::MaxCompletionTokens);
    assert_eq!(body["max_tokens"], json!(1024));
    assert!(body.get("max_completion_tokens").is_none());
    assert!(is_max_tokens_complaint("Unsupported parameter: 'max_tokens' is not supported with this model."));

    let request = ChatRequest { max_tokens: 0, ..conversation() };
    let body = request_body(&model, &request, &blobs(), STANDARD);
    assert!(body.get("max_tokens").is_none() && body.get("max_completion_tokens").is_none());
}

#[test]
fn text_with_usage_and_comments() {
    let events = run(&[
        r#"data: {"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","content":"","refusal":null},"finish_reason":null}],"usage":null}"#,
        "",
        r#"data: {"id":"c1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}"#,
        "",
        ": OPENROUTER PROCESSING",
        "",
        r#"data: {"id":"c1","choices":[{"index":0,"delta":{"content":" there"},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":null}"#,
        "",
        r#"data: {"id":"c1","choices":[],"usage":{"prompt_tokens":120,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":100,"cache_write_tokens":5}}}"#,
        "",
        "data: [DONE]",
        "",
    ])
    .unwrap();
    assert_eq!(
        events,
        vec![
            StreamEvent::TextDelta("Hello".into()),
            StreamEvent::TextDelta(" there".into()),
            StreamEvent::Usage(Usage { input: 15, output: 7, cache_read: 100, cache_write: 5 }),
            StreamEvent::Stop(StopReason::EndTurn),
        ]
    );
}

#[test]
fn reasoning_fields_of_different_gateways() {
    let events = run(&[
        r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"hmm"}}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"reasoning":"ok","content":"A"}}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"reasoning":"","content":"B"}}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
        "",
    ])
    .unwrap();
    assert_eq!(
        events,
        vec![
            StreamEvent::ThinkingDelta("hmm".into()),
            StreamEvent::ThinkingDelta("ok".into()),
            StreamEvent::TextDelta("A".into()),
            StreamEvent::TextDelta("B".into()),
            StreamEvent::Stop(StopReason::MaxTokens),
        ]
    );
}

#[test]
fn parallel_tool_calls_with_interleaved_arguments() {
    let events = run(&[
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\": \"Pa"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"get_time","arguments":""}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ris\"}"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"tz\":"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "",
        r#"data: {"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":30}}"#,
        "",
        "data: [DONE]",
    ])
    .unwrap();
    assert!(tool_input::parse(r#"{"tz":"#).1.is_some());
    assert_eq!(
        events,
        vec![
            StreamEvent::ToolUseStart { id: "call_a".into(), name: "get_weather".into() },
            StreamEvent::ToolUseInputDelta { id: "call_a".into(), json: r#"{"city": "Pa"#.into() },
            StreamEvent::ToolUseStart { id: "call_b".into(), name: "get_time".into() },
            StreamEvent::ToolUseInputDelta { id: "call_a".into(), json: r#"ris"}"#.into() },
            StreamEvent::ToolUseInputDelta { id: "call_b".into(), json: r#"{"tz":"#.into() },
            StreamEvent::ToolUse { id: "call_a".into(), name: "get_weather".into(), input: json!({"city": "Paris"}), error: None },
            // Broken arguments: an empty input and the reason.
            StreamEvent::ToolUse {
                id: "call_b".into(),
                name: "get_time".into(),
                input: json!({}),
                error: tool_input::parse(r#"{"tz":"#).1,
            },
            StreamEvent::Usage(Usage { input: 50, output: 30, ..Default::default() }),
            StreamEvent::Stop(StopReason::ToolUse),
        ]
    );
}

#[test]
fn whole_tool_call_in_one_chunk_without_index_or_id() {
    // Ollama style: the whole call in one chunk, finish_reason "stop", no [DONE].
    let events =
        run(&[r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"function":{"name":"ls","arguments":{"dir":"."}}}]},"finish_reason":"stop"}]}"#, ""])
            .unwrap();
    assert_eq!(
        events,
        vec![
            StreamEvent::ToolUseStart { id: "call_0".into(), name: "ls".into() },
            StreamEvent::ToolUseInputDelta { id: "call_0".into(), json: r#"{"dir":"."}"#.into() },
            StreamEvent::ToolUse { id: "call_0".into(), name: "ls".into(), input: json!({"dir": "."}), error: None },
            StreamEvent::Stop(StopReason::ToolUse),
        ]
    );
}

#[test]
fn every_finish_reason() {
    for (wire, stop) in [
        ("stop", StopReason::EndTurn),
        ("end_turn", StopReason::EndTurn),
        ("tool_calls", StopReason::ToolUse),
        ("function_call", StopReason::ToolUse),
        ("length", StopReason::MaxTokens),
        ("content_filter", StopReason::Other),
    ] {
        let chunk = format!(r#"data: {{"choices":[{{"index":0,"delta":{{}},"finish_reason":"{wire}"}}]}}"#);
        let events = run(&[&chunk, "", "data: [DONE]"]).unwrap();
        assert_eq!(events, vec![StreamEvent::Stop(stop)], "{wire}");
    }
    let err = run(&[r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"error"}]}"#, ""]).unwrap_err();
    assert!(matches!(err, LlmError::Other(_)));
}

#[test]
fn mid_stream_errors_and_truncation() {
    let err = run(&[
        r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#,
        "",
        r#"data: {"error":{"message":"Rate limit exceeded","code":429}}"#,
        "",
    ])
    .unwrap_err();
    assert!(matches!(&err, LlmError::RateLimited { message, .. } if message == "Rate limit exceeded"), "{err:?}");

    let err = run(&[r#"data: {"error":{"message":"Upstream overloaded","code":502},"choices":[{"index":0,"delta":{},"finish_reason":"error"}]}"#, ""])
        .unwrap_err();
    assert!(matches!(err, LlmError::Overloaded(_)), "{err:?}");

    let err = run(&["event: error", r#"data: {"message":"This model's maximum context length is 8192 tokens"}"#, ""]).unwrap_err();
    assert!(matches!(err, LlmError::ContextTooLong(_)), "{err:?}");

    let err = run(&[r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#]).unwrap_err();
    assert!(matches!(err, LlmError::Network(_)), "{err:?}");
}

/// The reasoning and sampling parts of a request for `model` at `reasoning`.
fn settings(model: &str, reasoning: Reasoning, effort: EffortField) -> Value {
    let request = ChatRequest {
        messages: vec![model::Message::user("hi")],
        temperature: Some(0.2),
        reasoning,
        ..Default::default()
    };
    let wire = Wire { effort, ..STANDARD };
    let body = request_body(&ModelRef::parse(model), &request, &crate::NoBlobs, wire);
    json!({
        "reasoning_effort": body.get("reasoning_effort"),
        "reasoning": body.get("reasoning"),
        "temperature": body.get("temperature"),
    })
}

#[test]
fn reasoning_levels_on_a_reasoning_model() {
    let plain = EffortField::ReasoningEffort;
    assert_eq!(
        settings("openai/gpt-5", Reasoning::Default, plain),
        json!({"reasoning_effort": null, "reasoning": null, "temperature": null})
    );
    for (level, effort) in [
        (Reasoning::Off, "minimal"),
        (Reasoning::Low, "low"),
        (Reasoning::Medium, "medium"),
        (Reasoning::High, "high"),
    ] {
        assert_eq!(
            settings("openai/gpt-5", level, plain),
            json!({"reasoning_effort": effort, "reasoning": null, "temperature": null}),
            "{level:?}"
        );
    }
    assert_eq!(settings("ollama/gpt-oss:20b", Reasoning::Off, plain)["reasoning_effort"], json!("low"));
    assert_eq!(settings("ollama/gpt-oss:20b", Reasoning::Off, plain)["temperature"], json!(0.2f32));
    // A Claude model behind a gateway: Off sends nothing where thinking is off by default.
    assert_eq!(settings("gw/claude-sonnet-4-6", Reasoning::Off, plain)["reasoning_effort"], Value::Null);
    assert_eq!(settings("gw/claude-sonnet-4-6", Reasoning::High, plain)["reasoning_effort"], json!("high"));
}

#[test]
fn openrouter_takes_a_reasoning_object() {
    assert_eq!(
        settings("openrouter/anthropic/claude-opus-5-5", Reasoning::Medium, EffortField::OpenRouter),
        json!({"reasoning_effort": null, "reasoning": {"effort": "medium"}, "temperature": null})
    );
}

#[test]
fn models_without_reasoning_ignore_it_and_keep_temperature() {
    for level in [Reasoning::Off, Reasoning::Low, Reasoning::High] {
        assert_eq!(
            settings("openai/gpt-4o", level, EffortField::ReasoningEffort),
            json!({"reasoning_effort": null, "reasoning": null, "temperature": 0.2f32}),
            "{level:?}"
        );
    }
}
