//! Harness — agent pipeline editor demo. App shell: sidebar + current screen.

use gpui::{AnyView, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use ui::nav::{self, Nav, Screen};

struct Shell {
    nav: Entity<Nav>,
    sidebar: Entity<sidebar::Sidebar>,
    editor: Entity<screen_editor::PipelineEditorScreen>,
    review: Entity<screen_review::ReviewScreen>,
    chat: Entity<screen_chat::ChatScreen>,
    issues: Entity<screen_issues::IssuesScreen>,
    agents: Entity<screen_agents::AgentsScreen>,
}

impl Shell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = nav::install(initial_screen(), cx);
        cx.observe(&nav, |_, _, cx| cx.notify()).detach();
        Self {
            nav,
            sidebar: cx.new(|cx| sidebar::Sidebar::new(window, cx)),
            editor: cx.new(|cx| screen_editor::PipelineEditorScreen::new(window, cx)),
            review: cx.new(|cx| screen_review::ReviewScreen::new(window, cx)),
            chat: cx.new(|cx| screen_chat::ChatScreen::new(window, cx)),
            issues: cx.new(|cx| screen_issues::IssuesScreen::new(window, cx)),
            agents: cx.new(|cx| screen_agents::AgentsScreen::new(window, cx)),
        }
    }
}

/// `HARNESS_SCREEN=chat|editor|inbox|issues|agents` picks the start screen (default: chat).
fn initial_screen() -> Screen {
    match std::env::var("HARNESS_SCREEN").as_deref() {
        Ok("editor") => Screen::PipelineEditor,
        Ok("inbox") | Ok("review") => Screen::Inbox,
        Ok("issues") => Screen::Issues,
        Ok("agents") => Screen::Agents,
        _ => Screen::Chat,
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen: AnyView = match self.nav.read(cx).screen {
            Screen::PipelineEditor => self.editor.clone().into(),
            Screen::Inbox => self.review.clone().into(),
            Screen::Chat => self.chat.clone().into(),
            Screen::Issues => self.issues.clone().into(),
            Screen::Agents => self.agents.clone().into(),
        };
        ui::root()
            .flex()
            .child(self.sidebar.clone())
            .child(div().flex_1().min_w_0().h_full().child(screen))
    }
}

fn main() {
    ui::run_app("Harness", 1440., 960., |window, cx| cx.new(|cx| Shell::new(window, cx)));
}
