//! Anthropic Messages API (`POST /v1/messages`, SSE).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use anyhow::{Context as _, anyhow};
use serde_json::{Value, json};

use crate::http::{self, SHORT_CALL_TIMEOUT};
use crate::sse::{self, Decoder, SseEvent};
use crate::{
    ChatRequest, ContentBlock, ProviderConfig, Role, StopReason, StreamEvent, ToolSpec, Usage,
};

const API_VERSION: &str = "2023-06-01";

fn with_auth<B>(
    provider: &ProviderConfig,
    req: ureq::RequestBuilder<B>,
) -> ureq::RequestBuilder<B> {
    let req = req.header("anthropic-version", API_VERSION);
    match &provider.api_key {
        Some(key) => req.header("x-api-key", key),
        None => req,
    }
}

pub(crate) fn list_models(provider: &ProviderConfig) -> anyhow::Result<Vec<String>> {
    let what = format!("{}: list models", provider.id);
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    // Paginate (bounded, in case a server keeps saying `has_more`).
    for _ in 0..20 {
        let mut url = format!("{}/v1/models?limit=1000", provider.base_url);
        if let Some(after) = &after {
            url.push_str("&after_id=");
            url.push_str(after);
        }
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
            .read_json()
            .with_context(|| what.clone())?;
        let data = page.get("data").and_then(Value::as_array);
        models.extend(
            data.into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .map(String::from),
        );
        let last = page.get("last_id").and_then(Value::as_str);
        match (page.get("has_more").and_then(Value::as_bool), last) {
            (Some(true), Some(last)) if after.as_deref() != Some(last) => after = Some(last.into()),
            _ => break,
        }
    }
    Ok(models)
}

pub(crate) fn stream_chat(
    provider: &ProviderConfig,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(StreamEvent),
) -> anyhow::Result<()> {
    let what = format!("{}: messages", provider.id);
    let body = request_body(request);
    let url = format!("{}/v1/messages", provider.base_url);
    let req = with_auth(provider, http::agent().post(&url))
        .header("accept", "text/event-stream")
        .header("content-type", "application/json");
    let response = req.send_json(&body).with_context(|| what.clone())?;
    let response = http::check_status(&what, response)?;
    let reader = std::io::BufReader::new(response.into_body().into_reader());
    sse::drive(reader, &mut AnthropicDecoder::default(), cancel, on_event)
}

/// Serialize a [`ChatRequest`] into a Messages API request body (`stream: true`).
pub(crate) fn request_body(request: &ChatRequest) -> Value {
    let mut messages = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        // tool_result blocks must lead the user message.
        let (results, rest): (Vec<_>, Vec<_>) = message
            .content
            .iter()
            .partition(|b| matches!(b, ContentBlock::ToolResult { .. }));
        let blocks: Vec<Value> = results
            .into_iter()
            .chain(rest)
            .filter_map(block_json)
            .collect();
        if blocks.is_empty() {
            continue; // the API rejects empty content
        }
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": blocks }));
    }
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "messages": messages,
    });
    if let Some(system) = request.system.as_deref().filter(|s| !s.is_empty()) {
        body["system"] = json!(system);
    }
    if !request.tools.is_empty() {
        body["tools"] = request.tools.iter().map(tool_json).collect();
    }
    if let Some(t) = request.temperature {
        body["temperature"] = json!(t);
    }
    body
}

fn block_json(block: &ContentBlock) -> Option<Value> {
    Some(match block {
        ContentBlock::Text(text) if text.is_empty() => return None,
        ContentBlock::Text(text) => json!({ "type": "text", "text": text }),
        ContentBlock::Image { media_type, data } => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media_type, "data": data },
        }),
        ContentBlock::ToolUse { id, name, input } => {
            let input = if input.is_object() {
                input.clone()
            } else {
                json!({})
            };
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let mut v = json!({ "type": "tool_result", "tool_use_id": tool_use_id });
            if !content.is_empty() {
                v["content"] = json!(content);
            }
            if *is_error {
                v["is_error"] = json!(true);
            }
            v
        }
    })
}

