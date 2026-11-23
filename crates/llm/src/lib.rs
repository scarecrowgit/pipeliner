//! Model clients: Anthropic Messages and OpenAI-compatible streaming, pricing, a scripted fake.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`api`] | The contract: [`LlmClient`], [`ChatRequest`], [`StreamEvent`], [`LlmError`] |
//! | [`config`] | The provider file (`config.toml`) plus providers found in the environment |
//! | [`client`] | [`Providers`], the real client that routes by provider id |
//! | [`pricing`] | Context windows, output caps and prices of known models |
//! | [`scripted`] | [`ScriptedLlm`], the fake other crates test against |

pub mod api;
mod anthropic;
pub mod client;
pub mod config;
mod http;
mod openai;
pub mod pricing;
pub mod scripted;
mod sse;
#[cfg(test)]
mod testing;
mod tool_input;

pub use api::*;
pub use client::Providers;
pub use config::{Config, ProviderConfig, ProviderKind};
pub use pricing::Price;
pub use scripted::{Reply, ScriptedLlm};
