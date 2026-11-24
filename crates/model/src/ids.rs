//! Typed identifiers. Each is a string underneath, so it serialises as one and is cheap to show.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

string_id!(
    /// A chat session, or the session of one agent inside a pipeline run.
    SessionId
);
string_id!(
    /// An issue key as people read it: `HAR-12`.
    IssueKey
);
string_id!(
    /// One pipeline run.
    RunId
);
string_id!(
    /// A node inside a pipeline graph; unique within that graph.
    NodeId
);
string_id!(
    /// A pipeline definition.
    PipelineId
);
string_id!(
    /// A set of variants waiting for a person.
    ReviewId
);
string_id!(
    /// An Inbox entry.
    InboxId
);
string_id!(
    /// A project: a named group of local repos with its own issue key prefix.
    ProjectId
);
string_id!(
    /// A scheduled or event-driven trigger.
    AutopilotId
);
string_id!(
    /// Content-addressed file in the blob store (an image, a render, a long tool output).
    BlobId
);
string_id!(
    /// A pending question to a person: a tool permission or an agent's question.
    RequestId
);

/// Anything that can take work: `agent:fixer`, `orchestrator:lead`, `pipeline:pay-triage`,
/// `spawned:<id>` for an agent another agent created.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunnableRef(pub String);

impl RunnableRef {
    pub fn agent(id: &str) -> Self {
        Self(format!("agent:{id}"))
    }

    pub fn orchestrator(id: &str) -> Self {
        Self(format!("orchestrator:{id}"))
    }

    pub fn pipeline(id: &str) -> Self {
        Self(format!("pipeline:{id}"))
    }

    pub fn spawned(id: &str) -> Self {
        Self(format!("spawned:{id}"))
    }

    /// An agent of an issue's run, named by its node's session: a mention of it continues
    /// that session instead of starting anything new.
    pub fn session(id: &SessionId) -> Self {
        Self(format!("session:{id}"))
    }

    /// The session a [`RunnableRef::session`] reference names.
    pub fn session_id(&self) -> Option<SessionId> {
        (self.scheme() == "session").then(|| SessionId::new(self.id()))
    }

    /// The part before the colon: `agent`, `orchestrator`, `pipeline`, `spawned` or `session`.
    pub fn scheme(&self) -> &str {
        self.0.split_once(':').map_or("", |(scheme, _)| scheme)
    }

    /// The part after the colon.
    pub fn id(&self) -> &str {
        self.0.split_once(':').map_or(self.0.as_str(), |(_, id)| id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunnableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_serialise_as_plain_strings() {
        let key = IssueKey::new("HAR-12");
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"HAR-12\"");
        assert_eq!(serde_json::from_str::<IssueKey>("\"HAR-12\"").unwrap(), key);
    }

    #[test]
    fn runnable_refs_split_into_scheme_and_id() {
        let r = RunnableRef::pipeline("mockup-gpui-build");
        assert_eq!(r.scheme(), "pipeline");
        assert_eq!(r.id(), "mockup-gpui-build");
        assert_eq!(RunnableRef("bare".into()).scheme(), "");
        assert_eq!(RunnableRef("bare".into()).id(), "bare");
    }
}
