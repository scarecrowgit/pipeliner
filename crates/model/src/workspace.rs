//! Projects, runtimes and app-wide settings.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ModelRef, ProjectId};

/// A named group of local repos with its own issue keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Prefix of its issue keys: `HAR` gives `HAR-12`.
    pub key_prefix: String,
    /// Checkout paths of its repos.
    #[serde(default)]
    pub repos: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// This machine.
    #[default]
    Local,
    /// A sandbox VM. Not available yet.
    Cloud,
    /// Another machine. Not available yet.
    Remote,
}

/// A place agents run. Only the local runtime is real; the others are shown as coming soon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    /// Host name for the local one.
    pub name: String,
    pub kind: RuntimeKind,
    /// False for runtimes that are drawn but cannot be picked.
    pub available: bool,
    pub online: bool,
    pub slots_total: u32,
    pub slots_busy: u32,
    /// What it is doing, in a few words.
    #[serde(default)]
    pub note: String,
}

/// App-wide settings the user changes from the UI. Provider keys and MCP servers stay in
/// `config.toml`; this is everything else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Agents working at the same time across chats and runs.
    #[serde(default = "default_slots")]
    pub slots: u32,
    /// Spend that stops new work for the day; `None` for no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_limit_usd: Option<f64>,
    /// Model for new chats when the config file names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<ModelRef>,
    /// The project shown on start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_project: Option<ProjectId>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// Share of the context window at which a session compacts itself.
    #[serde(default = "default_compact_at")]
    pub compact_at: f32,
}

fn default_slots() -> u32 {
    4
}

fn default_compact_at() -> f32 {
    0.9
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            slots: default_slots(),
            daily_limit_usd: None,
            default_model: None,
            current_project: None,
            sidebar_collapsed: false,
            compact_at: default_compact_at(),
        }
    }
}
