//! OpenAI-compatible Chat Completions (`POST {base}/chat/completions`, SSE). Also used for
//! OpenRouter, Ollama and other compatible gateways.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, anyhow};
use serde_json::{Value, json};

use crate::http::{self, SHORT_CALL_TIMEOUT};
use crate::sse::{self, Decoder, SseEvent};
use crate::{
    ChatRequest, ContentBlock, ProviderConfig, Role, StopReason, StreamEvent, ToolSpec, Usage,
};

fn with_auth<B>(
    provider: &ProviderConfig,
    req: ureq::RequestBuilder<B>,
) -> ureq::RequestBuilder<B> {
    match &provider.api_key {
        Some(key) => req.header("authorization", format!("Bearer {key}")),
        None => req,
    }
}

pub(crate) fn list_models(provider: &ProviderConfig) -> anyhow::Result<Vec<String>> {
    let what = format!("{}: list models", provider.id);
    let url = format!("{}/models", provider.base_url);
    let req = http::agent()
        .get(&url)
        .config()
        .timeout_global(Some(SHORT_CALL_TIMEOUT))
        .build();
    let response = with_auth(provider, req)
        .call()
        .with_context(|| what.clone())?;
    let mut response = http::check_status(&what, response)?;
    let page: Value = response
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024) // OpenRouter's list is large
        .read_json()
        .with_context(|| what.clone())?;
    let data = page.get("data").and_then(Value::as_array);
    Ok(data
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .map(String::from)
        .collect())
}

/// Which field carries the output-token limit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

pub(crate) fn stream_chat(
    provider: &ProviderConfig,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(StreamEvent),
) -> anyhow::Result<()> {
    let what = format!("{}: chat/completions", provider.id);
    let url = format!("{}/chat/completions", provider.base_url);
    // api.openai.com deprecated `max_tokens` (reasoning models reject it); others mostly
    // only know `max_tokens`. Retry once with the other field if the server complains.
    let mut field = if provider.base_url.contains("api.openai.com") {
        MaxTokensField::MaxCompletionTokens
    } else {
        MaxTokensField::MaxTokens
    };
    let mut retried = false;
    let response = loop {
        let body = request_body(request, field);
        let req = with_auth(provider, http::agent().post(&url))
            .header("accept", "text/event-stream")
            .header("content-type", "application/json");
        let response = req.send_json(&body).with_context(|| what.clone())?;
        match http::check_status(&what, response) {
            Ok(r) => break r,
            Err(e) if !retried && is_max_tokens_complaint(&e.to_string()) => {
                retried = true;
                field = match field {
                    MaxTokensField::MaxTokens => MaxTokensField::MaxCompletionTokens,
                    MaxTokensField::MaxCompletionTokens => MaxTokensField::MaxTokens,
                };
            }
            Err(e) => return Err(e),
        }
    };
    let reader = std::io::BufReader::new(response.into_body().into_reader());
    sse::drive(reader, &mut OpenAiDecoder::default(), cancel, on_event)
}

/// A 400 whose message mentions the token-limit parameter (e.g. "Unsupported parameter:
/// 'max_tokens' ... Use 'max_completion_tokens' instead").
fn is_max_tokens_complaint(err: &str) -> bool {
    err.contains("HTTP 400")
        && (err.contains("max_tokens") || err.contains("max_completion_tokens"))
}

/// Serialize a [`ChatRequest`] into a Chat Completions request body (`stream: true`).
pub(crate) fn request_body(request: &ChatRequest, max_field: MaxTokensField) -> Value {
    let mut messages = Vec::with_capacity(request.messages.len() + 1);
    if let Some(system) = request.system.as_deref().filter(|s| !s.is_empty()) {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &request.messages {
        let text = message
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) if !t.is_empty() => Some(t.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        match message.role {
            Role::User => {
                // Tool results must directly follow the assistant's tool_calls message.
                for block in &message.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
                        let content = if *is_error {
                            format!("Error: {content}")
                        } else {
                            content.clone()
                        };
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": content,
                        }));
                    }
                }
                let images: Vec<Value> = message
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Image { media_type, data } => Some(json!({
                            "type": "image_url",
                            "image_url": { "url": format!("data:{media_type};base64,{data}") },
                        })),
                        _ => None,
                    })
                    .collect();
                if !images.is_empty() {
                    let mut parts = Vec::with_capacity(images.len() + 1);
                    if !text.is_empty() {
                        parts.push(json!({ "type": "text", "text": text }));
                    }
                    parts.extend(images);
                    messages.push(json!({ "role": "user", "content": parts }));
                } else if !text.is_empty() {
                    messages.push(json!({ "role": "user", "content": text }));
                }
            }
            Role::Assistant => {
                let calls: Vec<Value> = message
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": input.to_string() },
                        })),
                        _ => None,
                    })
                    .collect();
                if calls.is_empty() {
                    if !text.is_empty() {
                        messages.push(json!({ "role": "assistant", "content": text }));
                    }
                } else {
                    let content = if text.is_empty() {
                        Value::Null
                    } else {
                        json!(text)
                    };
                    messages.push(json!({
                        "role": "assistant",
                        "content": content,
                        "tool_calls": calls,
                    }));
                }
            }
        }
    }
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    let key = match max_field {
        MaxTokensField::MaxTokens => "max_tokens",
        MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
    };
    body[key] = json!(request.max_tokens);
    if !request.tools.is_empty() {
        body["tools"] = request.tools.iter().map(tool_json).collect();
    }
    if let Some(t) = request.temperature {
        body["temperature"] = json!(t);
    }
    body
}

