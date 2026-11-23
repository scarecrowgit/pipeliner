//! [`ScriptedLlm`], the fake model every other crate tests against.
//!
//! Queue one [`Reply`] per expected model call; each `stream` call takes the next one, plays
//! it as a real client would (text in several deltas, tool calls with start, input deltas and
//! the parsed call, usage, then `Stop`), and records the request for assertions.
//!
//! ```
//! use llm::{ChatRequest, LlmClient, NoBlobs, Reply, ScriptedLlm};
//! use model::{ModelRef, StopReason};
//! use serde_json::json;
//!
//! let llm = ScriptedLlm::new()
//!     .reply(Reply::tool("t1", "read_file", json!({"path": "a.rs"})).and_text("Reading it."))
//!     .reply(Reply::text("All done.").with_stop(StopReason::EndTurn));
//!
//! let mut events = Vec::new();
//! let model = ModelRef::parse("scripted/scripted-model");
//! llm.stream(&model, &ChatRequest::default(), &NoBlobs, &Default::default(), &mut |e| events.push(e)).unwrap();
//! assert_eq!(llm.call_count(), 1);
//! assert_eq!(llm.remaining(), 1);
//! ```

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base::cancel::CancelToken;
use model::{ModelRef, StopReason, Usage};
use serde_json::Value;

use crate::{BlobSource, ChatRequest, LlmClient, LlmError, ProviderInfo, StreamEvent, tool_input};

/// One scripted model response.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    steps: Vec<Step>,
    usage: Option<Usage>,
    stop: Option<StopReason>,
    delay: Duration,
    failure: Option<LlmError>,
}

#[derive(Clone, Debug)]
enum Step {
    Text(String),
    Thinking { text: String, signature: Option<String> },
    Tool { id: String, name: String, raw: String },
}

impl Reply {
    /// A response with nothing in it but `Stop(EndTurn)`.
    pub fn new() -> Self {
        Self::default()
    }

    /// A text answer, streamed word by word.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new().and_text(text)
    }

    /// Reasoning without a signature.
    pub fn thinking(text: impl Into<String>) -> Self {
        Self::new().and_thinking(text)
    }

    /// A single tool call; the stop reason becomes `ToolUse`.
    pub fn tool(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self::new().and_tool(id, name, input)
    }

    /// A call that fails before any event, as a rejected request does.
    pub fn error(error: LlmError) -> Self {
        Self { failure: Some(error), ..Self::default() }
    }

    /// Adds text after what is already scripted.
    pub fn and_text(mut self, text: impl Into<String>) -> Self {
        self.steps.push(Step::Text(text.into()));
        self
    }

    /// Adds unsigned reasoning.
    pub fn and_thinking(mut self, text: impl Into<String>) -> Self {
        self.steps.push(Step::Thinking { text: text.into(), signature: None });
        self
    }

    /// Adds reasoning followed by its `ThinkingSignature`.
    pub fn and_signed_thinking(mut self, text: impl Into<String>, signature: impl Into<String>) -> Self {
        self.steps.push(Step::Thinking { text: text.into(), signature: Some(signature.into()) });
        self
    }

    /// Adds a tool call with this input.
    pub fn and_tool(self, id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        self.and_raw_tool(id, name, input.to_string())
    }

    /// Adds a tool call whose input deltas are exactly `raw`, which need not be valid JSON;
    /// broken input yields `ToolUse { input: {}, error: Some(..) }` as a real client's would.
    pub fn and_raw_tool(mut self, id: impl Into<String>, name: impl Into<String>, raw: impl Into<String>) -> Self {
        self.steps.push(Step::Tool { id: id.into(), name: name.into(), raw: raw.into() });
        self
    }

    /// Reports this usage after the content.
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Overrides the stop reason (default: `ToolUse` when the reply calls a tool, else
    /// `EndTurn`).
    pub fn with_stop(mut self, stop: StopReason) -> Self {
        self.stop = Some(stop);
        self
    }

    /// Waits this long before the first event; cancelling during the wait returns
    /// `Err(Cancelled)` at once.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Plays the scripted content, then fails with `error` instead of stopping, like a
    /// stream that breaks part-way.
    pub fn then_fail(mut self, error: LlmError) -> Self {
        self.failure = Some(error);
        self
    }

    fn play(self, cancel: &CancelToken, on_event: &mut dyn FnMut(StreamEvent)) -> Result<(), LlmError> {
        if !cancel.sleep(self.delay) {
            return Err(LlmError::Cancelled);
        }
        let mut emit = |event: StreamEvent| {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            on_event(event);
            Ok(())
        };
        let has_tool = self.steps.iter().any(|s| matches!(s, Step::Tool { .. }));
        for step in self.steps {
            match step {
                Step::Text(text) => {
                    for word in text.split_inclusive(' ') {
                        emit(StreamEvent::TextDelta(word.to_string()))?;
                    }
                }
                Step::Thinking { text, signature } => {
                    for word in text.split_inclusive(' ') {
                        emit(StreamEvent::ThinkingDelta(word.to_string()))?;
                    }
                    if let Some(signature) = signature {
                        emit(StreamEvent::ThinkingSignature(signature))?;
                    }
                }
                Step::Tool { id, name, raw } => {
                    emit(StreamEvent::ToolUseStart { id: id.clone(), name: name.clone() })?;
                    let mid = (0..=raw.len() / 2).rev().find(|&i| raw.is_char_boundary(i)).unwrap_or(0);
                    for part in [&raw[..mid], &raw[mid..]] {
                        if !part.is_empty() {
                            emit(StreamEvent::ToolUseInputDelta { id: id.clone(), json: part.to_string() })?;
                        }
                    }
                    let (input, error) = tool_input::parse(&raw);
                    emit(StreamEvent::ToolUse { id, name, input, error })?;
                }
            }
        }
        if let Some(usage) = self.usage {
            emit(StreamEvent::Usage(usage))?;
        }
        if let Some(error) = self.failure {
            return Err(error);
        }
        let stop = self.stop.unwrap_or(if has_tool { StopReason::ToolUse } else { StopReason::EndTurn });
        emit(StreamEvent::Stop(stop))
    }
}

