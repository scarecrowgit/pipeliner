//! Provider configuration: `~/.config/harness/config.toml` (or `$HARNESS_CONFIG`) plus
//! providers auto-detected from environment variables.
//!
//! ```toml
//! default_model = "anthropic/claude-sonnet-5-5"   # "<provider id>/<model id>"
//!
//! [[providers]]
//! id = "anthropic"
//! kind = "anthropic"                    # or "openai"
//! base_url = "https://api.anthropic.com"
//! api_key_env = "ANTHROPIC_API_KEY"     # or api_key = "..." (discouraged)
//! models = ["claude-sonnet-5-5"]        # optional; used if /models is unavailable
//! ```
//!
//! Env auto-detection (added when the variable is set and no provider with that id exists):
//! `ANTHROPIC_API_KEY` → id "anthropic"; `OPENAI_API_KEY` (+ optional `OPENAI_BASE_URL`) →
//! "openai"; `OPENROUTER_API_KEY` → "openrouter" (`https://openrouter.ai/api/v1`);
//! `OLLAMA_HOST` or a reachable `http://localhost:11434` → "ollama" (no key).

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderConfig {
    pub id: String,
    pub kind: ProviderKind,
    /// Anthropic: host root (`https://api.anthropic.com`). OpenAI: API root including
    /// `/v1` (`https://api.openai.com/v1`).
    pub base_url: String,
    /// Resolved key (from `api_key` or `api_key_env`). `None` for keyless local servers.
    pub api_key: Option<String>,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub providers: Vec<ProviderConfig>,
    /// `"<provider id>/<model id>"`.
    pub default_model: Option<String>,
}

impl Config {
    /// Load the config file (missing file = empty) and add env-detected providers.
    pub fn load() -> anyhow::Result<Config> {
        let path = Self::path();
        let mut config = Self::from_file(&path, &env_lookup)?;
        config.add_env_providers(&env_lookup, &ollama_reachable);
        Ok(config)
    }

    /// Parse a config file (missing file = empty config). No env auto-detection.
    fn from_file(path: &Path, env: &dyn Fn(&str) -> Option<String>) -> anyhow::Result<Config> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Self::from_toml_str(&text, env).with_context(|| format!("parsing {}", path.display()))
    }

    /// Parse config TOML. `env` resolves `api_key_env` variables (pass
    /// `|k| std::env::var(k).ok()` for the real environment). Does not add env-detected
    /// providers; see [`Config::add_env_providers`].
    pub fn from_toml_str(
        text: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Config> {
        let raw: RawConfig = toml::from_str(text)?;
        let mut providers: Vec<ProviderConfig> = Vec::with_capacity(raw.providers.len());
        for p in raw.providers {
            let id = p.id.trim().to_string();
            anyhow::ensure!(!id.is_empty(), "provider with empty id");
            anyhow::ensure!(!id.contains('/'), "provider id {id:?} must not contain '/'");
            anyhow::ensure!(
                providers.iter().all(|q| q.id != id),
                "duplicate provider id {id:?}"
            );
            let base_url = match p.base_url {
                Some(url) => normalize_base_url(p.kind, &url),
                None => default_base_url(p.kind).to_string(),
            };
            let api_key = p
                .api_key
                .filter(|k| !k.is_empty())
                .or_else(|| p.api_key_env.as_deref().and_then(|var| non_empty(env(var))));
            providers.push(ProviderConfig {
                id,
                kind: p.kind,
                base_url,
                api_key,
                models: p.models,
            });
        }
        Ok(Config {
            providers,
            default_model: raw.default_model.filter(|m| !m.is_empty()),
        })
    }

    /// Append providers detected from environment variables (see module docs), skipping
    /// ids that already exist. `ollama_reachable` is only consulted when `OLLAMA_HOST` is
    /// unset and no "ollama" provider exists.
    pub fn add_env_providers(
        &mut self,
        env: &dyn Fn(&str) -> Option<String>,
        ollama_reachable: &dyn Fn() -> bool,
    ) {
        let env = |k: &str| non_empty(env(k));
        fn add(config: &mut Config, id: &str, kind: ProviderKind, base: &str, key: Option<String>) {
            if config.provider(id).is_none() {
                config.providers.push(ProviderConfig {
                    id: id.to_string(),
                    kind,
                    base_url: normalize_base_url(kind, base),
                    api_key: key,
                    models: Vec::new(),
                });
            }
        }
        if let Some(key) = env("ANTHROPIC_API_KEY") {
            // ANTHROPIC_BASE_URL is deliberately ignored: it configures other tools.
            add(
                self,
                "anthropic",
                ProviderKind::Anthropic,
                ANTHROPIC_URL,
                Some(key),
            );
        }
        if let Some(key) = env("OPENAI_API_KEY") {
            let base = env("OPENAI_BASE_URL").unwrap_or_else(|| OPENAI_URL.into());
            add(self, "openai", ProviderKind::OpenAi, &base, Some(key));
        }
        if let Some(key) = env("OPENROUTER_API_KEY") {
            add(
                self,
                "openrouter",
                ProviderKind::OpenAi,
                OPENROUTER_URL,
                Some(key),
            );
        }
        if self.provider("ollama").is_none() {
            let base = match env("OLLAMA_HOST") {
                Some(host) => Some(ollama_base_url(&host)),
                None => ollama_reachable().then(|| OLLAMA_URL.to_string()),
            };
            if let Some(base) = base {
                add(self, "ollama", ProviderKind::OpenAi, &base, None);
            }
        }
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Path of the config file that `load` reads.
    pub fn path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("HARNESS_CONFIG") {
            return p.into();
        }
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
            });
        base.join("harness").join("config.toml")
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

