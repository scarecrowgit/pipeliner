//! Which providers exist: the user's `config.toml` plus providers found in the environment.
//!
//! The file format is the old app's, unchanged, because users keep their file:
//!
//! ```toml
//! default_model = "anthropic/claude-sonnet-5-5"   # "<provider id>/<model id>"
//!
//! [[providers]]
//! id = "anthropic"
//! kind = "anthropic"                    # or "openai"
//! base_url = "https://api.anthropic.com"
//! api_key_env = "ANTHROPIC_API_KEY"     # or api_key = "..."
//! models = ["claude-sonnet-5-5"]        # optional
//! ```
//!
//! Other keys and tables (`[[mcp_servers]]`, `import_claude_code`, …) belong to other crates
//! and are ignored here.
//!
//! Providers found in the environment are added when no provider with that id exists:
//! `ANTHROPIC_API_KEY` → `anthropic`; `OPENAI_API_KEY` (with `OPENAI_BASE_URL` if set) →
//! `openai`; `OPENROUTER_API_KEY` → `openrouter`; `OLLAMA_HOST`, or a server answering on
//! `localhost:11434`, → `ollama` (no key). `ANTHROPIC_BASE_URL` is deliberately ignored: it
//! points other tools at proxies this app should not use by surprise.

use std::fmt;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use base::paths::Paths;
use model::ModelRef;
use serde::Deserialize;

/// The wire protocol a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Anthropic Messages (`POST /v1/messages`).
    Anthropic,
    /// OpenAI-compatible chat completions; also OpenRouter, Ollama and most gateways.
    OpenAi,
}

impl ProviderKind {
    /// The name used in the config file: `anthropic` or `openai`.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
        }
    }
}

/// One provider: where it lives and how to authenticate.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    /// The `provider` half of a [`ModelRef`].
    pub id: String,
    pub kind: ProviderKind,
    /// Anthropic: the host root (`https://api.anthropic.com`). OpenAI-compatible: the API
    /// root including `/v1` (`https://api.openai.com/v1`). No trailing slash.
    pub base_url: String,
    /// The resolved key; `None` for keyless local servers.
    pub api_key: Option<String>,
    /// Models named in the file.
    pub models: Vec<String>,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("models", &self.models)
            .finish()
    }
}

/// Every configured provider and the default model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub providers: Vec<ProviderConfig>,
    pub default_model: Option<ModelRef>,
}

/// An environment lookup: the process environment in the app, a map in tests.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

impl Config {
    /// Reads `paths.config_file` (a missing file is an empty config) and adds providers
    /// found in the process environment.
    pub fn load(paths: &Paths) -> anyhow::Result<Config> {
        Self::load_with(paths, &|name| std::env::var(name).ok(), &ollama_listening)
    }

    /// [`Config::load`] with the environment and the Ollama probe passed in.
    pub fn load_with(paths: &Paths, env: EnvLookup<'_>, ollama_probe: &dyn Fn() -> bool) -> anyhow::Result<Config> {
        let mut config = Self::read_file(&paths.config_file, env)?;
        config.add_env_providers(env, ollama_probe);
        Ok(config)
    }

    fn read_file(path: &Path, env: EnvLookup<'_>) -> anyhow::Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::parse(&text, env).with_context(|| format!("reading {}", path.display()))
    }

