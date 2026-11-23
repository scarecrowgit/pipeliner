//! The Inbox: everything that waits for a person or reports finished work.

use serde::{Deserialize, Serialize};

use crate::{InboxId, IssueKey, Millis, RequestId, ReviewId, RunId, SessionId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InboxKind {
    /// Variants or renders to pick from.
    Review { review: ReviewId },
    /// An agent's summary of finished work.
    Report,
    /// An agent asked something and waits for the answer.
    Question { request: RequestId },
    /// A run stopped: an error, a budget, an interrupted app.
    Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    NeedsReview,
    Blocked,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: InboxId,
    #[serde(flatten)]
    pub kind: InboxKind,
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// Who it is from, as shown under the title: "Fixer · 3 variants".
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<IssueKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    pub created: Millis,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub archived: bool,
}

impl InboxItem {
    /// Counts toward "N waiting": a person has to do something.
    pub fn needs_person(&self) -> bool {
        !self.archived
            && match self.kind {
                InboxKind::Review { .. } | InboxKind::Question { .. } | InboxKind::Failure => true,
                InboxKind::Report => !self.read,
            }
    }
}

/// The status of a pull request, read from the host on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub url: String,
    pub number: u64,
    pub title: String,
    /// `open`, `merged`, `closed`, `draft`.
    pub state: String,
    /// `passing`, `failing`, `pending`, or empty when there are none.
    pub checks: String,
    /// `approved`, `changes_requested`, `review_required`, or empty.
    pub review: String,
    pub added: u32,
    pub removed: u32,
    pub head: String,
    pub base: String,
}
