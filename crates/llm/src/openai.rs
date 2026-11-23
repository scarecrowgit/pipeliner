//! OpenAI-compatible chat completions: `POST {base}/chat/completions` streamed as SSE,
//! `GET {base}/models`. Also serves OpenRouter, Ollama and most gateways.

use std::collections::BTreeMap;

use base::cancel::CancelToken;
use base64::Engine as _;
use model::{ContentBlock, ModelRef, Role, StopReason, ToolSpec, Usage};
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::http::{self, SHORT_CALL_TIMEOUT};
use crate::sse::{self, Decoder, SseEvent};
use crate::{BlobSource, ChatRequest, LlmError, Reasoning, StreamEvent, pricing, tool_input};

fn with_auth<B>(provider: &ProviderConfig, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
    match &provider.api_key {
        Some(key) => req.header("authorization", format!("Bearer {key}")),
        None => req,
    }
}

/// Every model id the provider lists.
pub(crate) fn list_models(provider: &ProviderConfig) -> Result<Vec<String>, LlmError> {
    let url = format!("{}/models", provider.base_url);
    let req = http::agent().get(&url).config().timeout_global(Some(SHORT_CALL_TIMEOUT)).build();
    let response = with_auth(provider, req).call().map_err(http::transport_error)?;
    let mut body = http::success_body(response)?;
    let page: Value = body
        .with_config()
        .limit(32 * 1024 * 1024) // OpenRouter's list is large
        .read_json()
        .map_err(|e| LlmError::Network(format!("reading the model list failed: {e}")))?;
    let data = page.get("data").and_then(Value::as_array);
    Ok(data.into_iter().flatten().filter_map(|m| m["id"].as_str()).map(String::from).collect())
}

/// Which field carries the output cap. api.openai.com wants `max_completion_tokens`
/// (reasoning models reject `max_tokens`); most other servers only know `max_tokens`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MaxTokensField {
    MaxTokens,
    MaxCompletionTokens,
}

impl MaxTokensField {
    fn key(self) -> &'static str {
        match self {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        }
    }

    fn other(self) -> Self {
        match self {
            MaxTokensField::MaxTokens => MaxTokensField::MaxCompletionTokens,
            MaxTokensField::MaxCompletionTokens => MaxTokensField::MaxTokens,
        }
    }
}

/// How the provider takes a reasoning effort.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EffortField {
    /// OpenAI's `reasoning_effort`, also understood by Ollama and most gateways.
    ReasoningEffort,
    /// OpenRouter's `reasoning: {effort}` object.
    OpenRouter,
}

impl EffortField {
    fn for_provider(provider: &ProviderConfig) -> Self {
        if provider.base_url.contains("openrouter.ai") { EffortField::OpenRouter } else { EffortField::ReasoningEffort }
    }
}

/// Request-shape choices that depend on the provider rather than the model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Wire {
    pub max_tokens: MaxTokensField,
    pub effort: EffortField,
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
    let field = if provider.base_url.contains("api.openai.com") {
        MaxTokensField::MaxCompletionTokens
    } else {
        MaxTokensField::MaxTokens
    };
    let wire = Wire { max_tokens: field, effort: EffortField::for_provider(provider) };
    let mut body = request_body(model, request, blobs, wire);
    let url = format!("{}/chat/completions", provider.base_url);
    let provider = provider.clone();
    let mut lines = http::spawn_stream(cancel, move || {
        let send = |body: &Value| {
            let bytes = http::to_bytes(body)?;
            let req = with_auth(&provider, http::agent().post(&url))
                .header("accept", "text/event-stream")
                .header("content-type", "application/json");
            http::success_body(req.send(&bytes[..]).map_err(http::transport_error)?)
        };
        match send(&body) {
            // The server wants the other name for the output cap: retry once with it.
            Err(LlmError::BadRequest(message)) if is_max_tokens_complaint(&message) => {
                swap_max_tokens_field(&mut body, field);
                send(&body)
            }
            other => other,
        }
    })?;
    sse::drive(&mut lines, &mut OpenAiDecoder::default(), on_event)
}

/// A complaint about the output-cap parameter ("Unsupported parameter: 'max_tokens' … use
/// 'max_completion_tokens' instead").
fn is_max_tokens_complaint(message: &str) -> bool {
    message.contains("max_tokens") || message.contains("max_completion_tokens")
}

