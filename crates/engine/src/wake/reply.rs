//! Answers in the issue's thread: a session woken by a comment owes the issue a reply, posted
//! as that agent's comment when the turn that took the comment in ends. Such a comment wakes
//! nobody, whatever it says: it never goes through the mention rules.
//!
//! Each owed reply has a tag that travels with its message through the session's queue
//! ([`crate::live::Live::send_tagged`]); a turn answers exactly the tags it took in, so two
//! comments with the same text stay two answers. What is owed is kept in the store too: after
//! a crash or a quit in the middle of the turn, the thread says the agent was interrupted
//! instead of going silent.

use std::collections::HashMap;
use std::sync::Mutex;

use agent::{AgentOutcome, StopCause};
use model::{Actor, IssueEventKind, IssueKey, SessionId};
use serde_json::json;

use crate::bus::lock;
use crate::engine::Inner;

/// Prefix of the store markers that keep owed replies over a restart.
const OWED: &str = "owed-reply:";

/// A reply a session owes an issue.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingReply {
    pub issue: IssueKey,
    /// The agent as the comment names it, with its session.
    pub actor: Actor,
}

/// Replies owed, by tag.
#[derive(Default)]
pub(crate) struct Replies {
    owed: Mutex<HashMap<String, PendingReply>>,
}

/// A new tag for an owed reply, unique across restarts.
pub(crate) fn new_tag() -> String {
    base::id::new_id()
}

fn actor_name(actor: &Actor) -> String {
    match actor {
        Actor::Runnable { name, .. } => name.clone(),
        _ => "The agent".to_string(),
    }
}

/// What the agent's comment says after a turn that stopped this way.
pub(crate) fn reply_text(outcome: &AgentOutcome, name: &str) -> String {
    let said = outcome.text.trim();
    match &outcome.stop {
        StopCause::Finished if said.is_empty() => "Done, without a written reply.".to_string(),
        StopCause::Finished => said.to_string(),
        StopCause::Waiting { question } => {
            format!("{}\n\nMention {name} in a comment to answer.", question.trim())
        }
        StopCause::Failed { error } => format!("Could not answer: {}", error.trim()),
        StopCause::Cancelled => "Stopped before answering.".to_string(),
        StopCause::StepLimit => format!("Stopped after using all its steps. Mention {name} again to let it go on."),
        StopCause::Budget => "Stopped at its budget before answering.".to_string(),
    }
}

impl Inner {
    /// Notes that the message tagged `tag` owes `reply.issue` an answer, here and in the store.
    pub(crate) fn owe_reply(&self, tag: &str, session: &SessionId, reply: PendingReply) {
        let kept = json!({ "session": session, "issue": reply.issue, "actor": reply.actor });
        if let Err(error) = self.store.set_marker(&format!("{OWED}{tag}"), &kept.to_string()) {
            self.notice_error(&format!("Could not keep the answer {} owes {}: {error:#}", actor_name(&reply.actor), reply.issue));
        }
        lock(&self.replies.owed).insert(tag.to_string(), reply);
    }

    /// Forgets an owed reply whose message never reached its session.
    pub(crate) fn forget_reply(&self, tag: &str) {
        lock(&self.replies.owed).remove(tag);
        let _ = self.store.remove_marker(&format!("{OWED}{tag}"));
    }

    /// Posts what `outcome` says on every issue whose comment the turn of `session` that just
    /// ended took in.
    ///
    /// Comments on one issue that the same turn took in together get one answer: the turn saw
    /// them all and answered once.
    pub(crate) fn post_replies(&self, session: &SessionId, outcome: &AgentOutcome) {
        let mut answered: Vec<(IssueKey, Actor)> = Vec::new();
        for tag in self.live.take_consumed(session) {
            let Some(reply) = lock(&self.replies.owed).remove(&tag) else { continue };
            let _ = self.store.remove_marker(&format!("{OWED}{tag}"));
            if answered.iter().any(|(issue, actor)| issue == &reply.issue && actor == &reply.actor) {
                continue;
            }
            answered.push((reply.issue.clone(), reply.actor.clone()));
            let name = actor_name(&reply.actor);
            let kind = IssueEventKind::Comment { text: reply_text(outcome, &name), images: Vec::new(), mentions: Vec::new() };
            self.post_on_issue(&reply.issue, reply.actor, kind);
        }
    }

    fn post_on_issue(&self, issue: &IssueKey, actor: Actor, kind: IssueEventKind) {
        let name = actor_name(&actor);
        if let Err(error) = self.activity(issue, actor, kind) {
            self.notice_error(&format!("Could not post {name}'s answer on {issue}: {error:#}"));
            return;
        }
        if let Ok(Some(mut stored)) = self.store.issue(issue) {
            stored.updated = self.now();
            let _ = self.save_issue(&stored);
        }
    }

    /// After a restart: every answer still owed came from a turn that did not finish. Each
    /// thread gets a line saying so, and nothing stays owed.
    pub(crate) fn settle_owed_replies(&self) {
        let Ok(owed) = self.store.markers_with_prefix(OWED) else { return };
        for (key, value) in owed {
            let kept: Option<(IssueKey, Actor)> = serde_json::from_str::<serde_json::Value>(&value).ok().and_then(|v| {
                let issue = serde_json::from_value(v.get("issue")?.clone()).ok()?;
                let actor = serde_json::from_value(v.get("actor")?.clone()).ok()?;
                Some((issue, actor))
            });
            if let Some((issue, actor)) = kept
                && self.store.issue(&issue).ok().flatten().is_some()
            {
                let name = actor_name(&actor);
                let text = format!("{name} was interrupted before answering, when the app closed. Mention {name} again to go on.");
                self.post_on_issue(&issue, actor, IssueEventKind::Note { text });
            }
            let _ = self.store.remove_marker(&key);
        }
    }
}