fn tool_json(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        other => StopReason::Other(other.to_string()),
    }
}

enum Block {
    Tool {
        id: String,
        name: String,
        json: String,
    },
    Other,
}

#[derive(Default)]
pub(crate) struct AnthropicDecoder {
    usage: Usage,
    blocks: HashMap<u64, Block>,
    stop: Option<StopReason>,
    /// A tool call whose input JSON did not parse (fine if the turn hit `max_tokens`).
    bad_tool_input: Option<String>,
}

impl AnthropicDecoder {
    fn update_usage(&mut self, u: &Value) -> bool {
        let mut changed = false;
        let mut set = |field: &mut u64, key: &str| {
            if let Some(n) = u.get(key).and_then(Value::as_u64) {
                *field = n;
                changed = true;
            }
        };
        set(&mut self.usage.input_tokens, "input_tokens");
        set(&mut self.usage.output_tokens, "output_tokens");
        set(&mut self.usage.cache_read_tokens, "cache_read_input_tokens");
        set(
            &mut self.usage.cache_write_tokens,
            "cache_creation_input_tokens",
        );
        changed
    }

    fn finish(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()> {
        let stop = self.stop.take().unwrap_or(StopReason::EndTurn);
        if let Some(err) = self.bad_tool_input.take()
            && stop != StopReason::MaxTokens
        {
            return Err(anyhow!(err));
        }
        emit(StreamEvent::Stop(stop));
        Ok(())
    }
}

impl Decoder for AnthropicDecoder {
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<bool> {
        let v: Value = serde_json::from_str(&ev.data)
            .with_context(|| format!("invalid JSON in {:?} event", ev.event))?;
        let kind = v.get("type").and_then(Value::as_str).unwrap_or(&ev.event);
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "message_start" => {
                if self.update_usage(&v["message"]["usage"]) {
                    emit(StreamEvent::Usage(self.usage));
                }
            }
            "content_block_start" => {
                let block = &v["content_block"];
                let b = match block["type"].as_str() {
                    Some("tool_use") => {
                        let id = block["id"].as_str().unwrap_or_default().to_string();
                        let name = block["name"].as_str().unwrap_or_default().to_string();
                        emit(StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        Block::Tool {
                            id,
                            name,
                            json: String::new(),
                        }
                    }
                    Some("text") => {
                        if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                            emit(StreamEvent::TextDelta(t.to_string()));
                        }
                        Block::Other
                    }
                    Some("thinking") => {
                        if let Some(t) = block["thinking"].as_str().filter(|t| !t.is_empty()) {
                            emit(StreamEvent::ThinkingDelta(t.to_string()));
                        }
                        Block::Other
                    }
                    _ => Block::Other,
                };
                self.blocks.insert(index, b);
            }
            "content_block_delta" => {
                let delta = &v["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        if let Some(t) = delta["text"].as_str() {
                            emit(StreamEvent::TextDelta(t.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(t) = delta["thinking"].as_str() {
                            emit(StreamEvent::ThinkingDelta(t.to_string()));
                        }
                    }
                    Some("input_json_delta") => {
                        let part = delta["partial_json"].as_str().unwrap_or_default();
                        if let Some(Block::Tool { id, json, .. }) = self.blocks.get_mut(&index) {
                            json.push_str(part);
                            if !part.is_empty() {
                                emit(StreamEvent::ToolUseInputDelta {
                                    id: id.clone(),
                                    partial_json: part.to_string(),
                                });
                            }
                        }
                    }
                    _ => {} // signature_delta, citations_delta, ...
                }
            }
            "content_block_stop" => {
                if let Some(Block::Tool { id, name, json }) = self.blocks.remove(&index) {
                    let parsed = if json.trim().is_empty() {
                        Ok(Value::Object(Default::default()))
                    } else {
                        serde_json::from_str::<Value>(&json)
                    };
                    match parsed {
                        Ok(input) => emit(StreamEvent::ToolUse { id, name, input }),
                        Err(e) => {
                            self.bad_tool_input = Some(format!(
                                "tool call {name} ({id}) has invalid input JSON: {e}"
                            ))
                        }
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(map_stop_reason(reason));
                }
                if self.update_usage(&v["usage"]) {
                    emit(StreamEvent::Usage(self.usage));
                }
            }
            "message_stop" => {
                self.finish(emit)?;
                return Ok(true);
            }
            "error" => {
                let msg = http::json_error_message(&v).unwrap_or_else(|| ev.data.clone());
                anyhow::bail!("stream error: {msg}");
            }
            _ => {} // ping, unknown future events
        }
        Ok(false)
    }

    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> anyhow::Result<()> {
        if self.stop.is_some() {
            return self.finish(emit);
        }
        anyhow::bail!("stream ended before message_stop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::decode_lines;
    use crate::{Message, ToolSpec};

    fn decode(lines: &[&str]) -> anyhow::Result<Vec<StreamEvent>> {
        decode_lines(&mut AnthropicDecoder::default(), lines)
    }

    #[test]
    fn plain_text_with_usage() {
        let events = decode(&[
            "event: message_start",
            r#"data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-5-5","stop_reason":null,"usage":{"input_tokens":25,"cache_creation_input_tokens":3,"cache_read_input_tokens":100,"output_tokens":1}}}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "",
            "event: ping",
            r#"data: {"type": "ping"}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":", world"}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}"#,
            "",
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
            "",
        ])
        .unwrap();
        let start = Usage {
            input_tokens: 25,
            output_tokens: 1,
            cache_read_tokens: 100,
            cache_write_tokens: 3,
        };
        assert_eq!(
            events,
            vec![
                StreamEvent::Usage(start),
                StreamEvent::TextDelta("Hello".into()),
                StreamEvent::TextDelta(", world".into()),
                StreamEvent::Usage(Usage {
                    output_tokens: 15,
                    ..start
                }),
                StreamEvent::Stop(StopReason::EndTurn),
            ]
        );
    }

    #[test]
    fn thinking_then_tool_call_with_split_json() {
        let events = decode(&[
            "event: message_start",
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":1}}}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Need weather."}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\": \"San Fra"}}"#,
            "",
            "event: content_block_delta",
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"ncisco\", \"unit\": \"c\"}"}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":1}"#,
            "",
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_02","name":"now","input":{}}}"#,
            "",
            "event: content_block_stop",
            r#"data: {"type":"content_block_stop","index":2}"#,
            "",
            "event: message_delta",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":89}}"#,
            "",
            "event: message_stop",
            r#"data: {"type":"message_stop"}"#,
        ])
        .unwrap();
        let usage = Usage {
            input_tokens: 10,
            output_tokens: 1,
            ..Default::default()
        };
        assert_eq!(
            events,
            vec![
                StreamEvent::Usage(usage),
                StreamEvent::ThinkingDelta("Need weather.".into()),
                StreamEvent::ToolUseStart {
                    id: "toolu_01".into(),
                    name: "get_weather".into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "toolu_01".into(),
                    partial_json: r#"{"location": "San Fra"#.into()
                },
                StreamEvent::ToolUseInputDelta {
                    id: "toolu_01".into(),
                    partial_json: r#"ncisco", "unit": "c"}"#.into()
                },
                StreamEvent::ToolUse {
                    id: "toolu_01".into(),
                    name: "get_weather".into(),
                    input: json!({"location": "San Francisco", "unit": "c"}),
                },
                StreamEvent::ToolUseStart {
                    id: "toolu_02".into(),
                    name: "now".into()
                },
                StreamEvent::ToolUse {
                    id: "toolu_02".into(),
                    name: "now".into(),
                    input: json!({})
                },
                StreamEvent::Usage(Usage {
                    output_tokens: 89,
                    ..usage
                }),
                StreamEvent::Stop(StopReason::ToolUse),
            ]
        );
    }