fn swap_max_tokens_field(body: &mut Value, field: MaxTokensField) {
    if let Some(map) = body.as_object_mut()
        && let Some(value) = map.remove(field.key())
    {
        map.insert(field.other().key().to_string(), value);
    }
}

/// The chat-completions request body. Thinking blocks are dropped: these servers neither
/// accept nor need them back.
pub(crate) fn request_body(model: &ModelRef, request: &ChatRequest, blobs: &dyn BlobSource, wire: Wire) -> Value {
    let mut messages = Vec::with_capacity(request.messages.len() + 1);
    if !request.system.is_empty() {
        messages.push(json!({ "role": "system", "content": request.system }));
    }
    for message in &request.messages {
        match message.role {
            Role::User => user_messages(&message.content, blobs, &mut messages),
            Role::Assistant => {
                if let Some(m) = assistant_message(&message.content) {
                    messages.push(m);
                }
            }
        }
    }
    let mut body = json!({
        "model": model.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    // 0 means "the model's maximum", which these servers give when the field is absent.
    if request.max_tokens > 0 {
        body[wire.max_tokens.key()] = json!(request.max_tokens);
    }
    if !request.tools.is_empty() {
        body["tools"] = request.tools.iter().map(tool_json).collect();
    }
    let traits = pricing::traits(model);
    if let Some(effort) = effort(traits, request.reasoning) {
        match wire.effort {
            EffortField::ReasoningEffort => body["reasoning_effort"] = json!(effort),
            EffortField::OpenRouter => body["reasoning"] = json!({ "effort": effort }),
        }
    }
    if let Some(t) = request.temperature
        && traits.temperature
    {
        body["temperature"] = json!(t);
    }
    body
}

/// The effort to send for `reasoning`, if the model takes one.
fn effort(traits: pricing::Traits, reasoning: Reasoning) -> Option<&'static str> {
    if !traits.effort {
        return None;
    }
    match reasoning {
        Reasoning::Default => None,
        Reasoning::Off => traits.effort_off,
        Reasoning::Low => Some("low"),
        Reasoning::Medium => Some("medium"),
        Reasoning::High => Some("high"),
    }
}

/// Tool results become `tool` messages, which must directly follow the assistant's calls;
/// the rest becomes one user message, with content parts when it carries images.
fn user_messages(content: &[ContentBlock], blobs: &dyn BlobSource, out: &mut Vec<Value>) {
    let mut parts: Vec<Value> = Vec::new();
    let mut has_image = false;
    for block in content {
        match block {
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                let content = if *is_error { format!("Error: {content}") } else { content.clone() };
                out.push(json!({ "role": "tool", "tool_call_id": tool_use_id, "content": content }));
            }
            ContentBlock::Text { text } if !text.is_empty() => parts.push(json!({ "type": "text", "text": text })),
            ContentBlock::Image { blob, media_type, name, .. } => match blobs.read(blob) {
                Ok(bytes) => {
                    has_image = true;
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    parts.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{media_type};base64,{data}") } }));
                }
                Err(_) => parts.push(json!({ "type": "text", "text": format!("[image {name} is no longer available]") })),
            },
            _ => {}
        }
    }
    if parts.is_empty() {
        return;
    }
    if has_image {
        out.push(json!({ "role": "user", "content": parts }));
    } else {
        out.push(json!({ "role": "user", "content": joined_text(&parts) }));
    }
}

fn joined_text(parts: &[Value]) -> String {
    parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n\n")
}

fn assistant_message(content: &[ContentBlock]) -> Option<Value> {
    let text = content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let calls: Vec<Value> = content
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
    match (calls.is_empty(), text.is_empty()) {
        (true, true) => None,
        (true, false) => Some(json!({ "role": "assistant", "content": text })),
        (false, _) => {
            let content = if text.is_empty() { Value::Null } else { json!(text) };
            Some(json!({ "role": "assistant", "content": content, "tool_calls": calls }))
        }
    }
}

fn tool_json(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": { "name": tool.name, "description": tool.description, "parameters": tool.input_schema },
    })
}

/// A tool call being assembled from deltas.
#[derive(Default)]
struct Call {
    id: Option<String>,
    name: String,
    args: String,
    started: bool,
    /// Bytes of `args` already reported as `ToolUseInputDelta`.
    sent: usize,
}

/// Turns chat-completion chunks into [`StreamEvent`]s.
#[derive(Default)]
pub(crate) struct OpenAiDecoder {
    calls: BTreeMap<u64, Call>,
    finish: Option<String>,
    tool_uses: usize,
}

