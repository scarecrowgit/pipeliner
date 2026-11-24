//! Reviews: the variants a pipeline hands to a person, and what the person decided.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{FileChange, IssueKey, Millis, NodeId, Render, ReviewId, RunId, SessionId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    #[default]
    Open,
    Shipped,
    Skipped,
    SentBack,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub id: ReviewId,
    pub run: RunId,
    /// The review node that opened it.
    pub node: NodeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueKey>,
    pub title: String,
    #[serde(default)]
    pub state: ReviewState,
    pub created: Millis,
    /// The path the item took, for the breadcrumb: `Issues › Triage · rule 1 › Fixer ×3 › You`.
    #[serde(default)]
    pub provenance: Vec<String>,
    /// Name of the agent whose work is reviewed.
    #[serde(default)]
    pub agent: String,
    pub variants: Vec<Variant>,
    /// Rendered mockups, when the pipeline has a render action before the review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renders: Vec<Render>,
    /// Notes from earlier "Send back" rounds, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feedback: Vec<ReviewNote>,
    /// The variant that shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNote {
    pub at: Millis,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub passed: u32,
    pub total: u32,
}

impl TestResult {
    pub fn failed(self) -> u32 {
        self.total.saturating_sub(self.passed)
    }
}

/// One column of the comparison table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    /// 1-based; shown as A, B, C.
    pub index: u32,
    pub name: String,
    /// The agent's own summary of what it did.
    pub approach: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    pub added: u32,
    pub removed: u32,
    /// Changed files with line counts; the diff itself is read on demand.
    #[serde(default)]
    pub files: Vec<FileChange>,
    /// Parsed counts when the tests command says how to read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestResult>,
    /// Pass or fail of the tests command when counts are not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_ok: Option<bool>,
    #[serde(default)]
    pub secs: u64,
    #[serde(default)]
    pub cost_usd: f64,
    /// Rendered mockups of this variant, when a render action ran inside its scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renders: Vec<Render>,
}

/// Where a sent-back round starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum StartFrom {
    /// Keep this variant's branch and build on it.
    Variant { index: u32 },
    /// Every variant gets the note and tries again.
    All,
    /// New branches; the old variants are archived.
    Over,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ReviewDecision {
    Ship { variant: u32 },
    Skip,
    SendBack { note: String, start: StartFrom, variants: u32 },
}