fn env_lookup(key: &str) -> Option<String> {
    std::env::var(key).ok()
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

/// Trim whitespace and trailing `/`. Anthropic URLs are host roots, so a trailing `/v1`
/// (a common mistake) is dropped too.
fn normalize_base_url(kind: ProviderKind, url: &str) -> String {
    let mut url = url.trim().trim_end_matches('/');
    if kind == ProviderKind::Anthropic {
        url = url.strip_suffix("/v1").unwrap_or(url).trim_end_matches('/');
    }
    url.to_string()
}

/// `OLLAMA_HOST` is the server's bind address: `host`, `host:port`, or a URL; `0.0.0.0`
/// means "all interfaces", so connect to localhost instead. Like Ollama itself, a bare
/// host defaults to port 11434, an explicit scheme to that scheme's default port.
fn ollama_base_url(host: &str) -> String {
    let host = host.trim().trim_end_matches('/');
    let (scheme, rest, explicit_scheme) = match host.split_once("://") {
        Some((scheme, rest)) => (scheme, rest, true),
        None => ("http", host, false),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let has_port = match authority.rsplit_once(':') {
        Some((_, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    };
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
    let path = if path.ends_with("/v1") {
        path.to_string()
    } else {
        format!("{path}/v1")
    };
    format!("{scheme}://{authority}{path}")
}

fn ollama_reachable() -> bool {
    let Ok(addrs) = ("localhost", 11434).to_socket_addrs() else {
        return false;
    };
    addrs
        .into_iter()
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok())
}

/// Split `"provider/model"` (model ids may themselves contain `/`, e.g. OpenRouter).
pub fn split_model_ref(s: &str) -> Option<(&str, &str)> {
    s.split_once('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn parses_file() {
        let env = env_of(&[("MY_KEY", "sk-secret")]);
        let config = Config::from_toml_str(
            r#"
            default_model = "anthropic/claude-sonnet-5-5"

            [[providers]]
            id = "anthropic"
            kind = "anthropic"
            base_url = "https://proxy.example.com/v1/"
            api_key_env = "MY_KEY"
            models = ["claude-sonnet-5-5"]

            [[providers]]
            id = "local"
            kind = "openai"
            base_url = "http://127.0.0.1:8080/v1/"

            [[providers]]
            id = "inline"
            kind = "openai"
            api_key = "k"
            api_key_env = "MISSING"
            "#,
            &env,
        )
        .unwrap();
        assert_eq!(
            config.default_model.as_deref(),
            Some("anthropic/claude-sonnet-5-5")
        );
        assert_eq!(
            config.providers,
            vec![
                ProviderConfig {
                    id: "anthropic".into(),
                    kind: ProviderKind::Anthropic,
                    base_url: "https://proxy.example.com".into(),
                    api_key: Some("sk-secret".into()),
                    models: vec!["claude-sonnet-5-5".into()],
                },
                ProviderConfig {
                    id: "local".into(),
                    kind: ProviderKind::OpenAi,
                    base_url: "http://127.0.0.1:8080/v1".into(),
                    api_key: None,
                    models: vec![],
                },
                ProviderConfig {
                    id: "inline".into(),
                    kind: ProviderKind::OpenAi,
                    base_url: OPENAI_URL.into(),
                    api_key: Some("k".into()),
                    models: vec![],
                },
            ]
        );
        assert_eq!(Config::from_toml_str("", &env).unwrap(), Config::default());
    }

    #[test]
    fn rejects_bad_files() {
        let env = env_of(&[]);
        assert!(
            Config::from_toml_str("[[providers]]\nid = \"x\"\nkind = \"gemini\"", &env).is_err()
        );
        assert!(Config::from_toml_str("default_model = ", &env).is_err());
        let dup = "[[providers]]\nid='a'\nkind='openai'\n[[providers]]\nid='a'\nkind='anthropic'";
        assert!(Config::from_toml_str(dup, &env).is_err());
    }

    #[test]
    fn load_error_mentions_path() {
        let dir = std::env::temp_dir().join(format!("llm-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "providers = 3").unwrap();
        let err = Config::from_file(&path, &env_of(&[])).unwrap_err();
        assert!(format!("{err:#}").contains("config.toml"), "{err:#}");
        assert_eq!(
            Config::from_file(&dir.join("nope.toml"), &env_of(&[])).unwrap(),
            Config::default()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_detection() {
        let env = env_of(&[
            ("ANTHROPIC_API_KEY", "a-key"),
            ("ANTHROPIC_BASE_URL", "http://claude-code-proxy"),
            ("OPENAI_API_KEY", "o-key"),
            ("OPENAI_BASE_URL", "https://gateway.example.com/v1/"),
            ("OPENROUTER_API_KEY", "  "),
        ]);
        let mut config = Config::default();
        config.add_env_providers(&env, &|| true);
        let summary: Vec<_> = config
            .providers
            .iter()
            .map(|p| {
                (
                    p.id.as_str(),
                    p.kind,
                    p.base_url.as_str(),
                    p.api_key.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "anthropic",
                    ProviderKind::Anthropic,
                    "https://api.anthropic.com",
                    Some("a-key")
                ),
                (
                    "openai",
                    ProviderKind::OpenAi,
                    "https://gateway.example.com/v1",
                    Some("o-key")
                ),
                (
                    "ollama",
                    ProviderKind::OpenAi,
                    "http://localhost:11434/v1",
                    None
                ),
            ]
        );
    }

    #[test]
    fn env_detection_keeps_file_providers_and_skips_probe() {
        let file = "[[providers]]\nid = 'openrouter'\nkind = 'openai'\nbase_url = 'https://x/v1'";
        let env = env_of(&[("OPENROUTER_API_KEY", "r"), ("OLLAMA_HOST", "0.0.0.0")]);
        let mut config = Config::from_toml_str(file, &env).unwrap();
        config.add_env_providers(&env, &|| panic!("must not probe when OLLAMA_HOST is set"));
        assert_eq!(config.providers.len(), 2);
        assert_eq!(config.providers[0].base_url, "https://x/v1");
        assert_eq!(config.providers[0].api_key, None);
        assert_eq!(config.providers[1].base_url, "http://localhost:11434/v1");

        let mut config = Config::default();
        config.add_env_providers(&env_of(&[]), &|| false);
        assert!(config.providers.is_empty());
    }

    #[test]
    fn ollama_host_forms() {
        assert_eq!(ollama_base_url("0.0.0.0"), "http://localhost:11434/v1");
        assert_eq!(
            ollama_base_url("127.0.0.1:9999"),
            "http://127.0.0.1:9999/v1"
        );
        assert_eq!(
            ollama_base_url("https://ollama.lan"),
            "https://ollama.lan/v1"
        );
        assert_eq!(ollama_base_url("http://box:11434/"), "http://box:11434/v1");
        assert_eq!(
            ollama_base_url("http://box:11434/v1"),
            "http://box:11434/v1"
        );
        assert_eq!(ollama_base_url(":8000"), "http://localhost:8000/v1");
    }

    #[test]
    fn splits_model_refs() {
        assert_eq!(
            split_model_ref("openrouter/meta/llama"),
            Some(("openrouter", "meta/llama"))
        );
        assert_eq!(split_model_ref("bare"), None);
    }
}