/// Where gateways put reasoning text: `reasoning_content` (DeepSeek, vLLM, Qwen),
/// `reasoning` (OpenRouter, Ollama), `thinking` (a few proxies).
const REASONING_FIELDS: &[&str] = &["reasoning_content", "reasoning", "thinking"];

impl OpenAiDecoder {
    fn tool_call_delta(&mut self, position: usize, tc: &Value, emit: &mut dyn FnMut(StreamEvent)) {
        let index = tc.get("index").and_then(Value::as_u64).unwrap_or(position as u64);
        let call = self.calls.entry(index).or_default();
        if call.id.is_none()
            && let Some(id) = tc.get("id").and_then(Value::as_str).filter(|s| !s.is_empty())
        {
            call.id = Some(id.to_string());
        }
        let function = &tc["function"];
        if !call.started
            && let Some(name) = function.get("name").and_then(Value::as_str)
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
            let id = call.id.get_or_insert_with(|| format!("call_{index}")).clone();
            emit(StreamEvent::ToolUseStart { id, name: call.name.clone() });
        }
        if call.started && call.args.len() > call.sent {
            let id = call.id.clone().unwrap_or_default();
            emit(StreamEvent::ToolUseInputDelta { id, json: call.args[call.sent..].to_string() });
            call.sent = call.args.len();
        }
    }

    /// Reports every assembled call, in index order.
    fn flush_calls(&mut self, emit: &mut dyn FnMut(StreamEvent)) {
        for (index, call) in std::mem::take(&mut self.calls) {
            if !call.started {
                continue; // never got a name: nothing to call
            }
            let id = call.id.unwrap_or_else(|| format!("call_{index}"));
            let (input, error) = tool_input::parse(&call.args);
            self.tool_uses += 1;
            emit(StreamEvent::ToolUse { id, name: call.name, input, error });
        }
    }

    fn finish(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        self.flush_calls(emit);
        let stop = match self.finish.as_deref() {
            Some("tool_calls" | "function_call") => StopReason::ToolUse,
            // Some servers (older Ollama) say "stop" after tool calls.
            Some("stop") | None if self.tool_uses > 0 => StopReason::ToolUse,
            Some("stop" | "end_turn") | None => StopReason::EndTurn,
            Some("length") => StopReason::MaxTokens,
            Some("error") => return Err(LlmError::Other("the provider ended the response with an error".into())),
            Some(_) => StopReason::Other, // content_filter and the like
        };
        emit(StreamEvent::Stop(stop));
        Ok(())
    }
}

/// Usage with Anthropic's meaning: `input` excludes cached tokens.
fn parse_usage(u: &Value) -> Option<Usage> {
    if !u.is_object() {
        return None;
    }
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let details = &u["prompt_tokens_details"];
    let prompt = n(&u["prompt_tokens"]);
    let cache_read = n(&details["cached_tokens"]).min(prompt);
    let cache_write = n(&details["cache_write_tokens"]).max(n(&u["cache_creation_input_tokens"])).min(prompt - cache_read);
    Some(Usage {
        input: prompt - cache_read - cache_write,
        output: n(&u["completion_tokens"]),
        cache_read,
        cache_write,
    })
}

impl Decoder for OpenAiDecoder {
    fn event(&mut self, ev: &SseEvent, emit: &mut dyn FnMut(StreamEvent)) -> Result<bool, LlmError> {
        let data = ev.data.trim();
        if data == "[DONE]" {
            self.finish(emit)?;
            return Ok(true);
        }
        if data.is_empty() {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Other(format!("the provider sent an invalid stream chunk: {e}")))?;
        if v.get("error").is_some_and(|e| !e.is_null()) || ev.event == "error" {
            return Err(http::stream_error(&v));
        }
        let choice = v
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|cs| cs.iter().find(|c| c.get("index").and_then(Value::as_u64).unwrap_or(0) == 0));
        if let Some(choice) = choice {
            let delta = &choice["delta"];
            let text = |key: &str| delta.get(key).and_then(Value::as_str).filter(|t| !t.is_empty());
            if let Some(t) = REASONING_FIELDS.iter().find_map(|key| text(key)) {
                emit(StreamEvent::ThinkingDelta(t.to_string()));
            }
            if let Some(t) = text("content") {
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

    fn eof(&mut self, emit: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        if self.finish.is_some() {
            return self.finish(emit);
        }
        Err(LlmError::Network("the response stream ended before the response was complete".into()))
    }
}

#[cfg(test)]
mod tests;
