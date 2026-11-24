//! The new-chat page (pick an agent and a folder, write the first message) and the list of
//! every chat.

use gpui::{AnyElement, Context, IntoElement, Render, SharedString, Window, div};

/// The whole content of `Route::NewChat`.
pub struct NewChatView {}

impl NewChatView {
    /// An empty new-chat page; the composer takes focus.
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {}
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

impl Render for NewChatView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The whole content of `Route::Chats`: every chat as a list.
pub struct ChatsView {}

impl ChatsView {
    /// The list, loaded from the chat index.
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {}
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

impl Render for ChatsView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
