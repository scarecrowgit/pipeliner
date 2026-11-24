//! The Home view.

use gpui::{AnyElement, App, Context, IntoElement, Render, SharedString, Window, div};

/// The whole content of `Route::Home`.
pub struct HomeView {}

impl HomeView {
    /// Home for the current project. Loading starts at once.
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {}
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &App) -> SharedString {
        "Home".into()
    }

    /// What follows the title in the shell's header.
    pub fn header_badges(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    /// The controls on the right of the shell's header, left to right: the period switch.
    pub fn header_actions(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }
}

impl Render for HomeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
