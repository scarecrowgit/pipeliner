//! Context windows, output caps, prices and request features of known models: how full a
//! context is, what a session cost, and which reasoning and sampling settings a model takes.
//!
//! Models are matched by the longest known prefix of their id, after dropping any gateway
//! prefix (`cc/claude-opus-5-5` reads `claude-opus-5-5`) and normalising case and dots
//! (`claude-sonnet-4.5` reads `claude-sonnet-4-5`).

use model::{ModelRef, Usage};

/// USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

/// Context window of a model that is not in the table.
pub const DEFAULT_WINDOW: u64 = 128_000;
/// Output cap of a model that is not in the table.
pub const DEFAULT_MAX_OUTPUT: u32 = 8_192;

struct Known {
    prefix: &'static str,
    window: u64,
    max_output: u32,
    price: Option<(f64, f64)>,
    traits: Traits,
}

const fn k(prefix: &'static str, window: u64, max_output: u32, price: Option<(f64, f64)>, traits: Traits) -> Known {
    Known { prefix, window, max_output, price, traits }
}

/// How a model takes reasoning and sampling settings; the clients build requests from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Traits {
    /// How the Anthropic Messages API takes reasoning for this model.
    pub thinking: Thinking,
    /// Whether OpenAI-compatible APIs take `reasoning_effort` (or the gateway's equivalent).
    pub effort: bool,
    /// The effort an OpenAI-compatible API gets for `Reasoning::Off`; `None` sends nothing.
    pub effort_off: Option<&'static str>,
    /// Whether a non-default `temperature` is accepted.
    pub temperature: bool,
}

/// Anthropic thinking support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Thinking {
    None,
    /// `{type: "enabled", budget_tokens}`; off unless asked for.
    Budget,
    /// `{type: "adaptive"}` with `output_config.effort`.
    Adaptive {
        off: ThinkingOff,
        /// The model omits thinking text unless asked for `display: "summarized"`.
        summarize: bool,
    },
}

/// What `Reasoning::Off` becomes on an adaptive-thinking model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThinkingOff {
    /// `thinking: {type: "disabled"}`.
    Disable,
    /// `thinking: {type: "between_tools"}`, the only off switch of Claude Sonnet 5.5.
    BetweenTools,
    /// Thinking cannot be turned off; ask for the lowest effort instead.
    LowEffort,
}

const fn adaptive(off: ThinkingOff, summarize: bool, temperature: bool) -> Traits {
    let effort_off = match off {
        ThinkingOff::LowEffort => Some("low"),
        _ => None,
    };
    Traits { thinking: Thinking::Adaptive { off, summarize }, effort: true, effort_off, temperature }
}

/// Fable, Mythos, Opus 5.5: thinking is always on; no sampling settings.
const ALWAYS_THINKS: Traits = adaptive(ThinkingOff::LowEffort, true, false);
/// Opus 5, 4.8, 4.7, Sonnet 5, Haiku 5.5: thinking text omitted by default; no sampling.
const ADAPTIVE: Traits = adaptive(ThinkingOff::Disable, true, false);
/// Sonnet 5.5.
const ADAPTIVE_BETWEEN_TOOLS: Traits = adaptive(ThinkingOff::BetweenTools, true, false);
/// Opus 4.6, Sonnet 4.6: thinking summarised by default, temperature still accepted.
const ADAPTIVE_46: Traits = adaptive(ThinkingOff::Disable, false, true);
/// Claude 3.7 to 4.5: budgeted thinking.
const BUDGET: Traits = Traits { thinking: Thinking::Budget, effort: true, effort_off: None, temperature: true };
/// OpenAI reasoning models that take `minimal` and no temperature.
const GPT5: Traits = Traits { thinking: Thinking::None, effort: true, effort_off: Some("minimal"), temperature: false };
/// OpenAI o-series: always reasons, no temperature.
const O_SERIES: Traits = Traits { thinking: Thinking::None, effort: true, effort_off: Some("low"), temperature: false };
/// gpt-oss (Ollama and gateways): always reasons, sampling is fine.
const GPT_OSS: Traits = Traits { thinking: Thinking::None, effort: true, effort_off: Some("low"), temperature: true };
/// No reasoning setting; sampling is fine. Also what unknown models get.
const PLAIN: Traits = Traits { thinking: Thinking::None, effort: false, effort_off: None, temperature: true };

