//! User configuration: model providers, default model, workspace rules (prompt layer 1),
//! prices for cost estimates, skill folders. Stored at `~/.config/pipeliner/config.toml`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// `provider/model`.
    #[serde(default)]
    pub default_model: String,
    /// Model for cheap helper calls (titles, summaries). Falls back to `default_model`.
    #[serde(default)]
    pub fast_model: String,
    #[serde(default)]
    pub providers: Vec<Provider>,
    /// Models listed in pickers, `provider/model`. Empty = ask the provider.
    #[serde(default)]
    pub models: Vec<String>,
    /// Prompt layer 1: language, voice, house rules. Sent with every call.
    #[serde(default)]
    pub rules: String,
    /// Spend shown against these in the sidebar (display only; limits are policies).
    #[serde(default)]
    pub daily_budget_usd: f64,
    #[serde(default)]
    pub monthly_budget_usd: f64,
    /// USD per million tokens, keyed by a substring of the model id (e.g. "opus").
    #[serde(default = "default_prices")]
    pub prices: BTreeMap<String, Price>,
    /// Folders with `<name>/SKILL.md` skills.
    #[serde(default = "default_skill_dirs")]
    pub skill_dirs: Vec<String>,
    /// Max agent tasks running at once.
    #[serde(default = "default_slots")]
    pub parallel_slots: u32,
    /// Command used to render HTML to PNG. `{in}`, `{out}`, `{w}`, `{h}` are replaced.
    /// Empty = find chromium on PATH, else `nix-shell -p chromium`.
    #[serde(default)]
    pub render_command: String,
    /// The repository of this app, for self-improvement pipelines. Empty = the folder it was built from.
    #[serde(default)]
    pub self_repo: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    pub id: String,
    /// Only `openai` (OpenAI-compatible chat completions) for now.
    #[serde(default = "openai")]
    pub kind: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Read the key from this environment variable when `api_key` is empty.
    #[serde(default)]
    pub api_key_env: String,
}

impl Provider {
    pub fn key(&self) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }
        if !self.api_key_env.is_empty() {
            return std::env::var(&self.api_key_env).unwrap_or_default();
        }
        String::new()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

fn openai() -> String {
    "openai".into()
}
fn default_slots() -> u32 {
    6
}

pub fn default_prices() -> BTreeMap<String, Price> {
    let mut m = BTreeMap::new();
    m.insert(
        "haiku".into(),
        Price {
            input: 1.0,
            output: 5.0,
        },
    );
    m.insert(
        "sonnet".into(),
        Price {
            input: 3.0,
            output: 15.0,
        },
    );
    m.insert(
        "opus".into(),
        Price {
            input: 5.0,
            output: 25.0,
        },
    );
    m
}

pub fn default_skill_dirs() -> Vec<String> {
    vec![
        "~/.config/pipeliner/skills".into(),
        "~/.claude/skills".into(),
    ]
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_model: String::new(),
            fast_model: String::new(),
            providers: vec![],
            models: vec![],
            rules: String::new(),
            daily_budget_usd: 0.0,
            monthly_budget_usd: 0.0,
            prices: default_prices(),
            skill_dirs: default_skill_dirs(),
            parallel_slots: default_slots(),
            render_command: String::new(),
            self_repo: String::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("config.toml")
    }

    pub fn load() -> Config {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        // Atomic and private: the file holds API keys.
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }

    pub fn is_configured(&self) -> bool {
        !self.providers.is_empty() && !self.default_model.is_empty()
    }

    /// Splits `provider/model` into the provider and the model id it serves.
    pub fn resolve(&self, model: &str) -> Option<(&Provider, String)> {
        let model = if model.is_empty() {
            self.default_model.as_str()
        } else {
            model
        };
        if let Some((pid, rest)) = model.split_once('/') {
            if let Some(p) = self.providers.iter().find(|p| p.id == pid) {
                return Some((p, rest.to_string()));
            }
        }
        self.providers.first().map(|p| (p, model.to_string()))
    }

    pub fn fast(&self) -> &str {
        if self.fast_model.is_empty() {
            &self.default_model
        } else {
            &self.fast_model
        }
    }

    /// Estimated USD cost of a call.
    pub fn cost(&self, model: &str, tokens_in: u64, tokens_out: u64) -> f64 {
        let lower = model.to_lowercase();
        let price = self
            .prices
            .iter()
            .filter(|(k, _)| lower.contains(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, p)| *p);
        match price {
            Some(p) => (tokens_in as f64 * p.input + tokens_out as f64 * p.output) / 1_000_000.0,
            None => 0.0,
        }
    }

    /// Short display name: `nr/cc/claude-sonnet-5-5` -> `Sonnet 5.5`.
    pub fn short_model(model: &str) -> String {
        let last = model.rsplit('/').next().unwrap_or(model);
        let s = last.strip_prefix("claude-").unwrap_or(last);
        let mut parts = s.split('-');
        let Some(name) = parts.next() else {
            return last.to_string();
        };
        let ver: Vec<&str> = parts
            .take_while(|p| p.chars().all(|c| c.is_ascii_digit()))
            .collect();
        let mut out = String::new();
        let mut cs = name.chars();
        if let Some(f) = cs.next() {
            out.extend(f.to_uppercase());
            out.push_str(cs.as_str());
        }
        if !ver.is_empty() {
            out.push(' ');
            out.push_str(&ver.join("."));
        }
        out
    }

    /// Imports providers and default model from a Harness v1 config (`~/.config/harness/config.toml`).
    pub fn import_harness_v1(&mut self) -> Result<bool> {
        let Some(home) = dirs::home_dir() else {
            return Ok(false);
        };
        let p = home.join(".config/harness/config.toml");
        let Ok(s) = std::fs::read_to_string(&p) else {
            return Ok(false);
        };
        #[derive(Deserialize)]
        struct V1 {
            #[serde(default)]
            default_model: String,
            #[serde(default)]
            providers: Vec<Provider>,
        }
        let v1: V1 = toml::from_str(&s)?;
        let mut changed = false;
        for prov in v1.providers {
            if !self.providers.iter().any(|p| p.id == prov.id) {
                self.providers.push(prov);
                changed = true;
            }
        }
        if self.default_model.is_empty() && !v1.default_model.is_empty() {
            self.default_model = v1.default_model;
            changed = true;
        }
        Ok(changed)
    }
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = dirs::home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_names() {
        assert_eq!(Config::short_model("nr/cc/claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(Config::short_model("gpt-4o"), "Gpt");
    }
    #[test]
    fn cost() {
        let c = Config::default();
        assert!((c.cost("nr/cc/claude-opus-5-5", 1_000_000, 0) - 5.0).abs() < 1e-9);
    }
}
