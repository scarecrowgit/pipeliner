//! Where a turn's agent events go: onto the bus, with a few numbers kept for the session
//! header once the turn ends.

use agent::AgentSink;
use model::{AgentEvent, Event, SessionId};

use crate::bus::Bus;

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
/// from sub-agent threads: it only sends on unbounded channels and writes its own fields.
pub(crate) struct BusSink<'a> {
    bus: &'a Bus,
    top: SessionId,
    pub notes: TurnNotes,
}

impl<'a> BusSink<'a> {
    pub fn new(bus: &'a Bus, top: SessionId) -> Self {
        Self { bus, top, notes: TurnNotes::default() }
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
        self.bus.publish(Event::Agent { session: session.clone(), event });
    }
}
