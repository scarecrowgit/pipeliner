//! Model selection state plus small lookup tables: context windows, output caps and
//! prices, matched by model-id prefix.

use llm::Usage;

/// A selected model: `provider/model`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn parse(s: &str) -> Option<Self> {
        let (p, m) = llm::config::split_model_ref(s)?;
        (!p.is_empty() && !m.is_empty()).then(|| Self {
            provider: p.to_string(),
            model: m.to_string(),
        })
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// Model list of one provider, fetched in the background.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderModels {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

pub use llm::pricing::{
    context_window, cost, max_output, price,
};

/// Tokens the request occupied in the context window (prompt incl. cache + output).
pub fn context_used(u: &Usage) -> u64 {
    u.input_tokens + u.cache_read_tokens + u.cache_write_tokens + u.output_tokens
}

/// `950`, `12.3k`, `1.2M`.
pub fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1_000.;
        if k < 100. {
            format!("{k:.1}k")
        } else {
            format!("{k:.0}k")
        }
    } else {
        let m = n as f64 / 1_000_000.;
        if m.fract() == 0. {
            format!("{m:.0}M")
        } else {
            format!("{m:.1}M")
        }
    }
}

pub fn fmt_cost(usd: f64) -> String {
    if usd > 0. && usd < 0.01 {
        "<$0.01".into()
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::pricing::{DEFAULT_MAX_OUTPUT, DEFAULT_WINDOW, Price};

    #[test]
    fn longest_prefix_wins() {
        assert_eq!(price("anthropic", "claude-opus-5-5").unwrap().input, 4.);
        assert_eq!(price("anthropic", "claude-opus-5").unwrap().input, 5.);
        assert_eq!(
            price("anthropic", "claude-opus-4-1-20250805")
                .unwrap()
                .input,
            15.
        );
        assert_eq!(
            price("openai", "gpt-4o-mini-2024-07-18").unwrap().input,
            0.15
        );
        assert_eq!(price("openai", "gpt-4o").unwrap().input, 2.5);
    }

    #[test]
    fn normalises_vendor_and_dots() {
        assert_eq!(context_window("anthropic/claude-sonnet-4.6"), 1_000_000);
        assert_eq!(
            price("openrouter", "openai/gpt-4.1-mini").unwrap().output,
            1.6
        );
    }

    #[test]
    fn unknown_models_use_defaults() {
        assert_eq!(context_window("mystery-model"), DEFAULT_WINDOW);
        assert_eq!(max_output("mystery-model"), DEFAULT_MAX_OUTPUT);
        assert!(price("openai", "mystery-model").is_none());
        assert!(price("anthropic", "claude-next").is_none());
        assert_eq!(price("ollama", "anything").unwrap().input, 0.);
    }

    #[test]
    fn cost_and_formatting() {
        let u = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_read_tokens: 1_000_000,
            cache_write_tokens: 0,
        };
        let c = cost(
            Price {
                input: 3.,
                output: 15.,
            },
            &u,
        );
        assert!((c - (3. + 1.5 + 0.3)).abs() < 1e-9);
        assert_eq!(context_used(&u), 2_100_000);
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_340), "12.3k");
        assert_eq!(fmt_tokens(200_000), "200k");
        assert_eq!(fmt_tokens(1_000_000), "1M");
        assert_eq!(fmt_cost(0.004), "<$0.01");
        assert_eq!(fmt_cost(0.237), "$0.24");
    }

    #[test]
    fn model_ref_parsing() {
        let r = ModelRef::parse("openrouter/anthropic/claude-sonnet-4.5").unwrap();
        assert_eq!(r.provider, "openrouter");
        assert_eq!(r.model, "anthropic/claude-sonnet-4.5");
        assert_eq!(r.label(), "openrouter/anthropic/claude-sonnet-4.5");
        assert!(ModelRef::parse("nomodel").is_none());
    }
}
