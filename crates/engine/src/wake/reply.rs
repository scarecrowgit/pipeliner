//! Answers in the issue's thread: a session woken by a comment owes the issue a reply, posted
//! as that agent's comment when the turn that took the comment in ends. Such a comment wakes
//! nobody, whatever it says: it never goes through the mention rules.

use std::collections::HashMap;
use std::sync::Mutex;

use agent::{AgentOutcome, StopCause};
use model::{Actor, IssueEventKind, IssueKey, SessionId};

use crate::bus::lock;
use crate::engine::Inner;

/// A reply a session owes an issue.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingReply {
    pub issue: IssueKey,
    /// The agent as the comment names it, with its session.
    pub actor: Actor,
    /// The message the comment became: while it still waits in the session's queue, the turn
    /// that ends did not see it.
    pub said: String,
}

/// Replies owed, by session.
#[derive(Default)]
pub(crate) struct Replies {
    owed: Mutex<HashMap<SessionId, Vec<PendingReply>>>,
}

impl Replies {
    pub fn expect(&self, session: &SessionId, reply: PendingReply) {
        lock(&self.owed).entry(session.clone()).or_default().push(reply);
    }

    /// Drops the reply owed for `said`: its message never reached the session.
    pub fn forget(&self, session: &SessionId, said: &str) {
        let mut owed = lock(&self.owed);
        if let Some(list) = owed.get_mut(session) {
            if let Some(at) = list.iter().position(|r| r.said == said) {
                list.remove(at);
            }
            if list.is_empty() {
                owed.remove(session);
            }
        }
    }

    /// Takes the replies whose message is no longer queued for `session`.
    fn due(&self, session: &SessionId, queued: &[String]) -> Vec<PendingReply> {
        let mut owed = lock(&self.owed);
        let Some(list) = owed.get_mut(session) else { return Vec::new() };
        let (due, wait): (Vec<_>, Vec<_>) = list.drain(..).partition(|r| !queued.contains(&r.said));
        *list = wait;
        if list.is_empty() {
            owed.remove(session);
        }
        due
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
    /// Posts what `outcome` says on every issue whose comment this turn of `session` took in.
    pub(crate) fn post_replies(&self, session: &SessionId, outcome: &AgentOutcome) {
        let queued: Vec<String> = self.live.queued(session).into_iter().map(|m| m.text).collect();
        for reply in self.replies.due(session, &queued) {
            let name = match &reply.actor {
                Actor::Runnable { name, .. } => name.clone(),
                _ => "the agent".to_string(),
            };
            let kind = IssueEventKind::Comment { text: reply_text(outcome, &name), images: Vec::new(), mentions: Vec::new() };
            if let Err(error) = self.activity(&reply.issue, reply.actor.clone(), kind) {
                self.notice_error(&format!("Could not post {name}'s answer on {}: {error:#}", reply.issue));
                continue;
            }
            if let Ok(Some(mut issue)) = self.store.issue(&reply.issue) {
                issue.updated = self.now();
                let _ = self.save_issue(&issue);
            }
        }
    }
}
