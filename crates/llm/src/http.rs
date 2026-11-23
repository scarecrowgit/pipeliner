//! HTTP plumbing shared by both protocols: the pooled agent, mapping failures to
//! [`LlmError`], and the reader thread that keeps a silent socket from blocking cancellation.

use std::io::{BufRead as _, BufReader, Read as _};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use base::cancel::CancelToken;
use serde_json::Value;
use ureq::Body;
use ureq::http::Response;

use crate::LlmError;
use crate::sse::LineSource;

/// Connect budget for every request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Longest silence on a socket. Anthropic pings every ~15 s and local models can take a
/// while to load; this also bounds how long an abandoned reader thread lives.
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Whole-call budget of small calls such as listing models.
pub(crate) const SHORT_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a waiting stream looks at the cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(50);
/// Most of an error body worth reading.
const ERROR_BODY_LIMIT: u64 = 64 * 1024;

/// The pooled agent. 4xx and 5xx come back as responses so their bodies can be read.
pub(crate) fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_per_read(Some(IDLE_READ_TIMEOUT))
            .http_status_as_error(false)
            .user_agent(concat!("harness/", env!("CARGO_PKG_VERSION")))
            .build();
        ureq::Agent::new_with_config(config)
    })
}

/// A request body as compact JSON (ureq's `send_json` pretty-prints).
pub(crate) fn to_bytes(body: &Value) -> Result<Vec<u8>, LlmError> {
    serde_json::to_vec(body).map_err(|e| LlmError::Other(format!("the request could not be encoded: {e}")))
}

/// A failure before any response arrived.
pub(crate) fn transport_error(err: ureq::Error) -> LlmError {
    match err {
        ureq::Error::BadUri(_) | ureq::Error::Http(_) | ureq::Error::InvalidProxyUrl => {
            LlmError::Other(format!("the request could not be built: {err}"))
        }
        other => LlmError::Network(other.to_string()),
    }
}

/// The body of a 2xx response, or the response turned into an error.
pub(crate) fn success_body(response: Response<Body>) -> Result<Body, LlmError> {
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response.into_body());
    }
    let retry_after = retry_after(&response);
    let mut bytes = Vec::new();
    let _ = response.into_body().into_reader().take(ERROR_BODY_LIMIT).read_to_end(&mut bytes);
    Err(status_error(status, retry_after, &String::from_utf8_lossy(&bytes)))
}

/// `retry-after-ms` or `retry-after` in seconds; the HTTP-date form is ignored.
fn retry_after(response: &Response<Body>) -> Option<Duration> {
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(ms) = header("retry-after-ms").and_then(|v| v.parse::<f64>().ok()) {
        return Duration::try_from_secs_f64(ms / 1000.0).ok();
    }
    header("retry-after").and_then(|v| v.parse::<f64>().ok()).and_then(|s| Duration::try_from_secs_f64(s).ok())
}

/// An HTTP error response mapped to the kind the agent loop branches on.
pub(crate) fn status_error(status: u16, retry_after: Option<Duration>, body: &str) -> LlmError {
    let json = serde_json::from_str::<Value>(body.trim()).ok();
    let message = json
        .as_ref()
        .and_then(json_error_message)
        .or_else(|| raw_message(body))
        .unwrap_or_else(|| format!("HTTP {status}"));
    let kind = json.as_ref().and_then(json_error_kind);
    classify(Some(status), kind.as_deref(), message, retry_after)
}

/// An error object that arrived inside a stream, such as Anthropic's `error` event or an
/// OpenAI-compatible chunk with an `error` field.
pub(crate) fn stream_error(value: &Value) -> LlmError {
    let message = json_error_message(value).unwrap_or_else(|| "the provider reported an error".to_string());
    let err = value.get("error").unwrap_or(value);
    let status = err
        .get("code")
        .or_else(|| err.get("status"))
        .and_then(Value::as_u64)
        .and_then(|c| u16::try_from(c).ok())
        .filter(|c| (400..600).contains(c));
    classify(status, json_error_kind(value).as_deref(), message, None)
}

/// The single place that decides an error's kind, from whatever the provider told us.
pub(crate) fn classify(status: Option<u16>, kind: Option<&str>, message: String, retry_after: Option<Duration>) -> LlmError {
    let kind = kind.unwrap_or_default();
    let context = kind == "context_length_exceeded" || mentions_context_limit(&message);
    match status {
        Some(401 | 403) => LlmError::Auth(message),
        Some(429) => LlmError::RateLimited { retry_after, message },
        Some(408) => LlmError::Network(message),
        Some(s) if s >= 500 => LlmError::Overloaded(message),
        Some(_) if context => LlmError::ContextTooLong(message),
        Some(_) => LlmError::BadRequest(message),
        None if context => LlmError::ContextTooLong(message),
        None => match kind {
            "authentication_error" | "permission_error" | "invalid_api_key" => LlmError::Auth(message),
            "rate_limit_error" | "rate_limit_exceeded" => LlmError::RateLimited { retry_after, message },
            "overloaded_error" | "api_error" | "server_error" | "timeout_error" => LlmError::Overloaded(message),
            "invalid_request_error" | "request_too_large" | "not_found_error" => LlmError::BadRequest(message),
            _ => LlmError::Other(message),
        },
    }
}

/// Whether a provider's message says the request does not fit the context window, in the
/// words Anthropic, OpenAI, OpenRouter, vLLM and Ollama use.
fn mentions_context_limit(message: &str) -> bool {
    let m = message.to_lowercase();
    [
        "prompt is too long",
        "context length",
        "context_length",
        "context window",
        "context limit",
        "maximum context",
        "input is too long",
        "too many input tokens",
    ]
    .iter()
    .any(|needle| m.contains(needle))
}

