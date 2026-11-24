//! The stored state of one run's machine.

use std::collections::BTreeMap;

use model::{NodeId, Review, ReviewNote, Run, Usage};
use serde::{Deserialize, Serialize};

use crate::api::{InstanceKey, Item, NodeOutput, Retry};

/// Everything the machine knows about a run. Serialise it after every step; hand it to
/// [`Machine::restore`](super::Machine::restore) to continue after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The run as the rest of the app sees it; `nodes` is the full history of instances.
    pub run: Run,
    /// What flows through the run; handed to every job.
    pub item: Item,
    /// Executor-side facts about each instance, parallel to `run.nodes`.
    pub meta: Vec<Meta>,
    /// Every review this run opened, oldest first.
    #[serde(default)]
    pub reviews: Vec<ReviewRound>,
    /// The run's spending cap as it is now (the graph's limit plus what a person added).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Variant indices of a scope opener when a send-back changed them from the node's own count.
    #[serde(default)]
    pub scopes: BTreeMap<NodeId, Vec<u32>>,
    /// The attempt new instances of a node get; 1 when absent.
    #[serde(default)]
    pub attempts: BTreeMap<NodeId, u32>,
    /// How the instances of the next attempt begin; taken when they are created.
    #[serde(default)]
    pub retries: Vec<PlannedRetry>,
    /// Set while the run is stopped and may be resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halt: Option<Halt>,
    /// The item was let in; false while the run is only queued.
    #[serde(default)]
    pub started: bool,
    /// The "budget reached" line was logged for the current cap.
    #[serde(default)]
    pub budget_noted: bool,
    /// Sequence number of the last log line.
    #[serde(default)]
    pub log_seq: u64,
    /// Counter behind [`Meta::order`].
    #[serde(default)]
    pub order: u64,
}

/// Why a run is stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Halt {
    /// A person cancelled it.
    Cancelled,
    /// A node failed outside a variant scope, or every variant of a scope did.
    Failed,
    /// The app closed, or the driver was stopped, while jobs were running.
    Interrupted,
}

/// Why an instance waits for a person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "hold", rename_all = "snake_case")]
pub enum Hold {
    /// The agent asked this.
    Question { text: String },
    /// The run's or the node's budget is used up.
    Budget,
}

/// Ties a spawned sub-agent's instance to the job that reported it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnLink {
    /// The runner's id of the sub-agent.
    pub id: String,
    pub parent: InstanceKey,
}

/// What the executor keeps about one instance beyond its [`model::NodeInstance`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    /// A later attempt replaced it; it is history and no longer part of the item's path.
    #[serde(default)]
    pub superseded: bool,
    /// A job was started for it and has not reported its end.
    #[serde(default)]
    pub in_flight: bool,
    /// What its last job left behind; final once the instance is done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<NodeOutput>,
    /// Why it waits for a person; kept while a stopped run is parked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
    /// The person's answer, handed to the next job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// How this attempt begins, when a review sent work back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// The next job continues earlier work of this instance and gets its session.
    #[serde(default)]
    pub resume: bool,
    /// The node's cap for this instance, raised together with the run's budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_budget: Option<f64>,
    /// Spend the job in flight has reported in notes, to not count its output twice.
    #[serde(default)]
    pub job_cost: f64,
    /// Tokens the job in flight has reported in notes.
    #[serde(default)]
    pub job_usage: Usage,
    /// Position in the order instances finished; 0 until then.
    #[serde(default)]
    pub order: u64,
    /// For a shipped review: the instance variant whose outputs continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked: Option<u32>,
    /// Set on instances of spawned sub-agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnLink>,
    /// Its worktree and branch were handed to `discard`.
    #[serde(default)]
    pub discarded: bool,
    /// Discard what its cancelled job reports when it returns.
    #[serde(default)]
    pub discard_late: bool,
}

/// A review the run opened, with what the executor needs to act on its decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewRound {
    /// The review as the Inbox shows it, with its present state.
    pub review: Review,
    /// The review instance's attempt.
    pub attempt: u32,
    /// The instance variant behind each of `review.variants`, in the same order.
    pub lanes: Vec<u32>,
    /// The node whose work is reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<NodeId>,
    /// The node that opened the variant scope this review closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener: Option<NodeId>,
    /// What the person wrote when sending it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<ReviewNote>,
}

impl ReviewRound {
    /// The instance variant shown under review index `index`.
    pub fn lane_of(&self, index: u32) -> Option<u32> {
        self.review.variants.iter().position(|variant| variant.index == index).and_then(|pos| self.lanes.get(pos)).copied()
    }
}

/// The [`Retry`] waiting for an instance of the next attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedRetry {
    /// The node and variant whose next instance takes it.
    pub node: NodeId,
    pub variant: u32,
    pub retry: Retry,
}