const M: u64 = 1_000_000;
const K200: u64 = 200_000;
const OUT128: u32 = 128_000;

/// Prefixes are already normalised (lowercase, `-` for `.`).
const KNOWN: &[Known] = &[
    // Anthropic
    k("claude-fable-5-2", M, OUT128, Some((8., 40.)), ALWAYS_THINKS),
    k("claude-fable-5", M, OUT128, Some((10., 50.)), ALWAYS_THINKS),
    k("claude-mythos-5-2", M, OUT128, Some((8., 40.)), ALWAYS_THINKS),
    k("claude-mythos-5", M, OUT128, Some((10., 50.)), ALWAYS_THINKS),
    k("claude-opus-5-5", M, OUT128, Some((4., 20.)), ALWAYS_THINKS),
    k("claude-opus-5", M, OUT128, Some((5., 25.)), ADAPTIVE),
    k("claude-opus-4-8", M, OUT128, Some((5., 25.)), ADAPTIVE),
    k("claude-opus-4-7", M, OUT128, Some((5., 25.)), ADAPTIVE),
    k("claude-opus-4-6", M, OUT128, Some((5., 25.)), ADAPTIVE_46),
    k("claude-opus-4-5", K200, 64_000, Some((5., 25.)), BUDGET),
    k("claude-opus-4", K200, 32_000, Some((15., 75.)), BUDGET),
    k("claude-sonnet-5-5", M, OUT128, Some((2., 10.)), ADAPTIVE_BETWEEN_TOOLS),
    k("claude-sonnet-5", M, OUT128, Some((2., 10.)), ADAPTIVE),
    k("claude-sonnet-4-6", M, OUT128, Some((3., 15.)), ADAPTIVE_46),
    k("claude-sonnet-4", K200, 64_000, Some((3., 15.)), BUDGET),
    k("claude-haiku-5-5", M, OUT128, Some((0.1, 0.5)), ADAPTIVE),
    k("claude-haiku-4-5", K200, 64_000, Some((1., 5.)), BUDGET),
    k("claude-3-7-sonnet", K200, 64_000, Some((3., 15.)), BUDGET),
    k("claude-3-5-haiku", K200, 8_192, Some((0.8, 4.)), PLAIN),
    k("claude-3-5-sonnet", K200, 8_192, Some((3., 15.)), PLAIN),
    k("claude", K200, 8_192, None, PLAIN),
    // OpenAI
    k("gpt-5-nano", 400_000, 32_000, Some((0.05, 0.4)), GPT5),
    k("gpt-5-mini", 400_000, 32_000, Some((0.25, 2.)), GPT5),
    k("gpt-5", 400_000, 32_000, Some((1.25, 10.)), GPT5),
    k("gpt-4-1-nano", 1_047_576, 32_768, Some((0.1, 0.4)), PLAIN),
    k("gpt-4-1-mini", 1_047_576, 32_768, Some((0.4, 1.6)), PLAIN),
    k("gpt-4-1", 1_047_576, 32_768, Some((2., 8.)), PLAIN),
    k("gpt-4o-mini", 128_000, 16_384, Some((0.15, 0.6)), PLAIN),
    k("gpt-4o", 128_000, 16_384, Some((2.5, 10.)), PLAIN),
    k("o4-mini", K200, 32_000, Some((1.1, 4.4)), O_SERIES),
    k("o3-mini", K200, 32_000, Some((1.1, 4.4)), O_SERIES),
    k("o3", K200, 32_000, Some((2., 8.)), O_SERIES),
    // Open-weight families commonly served by Ollama and OpenRouter
    k("gpt-oss", 128_000, 32_768, None, GPT_OSS),
    k("qwen3-coder", 256_000, 16_384, None, PLAIN),
    k("qwen", 32_768, 8_192, None, PLAIN),
    k("llama3", 128_000, 8_192, None, PLAIN),
    k("deepseek", 128_000, 8_192, None, PLAIN),
    k("gemini", M, 32_000, None, PLAIN),
];

fn known(model: &ModelRef) -> Option<&'static Known> {
    let id = model.short().to_lowercase().replace('.', "-");
    KNOWN.iter().filter(|k| id.starts_with(k.prefix)).max_by_key(|k| k.prefix.len())
}

/// Context window in tokens.
pub fn context_window(model: &ModelRef) -> u64 {
    known(model).map_or(DEFAULT_WINDOW, |k| k.window)
}

