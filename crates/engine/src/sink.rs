//! Where a turn's agent events go: onto the bus, with a few numbers kept for the session
//! header once the turn ends, and the display of every finished tool call kept in the store
//! so history draws it as the live stream did.

use agent::AgentSink;
use model::{AgentEvent, Event, SessionId};

use crate::engine::Inner;

impl Inner {
    /// Keeps what a finished tool call showed, beside its result. Best effort: a call whose
    /// display was not kept is drawn from its input in history.
    pub(crate) fn keep_tool_display(&self, session: &SessionId, event: &AgentEvent) {
        if let AgentEvent::ToolFinished { id, display, .. } = event {
            let _ = self.store.set_tool_display(session, id, display);
        }
    }
}

/// What the turn's events said about its own session, for the header.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TurnNotes {
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub pr_url: Option<String>,
    /// Sub-agent sessions that finished, with whether they did so on their own.
    pub finished_children: Vec<(SessionId, bool)>,
}

/// Forwards every event to the bus as [`Event::Agent`]. Called under the loop's lock, also
/// from sub-agent threads: it sends on unbounded channels, writes its own fields and, for a
/// finished tool call, one short store write.
pub(crate) struct BusSink<'a> {
    inner: &'a Inner,
    top: SessionId,
    pub notes: TurnNotes,
}

impl<'a> BusSink<'a> {
    pub fn new(inner: &'a Inner, top: SessionId) -> Self {
        Self { inner, top, notes: TurnNotes::default() }
    }
}

impl AgentSink for BusSink<'_> {
    fn event(&mut self, session: &SessionId, event: AgentEvent) {
        match &event {
            AgentEvent::Usage { context_tokens, context_window, .. } if session == &self.top => {
                self.notes.context_tokens = Some(*context_tokens);
                self.notes.context_window = Some(*context_window);
            }
            AgentEvent::Compacted { summarised, context_tokens } if session == &self.top && *summarised > 0 => {
                self.notes.context_tokens = Some(*context_tokens);
            }
            AgentEvent::PrDetected { url } if session == &self.top => self.notes.pr_url = Some(url.clone()),
            AgentEvent::SubagentFinished { session: child, ok, .. } => {
                self.notes.finished_children.push((child.clone(), *ok));
            }
            _ => {}
        }
        self.inner.keep_tool_display(session, &event);
        self.inner.publish(Event::Agent { session: session.clone(), event });
    }
}
