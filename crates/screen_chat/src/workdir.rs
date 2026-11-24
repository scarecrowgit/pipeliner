//! Where a chat works: the folder chip, the popover it opens, and the switch dialog.

use gpui::{Context, IntoElement, Render, Window, div};
use hub::model::SessionId;

/// The chip that names a session's folder and branch and opens the workdir popover. With no
/// session it belongs to the new-chat page and edits the choice for the chat about to start.
pub struct WorkdirChip {
    session: Option<SessionId>,
}

impl WorkdirChip {
    /// A chip for `session`, or for the chat that does not exist yet.
    pub fn new(session: Option<SessionId>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { session }
    }

    /// The session it belongs to, if any.
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }
}

impl Render for WorkdirChip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
