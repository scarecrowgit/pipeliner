//! Anthropic Messages: `POST {base}/v1/messages` streamed as SSE, `GET {base}/v1/models`.

use std::collections::HashMap;

use base::cancel::CancelToken;
use base64::Engine as _;
use model::{ContentBlock, ModelRef, Role, StopReason, ToolSpec, Usage};
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::http::{self, SHORT_CALL_TIMEOUT};
use crate::pricing::{Thinking, ThinkingOff};
use crate::sse::{self, Decoder, SseEvent};
use crate::{BlobSource, ChatRequest, LlmError, Reasoning, StreamEvent, pricing, tool_input};

const API_VERSION: &str = "2023-06-01";
const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];

fn with_auth<B>(provider: &ProviderConfig, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
    let req = req.header("anthropic-version", API_VERSION);
    match &provider.api_key {
        Some(key) => req.header("x-api-key", key),
        None => req,
    }
}

/// Every model id the provider lists, following pagination.
pub(crate) fn list_models(provider: &ProviderConfig) -> Result<Vec<String>, LlmError> {
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    // Bounded, in case a server keeps saying `has_more`.
    for _ in 0..20 {
        let mut url = format!("{}/v1/models?limit=1000", provider.base_url);
        if let Some(after) = &after {
            url.push_str("&after_id=");
            url.push_str(after);
        }
        let req = http::agent().get(&url).config().timeout_global(Some(SHORT_CALL_TIMEOUT)).build();
        let response = with_auth(provider, req).call().map_err(http::transport_error)?;
        let mut body = http::success_body(response)?;
        let page: Value = body.read_json().map_err(|e| LlmError::Network(format!("reading the model list failed: {e}")))?;
        let data = page.get("data").and_then(Value::as_array);
        models.extend(data.into_iter().flatten().filter_map(|m| m["id"].as_str()).map(String::from));
        let last = page.get("last_id").and_then(Value::as_str);
        match (page.get("has_more").and_then(Value::as_bool), last) {
            (Some(true), Some(last)) if after.as_deref() != Some(last) => after = Some(last.to_string()),
            _ => break,
        }
    }
    Ok(models)
}

/// Streams one response; see [`crate::LlmClient::stream`].
pub(crate) fn stream(
    provider: &ProviderConfig,
    model: &ModelRef,
    request: &ChatRequest,
    blobs: &dyn BlobSource,
    cancel: &CancelToken,
    on_event: &mut dyn FnMut(StreamEvent),
) -> Result<(), LlmError> {
    let body = http::to_bytes(&request_body(model, request, blobs))?;
    let url = format!("{}/v1/messages", provider.base_url);
    let provider = provider.clone();
    let mut lines = http::spawn_stream(cancel, move || {
        let req = with_auth(&provider, http::agent().post(&url))
            .header("accept", "text/event-stream")
            .header("content-type", "application/json");
        http::success_body(req.send(&body[..]).map_err(http::transport_error)?)
    })?;
    sse::drive(&mut lines, &mut AnthropicDecoder::default(), on_event)
}

/// The Messages request body, with cache breakpoints on the system prompt, the last tool
/// and the last message.
pub(crate) fn request_body(model: &ModelRef, request: &ChatRequest, blobs: &dyn BlobSource) -> Value {
    let mut messages: Vec<Value> = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        // Tool results must open a user message.
        let (results, rest): (Vec<_>, Vec<_>) =
            message.content.iter().partition(|b| matches!(b, ContentBlock::ToolResult { .. }));
        let blocks: Vec<Value> = results.into_iter().chain(rest).filter_map(|b| block_json(b, blobs)).collect();
        if blocks.is_empty() {
            continue; // the API rejects empty content
        }
        let role = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        messages.push(json!({ "role": role, "content": blocks }));
    }
    if let Some(last) = messages.last_mut().and_then(|m| m["content"].as_array_mut()) {
        // Thinking blocks cannot carry a breakpoint.
        if let Some(block) = last.iter_mut().rev().find(|b| b["type"] != "thinking") {
            block["cache_control"] = ephemeral();
        }
    }
    let max_tokens = if request.max_tokens == 0 { pricing::max_output(model) } else { request.max_tokens };
    let mut body = json!({
        "model": model.model,
        "max_tokens": max_tokens,
        "stream": true,
        "messages": messages,
    });
    if !request.system.is_empty() {
        body["system"] = json!([{ "type": "text", "text": request.system, "cache_control": ephemeral() }]);
    }
    if !request.tools.is_empty() {
        let mut tools: Vec<Value> = request.tools.iter().map(tool_json).collect();
        if let Some(last) = tools.last_mut() {
            last["cache_control"] = ephemeral();
        }
        body["tools"] = Value::Array(tools);
    }
    let thinks = apply_reasoning(&mut body, pricing::traits(model).thinking, request.reasoning, max_tokens);
    // Sampling settings are rejected by some models and by every model while it thinks.
    if let Some(t) = request.temperature
        && pricing::traits(model).temperature
        && !thinks
    {
        body["temperature"] = json!(t);
    }
    body
}

