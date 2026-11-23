//! The Tool trait and registry, sandboxed file tools, bash, permission rules.
//!
//! | Module | What it gives |
//! | --- | --- |
//! | [`api`] | The contract: [`Tool`], [`ToolContext`], [`ToolOutput`], [`ToolRegistry`] |
//! | [`builtin`] | The six built-in tools by access level, and tool grants |
//! | [`sandbox`] | Resolving a model-supplied path inside the workdir |
//! | [`diff`] | Line diffs as [`model::FileDiff`] and as unified text |
//! | [`permission`] | The rule engine behind "Allow once / Always allow" |
//! | [`workspace`] | Facts about a folder for the agent loop: files, notes, a summary |

pub mod api;
pub mod builtin;
pub mod diff;
pub mod permission;
pub mod sandbox;
pub mod workspace;

mod args;
mod bash;
mod edit;
mod grep;
mod read;
mod text;

#[cfg(test)]
mod tests;

pub use api::*;
pub use bash::Bash;
pub use builtin::{apply_grants, builtin, names};
pub use edit::{EditFile, WriteFile};
pub use grep::Grep;
pub use read::{ListDir, ReadFile};
