//! Provider-agnostic LLM client: streaming chat with tool calling.
//!
//! Blocking API — call it from a background thread (the GPUI side uses
//! `cx.background_spawn` and forwards [`StreamEvent`]s over a channel).
//! No GPUI dependency, so it can be tested with `cargo run -p llm --example chat`.
//!
//! Supported wire protocols ([`ProviderKind`]):
//! - `Anthropic` — `POST {base_url}/v1/messages` with `stream: true` (SSE), `GET /v1/models`.
//! - `OpenAi` — `POST {base_url}/chat/completions` with `stream: true` and
//!   `stream_options.include_usage` (SSE), `GET {base_url}/models`. Covers OpenAI, OpenRouter,
//!   Ollama (`http://localhost:11434/v1`), and OpenAI-compatible gateways.

mod anthropic;
pub mod config;
mod http;
mod openai;
mod sse;
mod types;

pub use config::{Config, ProviderConfig, ProviderKind};
pub use types::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// List model ids the provider offers (`GET .../models`). Falls back to
/// `provider.models` if the endpoint is unavailable and that list is non-empty.
pub fn list_models(provider: &ProviderConfig) -> anyhow::Result<Vec<String>> {
    let fetched = match provider.kind {
        ProviderKind::Anthropic => anthropic::list_models(provider),
        ProviderKind::OpenAi => openai::list_models(provider),
    };
    let mut models = match fetched {
        Ok(models) if !models.is_empty() || provider.models.is_empty() => models,
        Ok(_) => provider.models.clone(),
        Err(_) if !provider.models.is_empty() => provider.models.clone(),
        Err(e) => return Err(e),
    };
    models.sort();
    models.dedup();
    Ok(models)
}

/// Stream one model turn. Calls `on_event` for every event in order and returns after
/// [`StreamEvent::Stop`] (or an error). Checks `cancel` between SSE lines and returns
/// `Ok(())` early with `Stop(StopReason::Cancelled)` when it is set.
///
/// Tool calls: emits `ToolUseStart` when a call begins, `ToolUseInputDelta` as its JSON
/// arguments stream, and a final `ToolUse` with the parsed `input` once the call is
/// complete. The turn ends with `Stop(StopReason::ToolUse)` when the model wants tool
/// results — the caller then executes the tools, appends an assistant message with the
/// `ToolUse` blocks and a user message with `ToolResult` blocks, and calls this again.
pub fn stream_chat(
    provider: &ProviderConfig,
    request: &ChatRequest,
    cancel: Arc<AtomicBool>,
    on_event: &mut dyn FnMut(StreamEvent),
) -> anyhow::Result<()> {
    if cancel.load(Ordering::Relaxed) {
        on_event(StreamEvent::Stop(StopReason::Cancelled));
        return Ok(());
    }
    match provider.kind {
        ProviderKind::Anthropic => anthropic::stream_chat(provider, request, &cancel, on_event),
        ProviderKind::OpenAi => openai::stream_chat(provider, request, &cancel, on_event),
    }
}