/// Thinking budgets for models that take `budget_tokens`, by level.
const BUDGET_LOW: u32 = 4_096;
const BUDGET_MEDIUM: u32 = 16_000;
const BUDGET_HIGH: u32 = 32_000;
/// The smallest budget the API accepts.
const BUDGET_MIN: u32 = 1_024;

/// Writes the model's own reasoning settings for `reasoning`. Returns whether the request
/// asks for thinking.
fn apply_reasoning(body: &mut Value, thinking: Thinking, reasoning: Reasoning, max_tokens: u32) -> bool {
    let effort = match reasoning {
        Reasoning::Default => return false,
        Reasoning::Off => None,
        Reasoning::Low => Some("low"),
        Reasoning::Medium => Some("medium"),
        Reasoning::High => Some("high"),
    };
    match (thinking, effort) {
        (Thinking::None, _) | (Thinking::Budget, None) => false,
        (Thinking::Budget, Some(level)) => {
            let target = match level {
                "low" => BUDGET_LOW,
                "medium" => BUDGET_MEDIUM,
                _ => BUDGET_HIGH,
            };
            // The budget must stay below max_tokens.
            let budget = target.min(max_tokens.saturating_sub(1));
            if budget < BUDGET_MIN {
                return false;
            }
            body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            true
        }
        (Thinking::Adaptive { off, .. }, None) => {
            match off {
                ThinkingOff::Disable => body["thinking"] = json!({ "type": "disabled" }),
                ThinkingOff::BetweenTools => body["thinking"] = json!({ "type": "between_tools" }),
                ThinkingOff::LowEffort => body["output_config"] = json!({ "effort": "low" }),
            }
            off == ThinkingOff::LowEffort
        }
        (Thinking::Adaptive { summarize, .. }, Some(level)) => {
            body["thinking"] = if summarize {
                json!({ "type": "adaptive", "display": "summarized" })
            } else {
                json!({ "type": "adaptive" })
            };
            body["output_config"] = json!({ "effort": level });
            true
        }
    }
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

fn block_json(block: &ContentBlock, blobs: &dyn BlobSource) -> Option<Value> {
    Some(match block {
        ContentBlock::Text { text } if text.is_empty() => return None,
        ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
        // A block without a signature cannot be replayed (it came from another provider).
        ContentBlock::Thinking { text, signature: Some(signature) } => {
            json!({ "type": "thinking", "thinking": text, "signature": signature })
        }
        ContentBlock::Thinking { signature: None, .. } => return None,
        ContentBlock::Image { blob, media_type, name, .. } => {
            if !IMAGE_TYPES.contains(&media_type.as_str()) {
                return Some(json!({ "type": "text", "text": format!("[image {name}: {media_type} is not supported]") }));
            }
            match blobs.read(blob) {
                Ok(bytes) => json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                    },
                }),
                Err(_) => json!({ "type": "text", "text": format!("[image {name} is no longer available]") }),
            }
        }
        ContentBlock::ToolUse { id, name, input } => {
            let input = if input.is_object() { input.clone() } else { json!({}) };
            json!({ "type": "tool_use", "id": id, "name": name, "input": input })
        }
        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
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
    json!({ "name": tool.name, "description": tool.description, "input_schema": tool.input_schema })
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        _ => StopReason::Other, // refusal, pause_turn, future reasons
    }
}

/// A content block that is still streaming.
enum Open {
    Tool { id: String, name: String, json: String },
    Thinking { signature: String },
    Other,
}

/// Turns Messages stream events into [`StreamEvent`]s.
#[derive(Default)]
pub(crate) struct AnthropicDecoder {
    usage: Usage,
    blocks: HashMap<u64, Open>,
    stop: Option<StopReason>,
}

impl AnthropicDecoder {
    /// Takes the counts present in `u`; returns whether any was.
    fn update_usage(&mut self, u: &Value) -> bool {
        let mut changed = false;
        for (field, key) in [
            (&mut self.usage.input, "input_tokens"),
            (&mut self.usage.output, "output_tokens"),
            (&mut self.usage.cache_read, "cache_read_input_tokens"),
            (&mut self.usage.cache_write, "cache_creation_input_tokens"),
        ] {
            if let Some(n) = u.get(key).and_then(Value::as_u64) {
                *field = n;
                changed = true;
            }
        }
        changed
    }

