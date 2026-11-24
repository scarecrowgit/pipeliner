//! The contract of this crate: what a model client looks like to the agent loop.
//!
//! Signatures here are owned by the lead. Implementations live in the other modules.

use std::fmt;
use std::time::Duration;

use base::cancel::CancelToken;
use model::{BlobId, Message, ModelRef, StopReason, ToolSpec, Usage};

/// One request to a model. `model` in [`LlmClient::stream`] picks the provider; everything
/// here is provider-neutral.
#[derive(Clone, Debug, Default)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// Output cap; 0 lets the client use the model's maximum.
    pub max_tokens: u32,
    /// Sent only to models that accept it; clients drop it for the ones that reject it.
    pub temperature: Option<f32>,
    /// How much the model should think before answering.
    pub reasoning: Reasoning,
}

/// Reasoning effort, mapped by each client to its provider's own setting (thinking for
/// Anthropic, reasoning effort for OpenAI-compatible APIs). Models without the feature
/// ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reasoning {
    /// Send nothing: the provider's default behaviour.
    #[default]
    Default,
    Off,
    Low,
    Medium,
    High,
}

/// What a client reports while a response streams, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    /// The provider's opaque signature of the thinking block that just ended.
    ThinkingSignature(String),
    /// A tool call began; its JSON input follows in `ToolUseInputDelta`s.
    ToolUseStart { id: String, name: String },
    ToolUseInputDelta { id: String, json: String },
    /// The tool call is complete. When the model's JSON could not be parsed, `input` is `{}`
    /// and `error` says why; the caller answers with an error result instead of running it.
    ToolUse { id: String, name: String, input: serde_json::Value, error: Option<String> },
    /// Token counts so far; may be sent more than once, the last one is final.
    Usage(Usage),
    /// Always the last event of a successful stream.
    Stop(StopReason),
}

/// Why a request failed, in the terms the agent loop branches on.
#[derive(Clone, Debug, PartialEq)]
pub enum LlmError {
    /// The key is missing or rejected.
    Auth(String),
    /// A gateway took the key but its own sign-in to the model's provider failed. Nothing in
    /// this app's config fixes it.
    UpstreamAuth(String),
    RateLimited { retry_after: Option<Duration>, message: String },
    /// The provider is overloaded or returned a 5xx.
    Overloaded(String),
    /// The request does not fit the model's context window.
    ContextTooLong(String),
    /// The request itself is wrong; retrying will not help.
    BadRequest(String),
    /// Connection, DNS, TLS, or a stream that broke part-way.
    Network(String),
    /// The cancel token fired.
    Cancelled,
    /// The provider or model in the `ModelRef` is not configured.
    UnknownModel(String),
    Other(String),
}

impl LlmError {
    /// Worth trying again after a pause.
    pub fn is_retryable(&self) -> bool {
        matches!(self, LlmError::RateLimited { .. } | LlmError::Overloaded(_) | LlmError::Network(_))
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Auth(m) => write!(f, "the provider rejected the API key: {m}"),
            LlmError::UpstreamAuth(m) => write!(
                f,
                "the gateway accepted your key, but its own sign-in to the model's provider failed; sign in again on the gateway or pick another model: {m}"
            ),
            LlmError::RateLimited { message, .. } => write!(f, "rate limited: {message}"),
            LlmError::Overloaded(m) => write!(f, "the provider is overloaded: {m}"),
            LlmError::ContextTooLong(m) => write!(f, "the conversation no longer fits the model's context: {m}"),
            LlmError::BadRequest(m) => write!(f, "the provider rejected the request: {m}"),
            LlmError::Network(m) => write!(f, "network error: {m}"),
            LlmError::Cancelled => f.write_str("cancelled"),
            LlmError::UnknownModel(m) => write!(f, "no provider is configured for {m}"),
            LlmError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for LlmError {}

/// Where image bytes come from. Messages carry blob ids; the client reads the bytes when it
/// builds the wire request.
pub trait BlobSource: Send + Sync {
    fn read(&self, id: &BlobId) -> std::io::Result<Vec<u8>>;
}

/// A [`BlobSource`] with nothing in it, for requests that carry no images.
pub struct NoBlobs;

impl BlobSource for NoBlobs {
    fn read(&self, id: &BlobId) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("no blob {id}")))
    }
}

/// A configured provider, as the model menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    /// `anthropic` or `openai`.
    pub kind: String,
    pub base_url: String,
    /// Models named in the config file; the live list comes from `list_models`.
    pub models: Vec<String>,
    pub has_key: bool,
}

/// Everything the agent loop needs from a model backend. The real implementation routes by
/// the provider in the `ModelRef`; tests use the scripted fake.
pub trait LlmClient: Send + Sync {
    /// Streams one response. Blocks the calling thread until the stream ends, fails, or the
    /// token is cancelled; cancellation must be noticed within a fraction of a second even
    /// when the socket is silent. On success the last event is `StreamEvent::Stop`.
    fn stream(
        &self,
        model: &ModelRef,
        request: &ChatRequest,
        blobs: &dyn BlobSource,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(StreamEvent),
    ) -> Result<(), LlmError>;

    /// Model ids the provider offers right now.
    fn list_models(&self, provider: &str) -> Result<Vec<String>, LlmError>;

    fn providers(&self) -> Vec<ProviderInfo>;

    /// The model new chats use when nothing else says otherwise.
    fn default_model(&self) -> Option<ModelRef>;
}
