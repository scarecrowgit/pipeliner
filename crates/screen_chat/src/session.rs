//! One chat session: the transcript, the composer with its pickers, the context panel, and
//! the session's overlays (agent picker, share).

use gpui::{AnyElement, Context, IntoElement, Render, SharedString, Window, div};
use hub::model::SessionId;

/// The whole content of `Route::Chat`.
pub struct SessionView {
    session: SessionId,
}

impl SessionView {
    /// The view of `session`. Loading starts at once; the composer takes focus.
    pub fn new(session: SessionId, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { session }
    }

    /// The session it shows.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &gpui::App) -> SharedString {
        SharedString::default()
    }

    /// The controls on the right of the shell's header, left to right.
    pub fn header_actions(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }
}

impl Render for SessionView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