/// The largest output the model accepts; what a request with `max_tokens: 0` asks for.
pub fn max_output(model: &ModelRef) -> u32 {
    known(model).map_or(DEFAULT_MAX_OUTPUT, |k| k.max_output)
}

/// How the model takes reasoning and sampling settings.
pub(crate) fn traits(model: &ModelRef) -> Traits {
    known(model).map_or(PLAIN, |k| k.traits)
}

/// Price per million tokens. Models served by the `ollama` provider are free; unknown
/// models have no price.
pub fn price(model: &ModelRef) -> Option<Price> {
    if model.provider == "ollama" {
        return Some(Price { input: 0., output: 0. });
    }
    known(model).and_then(|k| k.price).map(|(input, output)| Price { input, output })
}

/// Cost of `usage` in USD. Cache reads bill at 0.1× the input price, cache writes at 1.25×.
pub fn cost(price: Price, usage: &Usage) -> f64 {
    (usage.input as f64 * price.input
        + usage.cache_read as f64 * price.input * 0.1
        + usage.cache_write as f64 * price.input * 1.25
        + usage.output as f64 * price.output)
        / 1_000_000.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str) -> ModelRef {
        ModelRef::parse(text)
    }

    #[test]
    fn longest_prefix_wins_after_stripping_gateway_prefixes() {
        assert_eq!(price(&m("nr/cc/claude-opus-5-5")), Some(Price { input: 4., output: 20. }));
        assert_eq!(price(&m("anthropic/claude-opus-5")), Some(Price { input: 5., output: 25. }));
        assert_eq!(price(&m("anthropic/claude-opus-4-1-20250805")), Some(Price { input: 15., output: 75. }));
        assert_eq!(context_window(&m("openrouter/anthropic/claude-sonnet-4.5")), K200);
        assert_eq!(max_output(&m("anthropic/claude-sonnet-4.6")), OUT128);
        assert_eq!(price(&m("openai/gpt-5-mini-2025")), Some(Price { input: 0.25, output: 2. }));
        assert_eq!(price(&m("openai/GPT-4o")), Some(Price { input: 2.5, output: 10. }));
        assert_eq!(price(&m("anthropic/claude-instant")), None);
        assert_eq!(context_window(&m("anthropic/claude-instant")), K200);
    }

    #[test]
    fn unknown_models_get_defaults_and_ollama_is_free() {
        let unknown = m("gw/mystery-7b");
        assert_eq!(context_window(&unknown), DEFAULT_WINDOW);
        assert_eq!(max_output(&unknown), DEFAULT_MAX_OUTPUT);
        assert_eq!(price(&unknown), None);
        assert_eq!(price(&m("ollama/qwen3:8b")), Some(Price { input: 0., output: 0. }));
        assert_eq!(context_window(&m("ollama/qwen3-coder:30b")), 256_000);
    }

    #[test]
    fn traits_follow_the_same_prefixes() {
        assert_eq!(traits(&m("nr/cc/claude-opus-5-5")), ALWAYS_THINKS);
        assert_eq!(traits(&m("anthropic/claude-sonnet-5-5")), ADAPTIVE_BETWEEN_TOOLS);
        assert_eq!(traits(&m("anthropic/claude-sonnet-5")), ADAPTIVE);
        assert_eq!(traits(&m("anthropic/claude-sonnet-4-6")), ADAPTIVE_46);
        assert_eq!(traits(&m("anthropic/claude-haiku-4-5")), BUDGET);
        assert_eq!(traits(&m("openai/gpt-5-mini")), GPT5);
        assert_eq!(traits(&m("ollama/gpt-oss:20b")), GPT_OSS);
        assert_eq!(traits(&m("gw/mystery-7b")), PLAIN);
        assert!(!traits(&m("openai/o3")).temperature);
        assert!(traits(&m("openai/gpt-4o")).temperature);
    }

    #[test]
    fn cost_bills_cache_reads_and_writes() {
        let usage = Usage { input: 1_000_000, output: 1_000_000, cache_read: 1_000_000, cache_write: 1_000_000 };
        let c = cost(Price { input: 4., output: 20. }, &usage);
        assert!((c - (4. + 20. + 0.4 + 5.)).abs() < 1e-9, "{c}");
        assert_eq!(cost(Price { input: 0., output: 0. }, &usage), 0.);
    }
}
