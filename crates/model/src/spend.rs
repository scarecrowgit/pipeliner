//! Spend: one ledger row per model call, and the sums the dashboard shows.

use serde::{Deserialize, Serialize};

use crate::{Millis, ModelRef, NodeId, RunId, RunnableRef, SessionId, Usage};

/// One model call. The ledger is append-only; every number on Home is a sum over it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageEntry {
    pub at: Millis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    /// Who spent it.
    pub runnable: RunnableRef,
    /// Display name at the time, so history survives renames.
    pub name: String,
    /// Set when the spender was created by another agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<RunnableRef>,
    pub model: ModelRef,
    pub usage: Usage,
    pub cost_usd: f64,
    /// A lump sum carried over from the old app, dated at the session's last change.
    #[serde(default)]
    pub imported: bool,
}

/// A bar of the spend chart: a day or a week.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendBucket {
    /// Local midnight (or Monday midnight) the bucket starts at.
    pub start: Millis,
    pub cost_usd: f64,
}

/// A row of "Spend by agent".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpendByRunnable {
    pub runnable: RunnableRef,
    pub name: String,
    pub cost_usd: f64,
    /// Created by another agent; drawn in violet.
    pub spawned: bool,
}

/// The spend block of the sidebar foot and the Home strip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpendToday {
    pub cost_usd: f64,
    /// `None` when no limit is set.
    pub limit_usd: Option<f64>,
}

impl SpendToday {
    /// At the limit running work finishes and new runs wait.
    pub fn at_limit(&self) -> bool {
        self.limit_usd.is_some_and(|limit| self.cost_usd >= limit)
    }
}