    fn block_start(&mut self, index: u64, block: &Value, emit: &mut dyn FnMut(StreamEvent)) {
        let text = |key: &str| block[key].as_str().filter(|t| !t.is_empty()).map(str::to_string);
        let open = match block["type"].as_str() {
            Some("tool_use") => {
                let id = block["id"].as_str().unwrap_or_default().to_string();
                let name = block["name"].as_str().unwrap_or_default().to_string();
                emit(StreamEvent::ToolUseStart { id: id.clone(), name: name.clone() });
                Open::Tool { id, name, json: String::new() }
            }
            Some("text") => {
                if let Some(t) = text("text") {
                    emit(StreamEvent::TextDelta(t));
                }
                Open::Other
            }
            Some("thinking") => {
                if let Some(t) = text("thinking") {
                    emit(StreamEvent::ThinkingDelta(t));
                }
                Open::Thinking { signature: text("signature").unwrap_or_default() }
            }
            _ => Open::Other, // redacted_thinking and future block types
        };
        self.blocks.insert(index, open);
    }

    fn block_delta(&mut self, index: u64, delta: &Value, emit: &mut dyn FnMut(StreamEvent)) {
        let part = |key: &str| delta[key].as_str().unwrap_or_default();
        match (delta["type"].as_str(), self.blocks.get_mut(&index)) {
            (Some("text_delta"), _) if !part("text").is_empty() => emit(StreamEvent::TextDelta(part("text").to_string())),
            (Some("thinking_delta"), _) if !part("thinking").is_empty() => {
                emit(StreamEvent::ThinkingDelta(part("thinking").to_string()))
            }
            (Some("signature_delta"), Some(Open::Thinking { signature })) => signature.push_str(part("signature")),
            (Some("input_json_delta"), Some(Open::Tool { id, json, .. })) if !part("partial_json").is_empty() => {
                json.push_str(part("partial_json"));
                emit(StreamEvent::ToolUseInputDelta { id: id.clone(), json: part("partial_json").to_string() });
            }
            _ => {} // citations and future delta types
        }
    }

    fn block_stop(&mut self, index: u64, emit: &mut dyn FnMut(StreamEvent)) {
        match self.blocks.remove(&index) {
            Some(Open::Tool { id, name, json }) => {
                let (input, error) = tool_input::parse(&json);
                emit(StreamEvent::ToolUse { id, name, input, error });
            }
            Some(Open::Thinking { signature }) if !signature.is_empty() => emit(StreamEvent::ThinkingSignature(signature)),
            _ => {}
        }
    }

    fn finish(&mut self, emit: &mut dyn FnMut(StreamEvent)) {
        // Blocks a broken server never closed still deliver their tool calls.
        let mut open: Vec<u64> = self.blocks.keys().copied().collect();
        open.sort_unstable();
        for index in open {
            self.block_stop(index, emit);
        }
        emit(StreamEvent::Stop(self.stop.take().unwrap_or(StopReason::EndTurn)));
    }
}

impl Decoder for AnthropicDecoder {
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> Result<bool, LlmError> {
        let v: Value = serde_json::from_str(&ev.data)
            .map_err(|e| LlmError::Other(format!("the provider sent invalid JSON in a {:?} event: {e}", ev.event)))?;
        let kind = v.get("type").and_then(Value::as_str).unwrap_or(&ev.event);
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
        match kind {
            "message_start" => {
                if self.update_usage(&v["message"]["usage"]) {
                    emit(StreamEvent::Usage(self.usage));
                }
            }
            "content_block_start" => self.block_start(index, &v["content_block"], emit),
            "content_block_delta" => self.block_delta(index, &v["delta"], emit),
            "content_block_stop" => self.block_stop(index, emit),
            "message_delta" => {
                if let Some(reason) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(stop_reason(reason));
                }
                if self.update_usage(&v["usage"]) {
                    emit(StreamEvent::Usage(self.usage));
                }
            }
            "message_stop" => {
                self.finish(emit);
                return Ok(true);
            }
            "error" => return Err(http::stream_error(&v)),
            _ => {} // ping and future events
        }
        Ok(false)
    }

    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        if self.stop.is_some() {
            self.finish(emit);
            return Ok(());
        }
        Err(LlmError::Network("the response stream ended before the message was complete".into()))
    }
}

#[cfg(test)]
mod tests;