fn tool_json(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        },
    })
}

#[derive(Default)]
struct Call {
    id: Option<String>,
    name: String,
    args: String,
    started: bool,
    /// Bytes of `args` already sent as `ToolUseInputDelta`.
    sent: usize,
}

#[derive(Default)]
pub(crate) struct OpenAiDecoder {
    calls: BTreeMap<u64, Call>,
    finish: Option<String>,
    /// Number of `ToolUse` events emitted this turn.
    tool_uses: usize,
    bad_tool_input: Option<String>,
}

impl OpenAiDecoder {
    fn tool_call_delta(&mut self, position: usize, tc: &Value, emit: &mut dyn FnMut(StreamEvent)) {
        let index = tc
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(position as u64);
        let call = self.calls.entry(index).or_default();
        if let Some(id) = tc
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            && call.id.is_none()
        {
            call.id = Some(id.to_string());
        }
        let function = &tc["function"];
        if let Some(name) = function.get("name").and_then(Value::as_str)
            && !call.started
        {
            call.name.push_str(name);
        }
        match function.get("arguments") {
            Some(Value::String(s)) => call.args.push_str(s),
            Some(Value::Null) | None => {}
            Some(other) => call.args.push_str(&other.to_string()), // some servers send objects
        }
        if !call.started && !call.name.is_empty() {
            call.started = true;
            let id = call
                .id
                .get_or_insert_with(|| format!("call_{index}"))
                .clone();
            emit(StreamEvent::ToolUseStart {
                id,
                name: call.name.clone(),
            });
        }
        if call.started && call.args.len() > call.sent {
            let id = call.id.clone().unwrap_or_default();
            emit(StreamEvent::ToolUseInputDelta {
                id,
                partial_json: call.args[call.sent..].to_string(),
            });
            call.sent = call.args.len();
        }
    }

    /// Emit `ToolUse` for every accumulated call, in index order.
    fn flush_calls(&mut self, emit: &mut dyn FnMut(StreamEvent)) {
        for (index, call) in std::mem::take(&mut self.calls) {
            let id = call.id.unwrap_or_else(|| format!("call_{index}"));
            if !call.started {
                if call.name.is_empty() {
                    continue;
                }
                emit(StreamEvent::ToolUseStart {
                    id: id.clone(),
                    name: call.name.clone(),
                });
            }
            let parsed = if call.args.trim().is_empty() {
                Ok(Value::Object(Default::default()))
            } else {
                serde_json::from_str::<Value>(&call.args)
            };
            match parsed {
                Ok(input) => {
                    self.tool_uses += 1;
                    emit(StreamEvent::ToolUse {
                        id,
                        name: call.name,
                        input,
                    });
                }
                Err(e) => {
                    self.bad_tool_input = Some(format!(
                        "tool call {} ({id}) has invalid arguments JSON: {e}",
                        call.name
                    ))
                }
            }
        }
    }