/// A model client that plays queued [`Reply`]s and records what it was asked.
///
/// Calling it with an empty queue panics with the request it got: a test that makes more
/// model calls than it scripted is wrong, and should say so loudly.
pub struct ScriptedLlm {
    queue: Mutex<VecDeque<Reply>>,
    calls: Mutex<Vec<(ModelRef, ChatRequest)>>,
    providers: Vec<ProviderInfo>,
    default_model: Option<ModelRef>,
}

impl Default for ScriptedLlm {
    fn default() -> Self {
        Self::new()
    }
}

/// The provider a fresh [`ScriptedLlm`] reports.
pub const SCRIPTED_PROVIDER: &str = "scripted";
/// The model a fresh [`ScriptedLlm`] reports as its default.
pub const SCRIPTED_MODEL: &str = "scripted-model";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking test thread must not hide what was recorded.
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ScriptedLlm {
    /// An empty queue, one provider `scripted` with model `scripted-model` as the default.
    pub fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            calls: Mutex::new(Vec::new()),
            providers: vec![ProviderInfo {
                id: SCRIPTED_PROVIDER.into(),
                kind: "anthropic".into(),
                base_url: String::new(),
                models: vec![SCRIPTED_MODEL.into()],
                has_key: true,
            }],
            default_model: Some(ModelRef::new(SCRIPTED_PROVIDER, SCRIPTED_MODEL)),
        }
    }

    /// Queues the response to the next call nobody scripted yet.
    pub fn reply(self, reply: Reply) -> Self {
        self.push(reply);
        self
    }

    /// Queues several responses in order.
    pub fn replies(self, replies: impl IntoIterator<Item = Reply>) -> Self {
        lock(&self.queue).extend(replies);
        self
    }

    /// Queues a response through a shared handle, for tests that script as they go.
    pub fn push(&self, reply: Reply) {
        lock(&self.queue).push_back(reply);
    }

    /// Replaces the reported providers.
    pub fn with_providers(mut self, providers: Vec<ProviderInfo>) -> Self {
        self.providers = providers;
        self
    }

    /// Replaces the default model.
    pub fn with_default_model(mut self, model: Option<ModelRef>) -> Self {
        self.default_model = model;
        self
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<(ModelRef, ChatRequest)> {
        lock(&self.calls).clone()
    }

    /// The requests of every call so far, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        lock(&self.calls).iter().map(|(_, r)| r.clone()).collect()
    }

    /// The most recent request.
    pub fn last_request(&self) -> Option<ChatRequest> {
        lock(&self.calls).last().map(|(_, r)| r.clone())
    }

    pub fn call_count(&self) -> usize {
        lock(&self.calls).len()
    }

    /// Responses still queued.
    pub fn remaining(&self) -> usize {
        lock(&self.queue).len()
    }
}

impl LlmClient for ScriptedLlm {
    fn stream(
        &self,
        model: &ModelRef,
        request: &ChatRequest,
        _blobs: &dyn BlobSource,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), LlmError> {
        let call = {
            let mut calls = lock(&self.calls);
            calls.push((model.clone(), request.clone()));
            calls.len()
        };
        let next = lock(&self.queue).pop_front();
        let Some(reply) = next else {
            let last = request.messages.last().map(|m| m.plain_text()).unwrap_or_default();
            panic!("ScriptedLlm: call #{call} to {model} has no scripted reply (last message: {last:?})");
        };
        reply.play(cancel, on_event)
    }

    fn list_models(&self, provider: &str) -> Result<Vec<String>, LlmError> {
        self.providers
            .iter()
            .find(|p| p.id == provider)
            .map(|p| p.models.clone())
            .ok_or_else(|| LlmError::UnknownModel(provider.to_string()))
    }

    fn providers(&self) -> Vec<ProviderInfo> {
        self.providers.clone()
    }

    fn default_model(&self) -> Option<ModelRef> {
        self.default_model.clone()
    }
}

#[cfg(test)]
mod tests;