    /// Parses the file's text. `env` resolves `api_key_env`. Adds nothing from the
    /// environment; see [`Config::add_env_providers`].
    pub fn parse(text: &str, env: EnvLookup<'_>) -> anyhow::Result<Config> {
        // toml's own Display quotes the offending source line, which may hold a key.
        let raw: RawConfig = toml::from_str(text).map_err(|e| anyhow::anyhow!(toml_error(text, &e)))?;
        let mut providers: Vec<ProviderConfig> = Vec::with_capacity(raw.providers.len());
        for p in raw.providers {
            let id = p.id.trim().to_string();
            anyhow::ensure!(!id.is_empty(), "a provider has an empty id");
            anyhow::ensure!(!id.contains('/'), "provider id {id:?} must not contain '/'");
            anyhow::ensure!(providers.iter().all(|q| q.id != id), "provider id {id:?} appears twice");
            let base_url = match p.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
                Some(url) => normalize_base_url(p.kind, url),
                None => default_base_url(p.kind).to_string(),
            };
            let api_key = non_empty(p.api_key).or_else(|| p.api_key_env.as_deref().and_then(|var| non_empty(env(var))));
            providers.push(ProviderConfig { id, kind: p.kind, base_url, api_key, models: p.models });
        }
        let default_model = raw.default_model.filter(|m| !m.trim().is_empty()).map(|m| ModelRef::parse(m.trim()));
        Ok(Config { providers, default_model })
    }

    /// Appends providers found in the environment (see the module docs), keeping any
    /// provider the file already defines under the same id. `ollama_probe` is asked only
    /// when `OLLAMA_HOST` is unset and no `ollama` provider exists.
    pub fn add_env_providers(&mut self, env: EnvLookup<'_>, ollama_probe: &dyn Fn() -> bool) {
        let env = |name: &str| non_empty(env(name));
        if let Some(key) = env("ANTHROPIC_API_KEY") {
            self.add_if_missing("anthropic", ProviderKind::Anthropic, ANTHROPIC_URL, Some(key));
        }
        if let Some(key) = env("OPENAI_API_KEY") {
            let base = env("OPENAI_BASE_URL").unwrap_or_else(|| OPENAI_URL.to_string());
            self.add_if_missing("openai", ProviderKind::OpenAi, &base, Some(key));
        }
        if let Some(key) = env("OPENROUTER_API_KEY") {
            self.add_if_missing("openrouter", ProviderKind::OpenAi, OPENROUTER_URL, Some(key));
        }
        if self.provider("ollama").is_none() {
            let base = match env("OLLAMA_HOST") {
                Some(host) => Some(ollama_base_url(&host)),
                None => ollama_probe().then(|| OLLAMA_URL.to_string()),
            };
            if let Some(base) = base {
                self.add_if_missing("ollama", ProviderKind::OpenAi, &base, None);
            }
        }
    }

    fn add_if_missing(&mut self, id: &str, kind: ProviderKind, base: &str, api_key: Option<String>) {
        if self.provider(id).is_none() {
            self.providers.push(ProviderConfig {
                id: id.to_string(),
                kind,
                base_url: normalize_base_url(kind, base),
                api_key,
                models: Vec::new(),
            });
        }
    }

    /// The provider with this id.
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }
}

const ANTHROPIC_URL: &str = "https://api.anthropic.com";
const OPENAI_URL: &str = "https://api.openai.com/v1";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
const OLLAMA_URL: &str = "http://localhost:11434/v1";

#[derive(Deserialize)]
struct RawConfig {
    default_model: Option<String>,
    #[serde(default)]
    providers: Vec<RawProvider>,
}

#[derive(Deserialize)]
struct RawProvider {
    id: String,
    kind: ProviderKind,
    base_url: Option<String>,
    api_key: Option<String>,
    api_key_env: Option<String>,
    #[serde(default)]
    models: Vec<String>,
}

/// A parse error by line and message only, never quoting the source.
fn toml_error(text: &str, err: &toml::de::Error) -> String {
    match err.span() {
        Some(span) => {
            let line = text.get(..span.start).map_or(0, |before| before.matches('\n').count()) + 1;
            format!("line {line}: {}", err.message())
        }
        None => err.message().to_string(),
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => ANTHROPIC_URL,
        ProviderKind::OpenAi => OPENAI_URL,
    }
}

/// Trims whitespace and trailing slashes. Anthropic URLs are host roots, so a trailing
/// `/v1` (a common mistake) is dropped as well.
fn normalize_base_url(kind: ProviderKind, url: &str) -> String {
    let mut url = url.trim().trim_end_matches('/');
    if kind == ProviderKind::Anthropic {
        url = url.strip_suffix("/v1").unwrap_or(url).trim_end_matches('/');
    }
    url.to_string()
}

/// `OLLAMA_HOST` is the server's bind address: `host`, `host:port` or a URL. `0.0.0.0` means
/// every interface, so the client connects to localhost. Like Ollama, a bare host defaults
/// to port 11434 and an explicit scheme to that scheme's port.
fn ollama_base_url(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    let (scheme, rest, explicit_scheme) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest, true),
        None => ("http", host, false),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let has_port = authority
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    let authority = authority.replacen("0.0.0.0", "localhost", 1);
    let authority = if authority.is_empty() {
        "localhost:11434".to_string()
    } else if authority.starts_with(':') {
        format!("localhost{authority}")
    } else if has_port || explicit_scheme {
        authority
    } else {
        format!("{authority}:11434")
    };
    let path = path.trim_end_matches('/');
    let path = if path.ends_with("/v1") { path.to_string() } else { format!("{path}/v1") };
    format!("{scheme}://{authority}{path}")
}

/// Whether something accepts connections on Ollama's default port (300 ms budget).
fn ollama_listening() -> bool {
    let Ok(addrs) = ("localhost", 11434).to_socket_addrs() else {
        return false;
    };
    addrs.into_iter().any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

#[cfg(test)]
mod tests;
