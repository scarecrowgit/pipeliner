//! The transcript of one session: messages, thinking, tool calls with their results and
//! diffs, todo lists, sub-agents, permission questions, compaction and error notices.

use gpui::{Context, IntoElement, Render, Window, div};
use hub::model::SessionId;

/// The scrolling transcript of one session. Self-contained: it reads the session's model from
/// `hub`, follows the stream, loads older pages when scrolled up and answers permission
/// questions through `hub::commands`.
pub struct TranscriptView {
    session: SessionId,
}

impl TranscriptView {
    /// A transcript for `session`. Loading starts at once.
    pub fn new(session: SessionId, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { session }
    }

    /// The session it shows.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Jumps to the newest message and resumes following the stream.
    pub fn scroll_to_end(&mut self, _cx: &mut Context<Self>) {}
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