    #[test]
    fn error_event_is_an_error() {
        let err = decode(&[
            "event: message_start",
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":1}}}"#,
            "",
            "event: error",
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            "",
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("Overloaded (overloaded_error)"),
            "{err}"
        );
    }

    #[test]
    fn truncated_stream_is_an_error_but_max_tokens_is_a_stop() {
        let err = decode(&[
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            "",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
        ])
        .unwrap_err();
        assert!(err.to_string().contains("message_stop"));

        // Tool input cut off by max_tokens: no ToolUse, Stop(MaxTokens).
        let events = decode(&[
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}"#,
            "",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#,
            "",
            r#"data: {"type":"content_block_stop","index":0}"#,
            "",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":5}}"#,
            "",
            r#"data: {"type":"message_stop"}"#,
            "",
        ])
        .unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::ToolUse { .. }))
        );
        assert_eq!(
            events.last(),
            Some(&StreamEvent::Stop(StopReason::MaxTokens))
        );
    }

    #[test]
    fn request_body_images() {
        let request = ChatRequest {
            model: "m".into(),
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: vec![
                    ContentBlock::Image {
                        media_type: "image/jpeg".into(),
                        data: crate::encode_base64(b"ABC"),
                    },
                    ContentBlock::Text("what is this?".into()),
                ],
            }],
            tools: vec![],
            max_tokens: 10,
            temperature: None,
        };
        assert_eq!(
            request_body(&request)["messages"],
            json!([{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "QUJD"}},
                {"type": "text", "text": "what is this?"},
            ]}])
        );
    }

    #[test]
    fn request_body_tool_round_trip() {
        let request = ChatRequest {
            model: "claude-sonnet-5-5".into(),
            system: Some("Be brief.".into()),
            messages: vec![
                Message::user_text("Weather in Paris?"),
                Message {
                    role: Role::Assistant,
                    content: vec![
                        ContentBlock::Text("Checking.".into()),
                        ContentBlock::ToolUse {
                            id: "toolu_1".into(),
                            name: "get_weather".into(),
                            input: json!({"city": "Paris"}),
                        },
                    ],
                },
                Message {
                    role: Role::User,
                    content: vec![
                        ContentBlock::Text("also hurry".into()),
                        ContentBlock::ToolResult {
                            tool_use_id: "toolu_1".into(),
                            content: "18C".into(),
                            is_error: false,
                        },
                        ContentBlock::ToolResult {
                            tool_use_id: "toolu_2".into(),
                            content: "boom".into(),
                            is_error: true,
                        },
                    ],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text(String::new())],
                },
            ],
            tools: vec![ToolSpec {
                name: "get_weather".into(),
                description: "Get weather".into(),
                input_schema: json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            }],
            max_tokens: 1024,
            temperature: Some(0.5),
        };
        assert_eq!(
            request_body(&request),
            json!({
                "model": "claude-sonnet-5-5",
                "max_tokens": 1024,
                "stream": true,
                "system": "Be brief.",
                "temperature": 0.5,
                "messages": [
                    {"role": "user", "content": [{"type": "text", "text": "Weather in Paris?"}]},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "Checking."},
                        {"type": "tool_use", "id": "toolu_1", "name": "get_weather", "input": {"city": "Paris"}},
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "18C"},
                        {"type": "tool_result", "tool_use_id": "toolu_2", "content": "boom", "is_error": true},
                        {"type": "text", "text": "also hurry"},
                    ]},
                ],
                "tools": [{
                    "name": "get_weather",
                    "description": "Get weather",
                    "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}},
                }],
            })
        );
    }
}
