//! Context windows, output caps and prices of known models, matched by model-id prefix.

use crate::Usage;

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

