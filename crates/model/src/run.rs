//! Pipeline runs: one item moving through a deployed graph.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{BlobId, IssueKey, Millis, NodeId, PipelineId, RunId, SessionId, Usage};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a free slot or for a spend limit to reset.
    #[default]
    Queued,
    Running,
    /// A review is open in the Inbox.
    WaitingReview,
    /// An agent asked a person something, or a budget was hit.
    WaitingAnswer,
    Done,
    Failed,
    Cancelled,
    /// The app closed while it was running. Resuming is explicit.
    Interrupted,
}

impl RunState {
    /// Still owes work or an answer.
    pub fn is_open(self) -> bool {
        !matches!(self, RunState::Done | RunState::Failed | RunState::Cancelled)
    }

    pub fn needs_person(self) -> bool {
        matches!(self, RunState::WaitingReview | RunState::WaitingAnswer)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub pipeline: PipelineId,
    /// The deployed version the run is pinned to; 0 for a test run of the draft.
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueKey>,
    /// "Run 2 on this issue".
    #[serde(default)]
    pub number: u32,
    pub title: String,
    /// The item's text handed to the first agents: the issue body, or the manual input.
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub state: RunState,
    /// A test run from the editor: real agents in throwaway worktrees, simulated merges and PRs.
    #[serde(default)]
    pub test: bool,
    pub started: Millis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<Millis>,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// Every node instance that exists so far, in creation order.
    #[serde(default)]
    pub nodes: Vec<NodeInstance>,
}

impl Run {
    /// Rough progress for a board card: finished instances over all known ones.
    pub fn progress(&self) -> f32 {
        let total = self.nodes.iter().filter(|n| n.status != NodeStatus::Skipped).count();
        if total == 0 {
            return 0.0;
        }
        let done = self.nodes.iter().filter(|n| n.status == NodeStatus::Done).count();
        done as f32 / total as f32
    }

    pub fn instance(&self, node: &NodeId, variant: u32) -> Option<&NodeInstance> {
        self.nodes.iter().filter(|n| &n.node == node && n.variant == variant).max_by_key(|n| n.attempt)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatus {
    #[default]
    Pending,
    Running,
    /// A review node waiting for a person, or an agent waiting for an answer.
    Waiting,
    Done,
    Failed,
    /// Every way into it was a branch not taken.
    Skipped,
    Cancelled,
    Interrupted,
}

/// What kind of work an instance is, for timeline colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    #[default]
    Agent,
    /// Created by another agent.
    Spawned,
    /// Waiting for a person.
    Waiting,
    /// Source, router, file or action.
    Step,
}

/// One execution of one node: a node, one of its variants, one attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeInstance {
    pub node: NodeId,
    /// Node name at the time of the run, so history survives edits to the graph.
    pub name: String,
    /// 0 outside a variant scope, otherwise 1-based: variant 1 is "A".
    #[serde(default)]
    pub variant: u32,
    /// Starts at 1; "Send back" adds one.
    #[serde(default = "first_attempt")]
    pub attempt: u32,
    #[serde(default)]
    pub status: NodeStatus,
    #[serde(default)]
    pub kind: SpanKind,
    /// The out-port that fired when it finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<Millis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<Millis>,
    #[serde(default)]
    pub cost_usd: f64,
    /// One or two sentences: the agent's own summary, a router's reason, a command's result.
    #[serde(default)]
    pub summary: String,
    /// The instance that created it, for spawned sub-agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<NodeId>,
}

fn first_attempt() -> u32 {
    1
}

impl NodeInstance {
    /// "A", "B", "C" for variants 1, 2, 3; empty outside a variant scope.
    pub fn letter(&self) -> String {
        variant_letter(self.variant)
    }
}

/// "A" for 1, "B" for 2; empty for 0.
pub fn variant_letter(variant: u32) -> String {
    match variant {
        0 => String::new(),
        n => char::from_u32('A' as u32 + (n - 1) % 26).map(String::from).unwrap_or_default(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    #[default]
    Info,
    /// A tool call line: "read src/lib.rs".
    Tool,
    Warn,
    Error,
}

/// One line of a run's log, read from the store in pages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLogLine {
    pub seq: u64,
    pub at: Millis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    #[serde(default)]
    pub variant: u32,
    #[serde(default)]
    pub level: LogLevel,
    pub text: String,
}

/// A before/after picture of one rendered file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Render {
    /// The file that was rendered, relative to its workdir.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<BlobId>,
    pub after: BlobId,
    pub width: u32,
    pub height: u32,
}

/// Numbers of the editor's test-run strip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunStats {
    pub agents_running: u32,
    pub agents_max: u32,
    pub spawned: u32,
    pub waiting_variants: u32,
    pub cost_usd: f64,
    pub budget_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_letters() {
        assert_eq!(variant_letter(0), "");
        assert_eq!(variant_letter(1), "A");
        assert_eq!(variant_letter(3), "C");
    }

    #[test]
    fn open_and_waiting_states() {
        assert!(RunState::WaitingReview.is_open());
        assert!(RunState::WaitingReview.needs_person());
        assert!(RunState::Interrupted.is_open());
        assert!(!RunState::Done.is_open());
    }
}
