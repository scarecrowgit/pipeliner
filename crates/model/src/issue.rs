//! The issue tracker: issues, their activity feed, and autopilots that create or pick them up.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{AutopilotId, BlobId, IssueKey, Millis, NodeId, ProjectId, RunId, RunnableKind, RunnableRef, SessionId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    #[default]
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
}

impl IssueStatus {
    pub const ALL: [IssueStatus; 5] =
        [IssueStatus::Backlog, IssueStatus::Todo, IssueStatus::InProgress, IssueStatus::InReview, IssueStatus::Done];

    pub fn label(self) -> &'static str {
        match self {
            IssueStatus::Backlog => "Backlog",
            IssueStatus::Todo => "Todo",
            IssueStatus::InProgress => "In progress",
            IssueStatus::InReview => "In review",
            IssueStatus::Done => "Done",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    #[default]
    Medium,
    High,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueType {
    Bug,
    #[default]
    Story,
    Chore,
    Docs,
}

impl IssueType {
    pub const ALL: [IssueType; 4] = [IssueType::Bug, IssueType::Story, IssueType::Chore, IssueType::Docs];

    pub fn label(self) -> &'static str {
        match self {
            IssueType::Bug => "Bug",
            IssueType::Story => "Story",
            IssueType::Chore => "Chore",
            IssueType::Docs => "Docs",
        }
    }
}

/// When an attached runner starts on its issue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartTriggers {
    /// As soon as it is attached (or the issue is created with it).
    pub on_assign: bool,
    /// On every new comment.
    pub on_comment: bool,
    /// When a comment mentions it.
    pub on_mention: bool,
    /// When the issue moves to Todo.
    #[serde(default)]
    pub on_todo: bool,
}

impl Default for StartTriggers {
    fn default() -> Self {
        Self { on_assign: true, on_comment: false, on_mention: true, on_todo: false }
    }
}

/// An image attached to an issue, a comment or a chat message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub blob: BlobId,
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub key: IssueKey,
    pub project: ProjectId,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: IssueStatus,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub kind: IssueType,
    /// Display name of the person it belongs to; empty when unowned.
    #[serde(default)]
    pub owner: String,
    /// Repo name the work is expected in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub created: Millis,
    pub updated: Millis,
    /// The agent that filed it, when it was not a person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_by: Option<RunnableRef>,
    /// The agent, orchestrator or pipeline attached under "Runs with".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RunnableRef>,
    #[serde(default)]
    pub triggers: StartTriggers,
    /// The latest run on this issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    /// How many runs it has had.
    #[serde(default)]
    pub run_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Attachment>,
}

/// Who did something on an issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "actor", rename_all = "snake_case")]
pub enum Actor {
    Me,
    Runnable {
        reference: RunnableRef,
        name: String,
        /// Created by another agent; drawn in violet.
        #[serde(default)]
        spawned: bool,
        /// The session it wrote from, when it can be opened: an agent answering a mention.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionId>,
    },
    System,
}

/// An agent or orchestrator that worked on an issue in its latest run, as a comment can
/// mention it: the mention continues its node's own session, in its worktree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAgent {
    /// What a mention names: [`RunnableRef::session`] of `session`.
    pub reference: RunnableRef,
    /// The node's name in the run, with the variant's letter when it has one: "Mockup".
    pub name: String,
    /// The library runnable's name when the node uses one and it is not `name`: a mention
    /// may use it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runnable_name: Option<String>,
    pub kind: RunnableKind,
    pub session: SessionId,
    pub run: RunId,
    /// "run 2" on the issue.
    pub run_number: u32,
    pub node: NodeId,
    #[serde(default)]
    pub variant: u32,
    /// Where the node worked, when it had a worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Its node is running right now: a mention is refused until it finishes.
    #[serde(default)]
    pub busy: bool,
}

impl RunAgent {
    /// The names a mention may use, the node's first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.runnable_name.as_deref())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueEvent {
    pub seq: u64,
    pub at: Millis,
    pub actor: Actor,
    #[serde(flatten)]
    pub kind: IssueEventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum IssueEventKind {
    Created,
    Comment {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<Attachment>,
        /// Runnables named with `@` in the text.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        mentions: Vec<RunnableRef>,
    },
    StatusChanged {
        from: IssueStatus,
        to: IssueStatus,
    },
    Attached {
        runner: RunnableRef,
    },
    Detached,
    /// Something an agent or a pipeline did, in one sentence: "Picked up by rule 2 → Reproducer."
    Note {
        text: String,
    },
    /// A mention or a trigger started work.
    Woken {
        target: RunnableRef,
        name: String,
        run: Option<RunId>,
        number: u32,
        /// "comment passed as input", "moved to Todo".
        cause: String,
    },
    /// A mention or a trigger could not start work.
    WakeFailed {
        target: RunnableRef,
        name: String,
        reason: String,
    },
    PrOpened {
        url: String,
    },
}

// ---------------------------------------------------------------- autopilots

/// A clock rule in local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "every", rename_all = "snake_case")]
pub enum Schedule {
    Day {
        hour: u32,
        minute: u32,
    },
    /// `weekday` 0 is Monday.
    Week {
        weekday: u32,
        hour: u32,
        minute: u32,
    },
    Minutes {
        minutes: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum AutopilotTrigger {
    Schedule {
        #[serde(flatten)]
        schedule: Schedule,
    },
    /// Something happening elsewhere (a new Sentry error). Not available yet.
    Event { source: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Autopilot {
    pub id: AutopilotId,
    pub project: ProjectId,
    pub name: String,
    pub trigger: AutopilotTrigger,
    pub target: RunnableRef,
    /// What the target is told on each tick.
    #[serde(default)]
    pub prompt: String,
    pub enabled: bool,
    /// Set when the app switched it off itself (an expired token, a missing repo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<Millis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<Millis>,
    /// "filed 2 issues".
    #[serde(default)]
    pub last_result: String,
    /// Spending cap per tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_rows_written_before_agent_sessions_still_load() {
        let old = r#"{"seq":5,"at":1,"actor":{"actor":"runnable","reference":"agent:fixer","name":"Fixer","spawned":false},"event":"comment","text":"Picking this up."}"#;
        let event: IssueEvent = serde_json::from_str(old).expect("old row");
        assert_eq!(
            event.actor,
            Actor::Runnable { reference: RunnableRef::agent("fixer"), name: "Fixer".into(), spawned: false, session: None }
        );
        let reply = IssueEvent {
            actor: Actor::Runnable {
                reference: RunnableRef::session(&SessionId::new("s1")),
                name: "Mockup".into(),
                spawned: false,
                session: Some(SessionId::new("s1")),
            },
            ..event
        };
        let text = serde_json::to_string(&reply).expect("write");
        assert_eq!(serde_json::from_str::<IssueEvent>(&text).expect("read"), reply);
        assert!(!serde_json::to_string(&Actor::Runnable {
            reference: RunnableRef::agent("fixer"),
            name: "Fixer".into(),
            spawned: false,
            session: None,
        })
        .expect("write")
        .contains("session"));
    }

    #[test]
    fn a_session_reference_names_its_session() {
        let id = SessionId::new("0mwuv2k31-01zxq1xwjl");
        let r = RunnableRef::session(&id);
        assert_eq!(r.scheme(), "session");
        assert_eq!(r.session_id(), Some(id));
        assert_eq!(RunnableRef::agent("fixer").session_id(), None);
    }
}
