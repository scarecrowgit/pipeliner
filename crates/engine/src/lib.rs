//! The facade over the domain: worker threads, slots, commands, the typed event bus, recovery.
//!
//! [`Engine`] implements [`Api`] over the store, the model client, the agent loop, the catalog,
//! git and the MCP supervisor. It has no UI: the GUI drives it through `hub`, the CLI directly.
//! Every chat turn runs on a thread of its own, behind a slot limit taken from the settings.
//!
//! | Module | What it does |
//! | --- | --- |
//! | [`api`] | The contract (owned by the lead) |
//! | `engine` | [`Engine`]: construction, recovery, shutdown, shared helpers |
//! | `bus` | The event bus behind [`Api::subscribe`] |
//! | `live` | Running sessions, their queues and cancel tokens; the slot limit |
//! | `host`, `gate`, `sink` | What the agent loop is given: storage, permissions, events |
//! | `chat` | Chat sessions: commands, queries, turns, exports, slash commands |
//! | `workdir` | Folders, worktrees and clones for sessions |
//! | `mcp` | The MCP supervisors, started on first use |
//! | `pipelines` | Pipeline runs: drivers, the node runner's services, the observer, recovery |
//! | `runs`, `reviews` | [`api::RunApi`] and [`api::InboxApi`] over the drivers and the store |
//! | `issues`, `catalog`, `ops` | The other parts of the API |

pub mod api;
mod bus;
mod catalog;
mod chat;
mod engine;
mod gate;
mod host;
mod issues;
mod live;
mod mcp;
mod ops;
mod pipelines;
mod reviews;
mod runs;
mod sink;
mod wake;
mod workdir;

#[cfg(test)]
mod tests;

pub use api::Api;
pub use engine::{Engine, EngineOptions, NOT_WIRED};
pub use store::import_v1::ImportReport;

/// The receiving end of [`Api::subscribe`], for callers that do not depend on the channel
/// crate themselves.
pub mod events {
    pub use async_channel::{Receiver, TryRecvError};
}

/// Pieces for testing code that drives an [`Engine`] without a real model.
pub mod testing {
    pub use llm::{Reply, ScriptedLlm};
}