/// A human-readable message from an error-shaped JSON value: Anthropic
/// `{"error":{"type","message"}}`, OpenAI `{"error":{"message"}}`, Ollama `{"error":"…"}`,
/// or a bare `{"message":"…"}`.
pub(crate) fn json_error_message(v: &Value) -> Option<String> {
    let err = v.get("error").unwrap_or(v);
    if let Some(s) = err.as_str() {
        return Some(s.to_string());
    }
    let message = err.get("message").and_then(Value::as_str)?;
    match err.get("type").and_then(Value::as_str) {
        Some(kind) if !kind.is_empty() && kind != "error" => Some(format!("{message} ({kind})")),
        _ => Some(message.to_string()),
    }
}

/// The machine-readable kind of an error value: its `type`, or a string `code`.
fn json_error_kind(v: &Value) -> Option<String> {
    let err = v.get("error").unwrap_or(v);
    let field = |name: &str| err.get(name).and_then(Value::as_str).filter(|s| !s.is_empty() && *s != "error");
    field("code").or_else(|| field("type")).map(str::to_string)
}

/// A non-JSON error body, shortened.
fn raw_message(body: &str) -> Option<String> {
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let mut text: String = body.chars().take(500).collect();
    if text.len() < body.len() {
        text.push('…');
    }
    Some(text)
}

/// Lines of a response body read on a helper thread, so that waiting for them can be
/// interrupted by the cancel token even when the socket is silent.
pub(crate) struct ChannelLines<'a> {
    rx: Receiver<Result<Vec<u8>, LlmError>>,
    cancel: &'a CancelToken,
}

impl LineSource for ChannelLines<'_> {
    fn next_line(&mut self) -> Result<Option<Vec<u8>>, LlmError> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            match self.rx.recv_timeout(CANCEL_POLL) {
                Ok(line) => return line.map(Some),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

/// Runs `open` (send the request, check the status) and then reads the body line by line,
/// all on a helper thread. A cancelled caller simply stops listening; the thread ends when
/// its next send fails or the socket's read timeout fires.
pub(crate) fn spawn_stream<'a>(
    cancel: &'a CancelToken,
    open: impl FnOnce() -> Result<Body, LlmError> + Send + 'static,
) -> Result<ChannelLines<'a>, LlmError> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("llm-stream".into())
        .spawn(move || {
            let body = match open() {
                Ok(body) => body,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let mut reader = BufReader::new(body.into_reader());
            loop {
                let mut line = Vec::new();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::Network(format!("the response stream broke: {e}"))));
                        return;
                    }
                }
            }
        })
        .map_err(|e| LlmError::Other(format!("could not start the stream reader: {e}")))?;
    Ok(ChannelLines { rx, cancel })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_kinds() {
        let anthropic = |kind: &str, msg: &str| format!(r#"{{"type":"error","error":{{"type":"{kind}","message":"{msg}"}}}}"#);
        assert!(matches!(status_error(401, None, &anthropic("authentication_error", "bad key")), LlmError::Auth(m) if m == "bad key (authentication_error)"));
        assert!(matches!(status_error(403, None, ""), LlmError::Auth(m) if m == "HTTP 403"));
        assert_eq!(
            status_error(429, Some(Duration::from_secs(7)), r#"{"error":{"message":"slow down"}}"#),
            LlmError::RateLimited { retry_after: Some(Duration::from_secs(7)), message: "slow down".into() }
        );
        assert!(matches!(status_error(529, None, &anthropic("overloaded_error", "Overloaded")), LlmError::Overloaded(_)));
        assert!(matches!(status_error(502, None, "<html>bad gateway</html>"), LlmError::Overloaded(m) if m.contains("bad gateway")));
        assert!(matches!(
            status_error(400, None, &anthropic("invalid_request_error", "prompt is too long: 250000 tokens > 200000 maximum")),
            LlmError::ContextTooLong(_)
        ));
        assert!(matches!(
            status_error(400, None, r#"{"error":{"message":"This model's maximum context length is 128000 tokens","code":"context_length_exceeded"}}"#),
            LlmError::ContextTooLong(_)
        ));
        assert!(matches!(status_error(404, None, r#"{"error":"model 'x' not found"}"#), LlmError::BadRequest(m) if m == "model 'x' not found"));
        assert!(matches!(status_error(408, None, ""), LlmError::Network(_)));
        // A rate limit that talks about tokens is still a rate limit.
        assert!(matches!(status_error(429, None, "too many tokens per minute, context length"), LlmError::RateLimited { .. }));
    }

    #[test]
    fn stream_errors_map_by_type_or_code() {
        let v: Value = serde_json::from_str(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#).unwrap();
        assert_eq!(stream_error(&v), LlmError::Overloaded("Overloaded (overloaded_error)".into()));
        let v: Value = serde_json::from_str(r#"{"error":{"message":"Rate limit exceeded","code":429}}"#).unwrap();
        assert!(matches!(stream_error(&v), LlmError::RateLimited { .. }));
        let v: Value = serde_json::from_str(r#"{"error":{"message":"nope","code":"invalid_api_key"}}"#).unwrap();
        assert!(matches!(stream_error(&v), LlmError::Auth(_)));
        let v: Value = serde_json::from_str(r#"{"error":"something odd"}"#).unwrap();
        assert_eq!(stream_error(&v), LlmError::Other("something odd".into()));
    }

    #[test]
    fn long_raw_bodies_are_shortened() {
        let body = "x".repeat(2000);
        let LlmError::BadRequest(m) = status_error(400, None, &body) else { panic!() };
        assert_eq!(m.chars().count(), 501);
    }
}