    fn finish(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()> {
        self.flush_calls(emit);
        let stop = match self.finish.as_deref() {
            Some("tool_calls" | "function_call") => StopReason::ToolUse,
            // Some servers (older Ollama) report "stop" alongside tool calls.
            Some("stop") | None if self.tool_uses > 0 => StopReason::ToolUse,
            Some("stop" | "end_turn") | None => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some(other) => StopReason::Other(other.to_string()),
        };
        if let Some(err) = self.bad_tool_input.take()
            && stop != StopReason::MaxTokens
        {
            return Err(anyhow!(err));
        }
        emit(StreamEvent::Stop(stop));
        Ok(())
    }
}

fn parse_usage(u: &Value) -> Option<Usage> {
    if !u.is_object() {
        return None;
    }
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let prompt = n(&u["prompt_tokens"]);
    let cached = n(&u["prompt_tokens_details"]["cached_tokens"]).min(prompt);
    Some(Usage {
        // Match Anthropic semantics: input_tokens excludes cache reads.
        input_tokens: prompt - cached,
        output_tokens: n(&u["completion_tokens"]),
        cache_read_tokens: cached,
        cache_write_tokens: 0,
    })
}

impl Decoder for OpenAiDecoder {
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<bool> {
        let data = ev.data.trim();
        if data == "[DONE]" {
            self.finish(emit)?;
            return Ok(true);
        }
        if data.is_empty() {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data).context("invalid JSON in stream chunk")?;
        if v.get("error").is_some_and(|e| !e.is_null()) || ev.event == "error" {
            let msg = http::json_error_message(&v).unwrap_or_else(|| data.to_string());
            anyhow::bail!("stream error: {msg}");
        }
        let choices = v.get("choices").and_then(Value::as_array);
        let choice = choices.and_then(|cs| {
            cs.iter()
                .find(|c| c.get("index").and_then(Value::as_u64).unwrap_or(0) == 0)
        });
        if let Some(choice) = choice {
            let delta = &choice["delta"];
            for key in ["reasoning_content", "reasoning"] {
                if let Some(t) = delta
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    emit(StreamEvent::ThinkingDelta(t.to_string()));
                    break;
                }
            }
            if let Some(t) = delta
                .get("content")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                emit(StreamEvent::TextDelta(t.to_string()));
            }
            if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for (position, tc) in calls.iter().enumerate() {
                    self.tool_call_delta(position, tc, emit);
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish = Some(reason.to_string());
                self.flush_calls(emit);
            }
        }
        if let Some(usage) = v.get("usage").and_then(parse_usage) {
            emit(StreamEvent::Usage(usage));
        }
        Ok(false)
    }

    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()> {
        if self.finish.is_some() {
            return self.finish(emit);
        }
        anyhow::bail!("stream ended before a finish_reason or [DONE]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::decode_lines;
    use crate::{Message, ToolSpec};

    fn decode(lines: &[&str]) -> anyhow::Result<Vec<StreamEvent>> {
        decode_lines(&mut OpenAiDecoder::default(), lines)
    }

    #[test]
    fn plain_text_with_usage() {
        let events = decode(&[
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"","refusal":null},"finish_reason":null}],"usage":null}"#,
            "",
            r#"data: {"id":"c1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}"#,
            "",
            ": OPENROUTER PROCESSING",
            "",
            r#"data: {"id":"c1","choices":[{"index":0,"delta":{"content":" there"},"finish_reason":null}],"usage":null}"#,
            "",
            r#"data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":null}"#,
            "",
            r#"data: {"id":"c1","choices":[],"usage":{"prompt_tokens":120,"completion_tokens":7,"total_tokens":127,"prompt_tokens_details":{"cached_tokens":100}}}"#,
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
                StreamEvent::Usage(Usage {
                    input_tokens: 20,
                    output_tokens: 7,
                    cache_read_tokens: 100,
                    cache_write_tokens: 0
                }),
                StreamEvent::Stop(StopReason::EndTurn),
            ]
        );
    }

    #[test]
    fn reasoning_and_length() {
        let events = decode(&[
            r#"data: {"choices":[{"index":0,"delta":{"reasoning_content":"hmm"}}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"reasoning":"ok","content":"A"}}]}"#,
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
                StreamEvent::Stop(StopReason::MaxTokens),
            ]
        );
    }

    #[test]
    fn parallel_tool_calls_with_split_arguments() {
        let events = decode(&[
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\": \"Pa"}}]},"finish_reason":null}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"get_time","arguments":""}}]},"finish_reason":null}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ris\"}"}}]},"finish_reason":null}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"tz\":\"UTC\"}"}}]},"finish_reason":null}]}"#,
            "",
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            "",
            r#"data: {"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":30}}"#,
            "",
            "data: [DONE]",
        ])
        .unwrap();
        assert_eq!(
            events,
            vec![
                StreamEvent::ToolUseStart {
                    id: "call_a".into(),
                    name: "get_weather".into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "call_a".into(),
                    partial_json: r#"{"city": "Pa"#.into()
                },
                StreamEvent::ToolUseStart {
                    id: "call_b".into(),
                    name: "get_time".into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "call_a".into(),
                    partial_json: r#"ris"}"#.into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "call_b".into(),
                    partial_json: r#"{"tz":"UTC"}"#.into()
                },
                StreamEvent::ToolUse {
                    id: "call_a".into(),
                    name: "get_weather".into(),
                    input: json!({"city": "Paris"})
                },
                StreamEvent::ToolUse {
                    id: "call_b".into(),
                    name: "get_time".into(),
                    input: json!({"tz": "UTC"})
                },
                StreamEvent::Usage(Usage {
                    input_tokens: 50,
                    output_tokens: 30,
                    ..Default::default()
                }),
                StreamEvent::Stop(StopReason::ToolUse),
            ]
        );
    }

    #[test]
    fn whole_tool_call_in_one_chunk_without_index_or_id() {
        // Ollama-style: complete call in one chunk, finish_reason "stop", no [DONE].
        let events = decode(&[
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"function":{"name":"ls","arguments":"{}"}}]},"finish_reason":"stop"}]}"#,
            "",
        ])
        .unwrap();
        assert_eq!(
            events,
            vec![
                StreamEvent::ToolUseStart {
                    id: "call_0".into(),
                    name: "ls".into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "call_0".into(),
                    partial_json: "{}".into()
                },
                StreamEvent::ToolUse {
                    id: "call_0".into(),
                    name: "ls".into(),
                    input: json!({})
                },
                StreamEvent::Stop(StopReason::ToolUse),
            ]
        );
    }

    #[test]
    fn errors() {
        let err = decode(&[
            r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#,
            "",
            r#"data: {"error":{"message":"Rate limit exceeded","code":429}}"#,
            "",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("Rate limit exceeded"), "{err}");

        let err =
            decode(&[r#"data: {"choices":[{"index":0,"delta":{"content":"Hi"}}]}"#]).unwrap_err();
        assert!(err.to_string().contains("stream ended"), "{err}");
    }

    fn image_request(text: &str) -> ChatRequest {
        let mut content = vec![ContentBlock::Image {
            media_type: "image/png".into(),
            data: "QUJD".into(),
        }];
        if !text.is_empty() {
            content.insert(0, ContentBlock::Text(text.into()));
        }
        ChatRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message { role: Role::User, content }, Message::user_text("plain")],
            tools: vec![],
            max_tokens: 10,
            temperature: None,
        }
    }

    #[test]
    fn request_body_images() {
        let body = request_body(&image_request("look"), MaxTokensField::MaxTokens);
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}},
                ]},
                {"role": "user", "content": "plain"},
            ])
        );
        // Image-only message is still sent, without a text part.
        let body = request_body(&image_request(""), MaxTokensField::MaxTokens);
        assert_eq!(
            body["messages"][0],
            json!({"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}},
            ]})
        );
    }

    #[test]
    fn request_body_tool_round_trip() {
        let request = ChatRequest {
            model: "gpt-x".into(),
            system: Some("Be brief.".into()),
            messages: vec![
                Message::user_text("Weather in Paris and the time?"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::ToolUse {
                            id: "call_a".into(),
                            name: "get_weather".into(),
                            input: json!({"city": "Paris"}),
                        },
                        ContentBlock::ToolUse {
                            id: "call_b".into(),
                            name: "now".into(),
                            input: json!({}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        ContentBlock::ToolResult {
                            tool_use_id: "call_a".into(),
                            content: "18C".into(),
                            is_error: false,
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: "call_b".into(),
                            content: "clock broken".into(),
                            is_error: true,
                        },
                    ],
                },
                Message::assistant_text("18C; time unknown."),
            ],
            tools: vec![ToolSpec {
                name: "get_weather".into(),
                description: "Get weather".into(),
                input_schema: json!({"type": "object"}),
            }],
            max_tokens: 512,
            temperature: None,
        };
        assert_eq!(
            request_body(&request, MaxTokensField::MaxTokens),
            json!({
                "model": "gpt-x",
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": 512,
                "messages": [
                    {"role": "system", "content": "Be brief."},
                    {"role": "user", "content": "Weather in Paris and the time?"},
                    {"role": "assistant", "content": null, "tool_calls": [
                        {"id": "call_a", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}},
                        {"id": "call_b", "type": "function", "function": {"name": "now", "arguments": "{}"}},
                    ]},
                    {"role": "tool", "tool_call_id": "call_a", "content": "18C"},
                    {"role": "tool", "tool_call_id": "call_b", "content": "Error: clock broken"},
                    {"role": "assistant", "content": "18C; time unknown."},
                ],
                "tools": [{"type": "function", "function": {
                    "name": "get_weather", "description": "Get weather", "parameters": {"type": "object"},
                }}],
            })
        );
        let body = request_body(&request, MaxTokensField::MaxCompletionTokens);
        assert_eq!(body["max_completion_tokens"], json!(512));
        assert!(body.get("max_tokens").is_none());
    }
}
