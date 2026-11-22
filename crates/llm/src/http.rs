//! Shared HTTP plumbing: one pooled `ureq` agent, error-body extraction.

use std::io::Read as _;
use std::sync::OnceLock;
use std::time::Duration;

use ureq::Body;
use ureq::http::Response;

/// Connect budget for every request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Max silence on the socket while streaming (Anthropic pings every ~15 s; local models
/// can take a while to load).
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Whole-request budget for small non-streaming calls (`/models`).
pub(crate) const SHORT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Pooled agent. No global timeout so long generations can stream; 4xx/5xx are returned
/// as responses (not errors) so we can surface the API's own error message.
pub(crate) fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_per_read(Some(IDLE_READ_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(concat!("harness-llm/", env!("CARGO_PKG_VERSION")))
            .build();
        ureq::Agent::new_with_config(config)
    })
}

/// If `response` is not 2xx, read its body and turn it into an error
/// `"{what}: HTTP {status}: {message}"`.
pub(crate) fn check_status(what: &str, response: Response<Body>) -> anyhow::Result<Response<Body>> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let mut body = String::new();
    let _ = response
        .into_body()
        .into_reader()
        .take(64 * 1024)
        .read_to_string(&mut body);
    let message = error_message(&body);
    let reason = status.canonical_reason().unwrap_or("");
    match message {
        Some(m) => anyhow::bail!("{what}: HTTP {} {reason}: {m}", status.as_u16()),
        None => anyhow::bail!("{what}: HTTP {} {reason}", status.as_u16()),
    }
}

/// Pull a human-readable message out of an API error body. Handles
/// Anthropic `{"error":{"type","message"}}`, OpenAI `{"error":{"message"}}`,
/// Ollama `{"error":"..."}`, and `{"message":"..."}`; falls back to the raw text.
pub(crate) fn error_message(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(m) = json_error_message(&v)
    {
        return Some(m);
    }
    let mut text: String = body.chars().take(500).collect();
    if text.len() < body.len() {
        text.push('…');
    }
    Some(text)
}

/// Message from an error-shaped JSON value (see [`error_message`]).
pub(crate) fn json_error_message(v: &serde_json::Value) -> Option<String> {
    let err = v.get("error").unwrap_or(v);
    if let Some(s) = err.as_str() {
        return Some(s.to_string());
    }
    let message = err.get("message").and_then(|m| m.as_str())?;
    match err.get("type").and_then(|t| t.as_str()) {
        Some(kind) if !kind.is_empty() && kind != "error" => Some(format!("{message} ({kind})")),
        _ => Some(message.to_string()),
    }
}
