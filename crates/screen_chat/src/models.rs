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

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

struct Known {
    prefix: &'static str,
    window: u64,
    max_output: u32,
    price: Option<(f64, f64)>,
}

const fn k(prefix: &'static str, window: u64, max_output: u32, price: Option<(f64, f64)>) -> Known {
    Known {
        prefix,
        window,
        max_output,
        price,
    }
}

const M: u64 = 1_000_000;
const K200: u64 = 200_000;

/// Longest matching prefix wins. Ids are normalised (`.` → `-`, lowercase) on both sides.
const KNOWN: &[Known] = &[
    // Anthropic
    k("claude-fable-5-2", M, 64_000, Some((8., 40.))),
    k("claude-fable-5", M, 64_000, Some((10., 50.))),
    k("claude-mythos-5-2", M, 64_000, Some((8., 40.))),
    k("claude-mythos-5", M, 64_000, Some((10., 50.))),
    k("claude-opus-5-5", M, 64_000, Some((4., 20.))),
    k("claude-opus-5", M, 64_000, Some((5., 25.))),
    k("claude-opus-4-8", M, 64_000, Some((5., 25.))),
    k("claude-opus-4-7", M, 64_000, Some((5., 25.))),
    k("claude-opus-4-6", M, 64_000, Some((5., 25.))),
    k("claude-opus-4-5", K200, 64_000, Some((5., 25.))),
    k("claude-opus-4", K200, 32_000, Some((15., 75.))),
    k("claude-sonnet-5", M, 64_000, Some((2., 10.))),
    k("claude-sonnet-4-6", M, 64_000, Some((3., 15.))),
    k("claude-sonnet-4", K200, 64_000, Some((3., 15.))),
    k("claude-haiku-5-5", M, 64_000, Some((0.1, 0.5))),
    k("claude-haiku-4-5", K200, 64_000, Some((1., 5.))),
    k("claude-3-7-sonnet", K200, 64_000, Some((3., 15.))),
    k("claude-3-5-haiku", K200, 8_192, Some((0.8, 4.))),
    k("claude-3-5-sonnet", K200, 8_192, Some((3., 15.))),
    k("claude", K200, 8_192, None),
    // OpenAI
    k("gpt-5-nano", 400_000, 32_000, Some((0.05, 0.4))),
    k("gpt-5-mini", 400_000, 32_000, Some((0.25, 2.))),
    k("gpt-5", 400_000, 32_000, Some((1.25, 10.))),
    k("gpt-4-1-nano", 1_047_576, 32_768, Some((0.1, 0.4))),
    k("gpt-4-1-mini", 1_047_576, 32_768, Some((0.4, 1.6))),
    k("gpt-4-1", 1_047_576, 32_768, Some((2., 8.))),
    k("gpt-4o-mini", 128_000, 16_384, Some((0.15, 0.6))),
    k("gpt-4o", 128_000, 16_384, Some((2.5, 10.))),
    k("o4-mini", K200, 32_000, Some((1.1, 4.4))),
    k("o3-mini", K200, 32_000, Some((1.1, 4.4))),
    k("o3", K200, 32_000, Some((2., 8.))),
    // Open-weight families commonly served by Ollama / OpenRouter
    k("qwen3-coder", 256_000, 16_384, None),
    k("qwen", 32_768, 8_192, None),
    k("llama3", 128_000, 8_192, None),
    k("deepseek", 128_000, 8_192, None),
    k("gemini", M, 32_000, None),
];

pub const DEFAULT_WINDOW: u64 = 128_000;
pub const DEFAULT_MAX_OUTPUT: u32 = 8_192;

fn normalise(model: &str) -> String {
    // OpenRouter ids carry the vendor (`anthropic/claude-sonnet-4.5`), Ollama a tag (`qwen3:8b`).
    let base = model.rsplit('/').next().unwrap_or(model);
    base.to_lowercase().replace('.', "-")
}

fn known(model: &str) -> Option<&'static Known> {
    let id = normalise(model);
    KNOWN
        .iter()
        .filter(|k| id.starts_with(&k.prefix.replace('.', "-")))
        .max_by_key(|k| k.prefix.len())
}

/// Context window in tokens (default 128k).
pub fn context_window(model: &str) -> u64 {
    known(model).map_or(DEFAULT_WINDOW, |k| k.window)
}

/// `max_tokens` to request.
pub fn max_output(model: &str) -> u32 {
    known(model).map_or(DEFAULT_MAX_OUTPUT, |k| k.max_output)
}

/// Price per MTok; local Ollama models are free; unknown → `None`.
pub fn price(provider: &str, model: &str) -> Option<Price> {
    if provider == "ollama" {
        return Some(Price {
            input: 0.,
            output: 0.,
        });
    }
    known(model)
        .and_then(|k| k.price)
        .map(|(input, output)| Price { input, output })
}

/// Cost of one request in USD. Cache reads bill at 0.1×, cache writes at 1.25× input.
pub fn cost(price: Price, u: &Usage) -> f64 {
    let m = 1_000_000.;
    (u.input_tokens as f64 * price.input
        + u.cache_read_tokens as f64 * price.input * 0.1
        + u.cache_write_tokens as f64 * price.input * 1.25
        + u.output_tokens as f64 * price.output)
        / m
}

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
        assert_eq!(context_window("anthropic/claude-sonnet-4.6"), M);
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
