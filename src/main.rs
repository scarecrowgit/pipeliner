//! Harness — agent pipeline editor demo. App shell: sidebar + current screen.

use gpui::{AnyView, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use ui::nav::{self, Nav, Screen};

struct Shell {
    nav: Entity<Nav>,
    sidebar: Entity<sidebar::Sidebar>,
    home: Entity<screen_home::HomeScreen>,
    history: Entity<screen_chat::ChatHistory>,
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
        // The pipeline service (issues, runs, agent config) is global; the issue board and
        // the agents screen read it, so it must exist before they are built.
        pipeline::init(cx);
        Self {
            nav,
            sidebar: cx.new(|cx| sidebar::Sidebar::new(window, cx)),
            home: cx.new(|cx| screen_home::HomeScreen::new(window, cx)),
            history: cx.new(|cx| screen_chat::ChatHistory::new(window, cx)),
            editor: cx.new(|cx| screen_editor::PipelineEditorScreen::new(window, cx)),
            review: cx.new(|cx| screen_review::ReviewScreen::new(window, cx)),
            chat: cx.new(|cx| screen_chat::ChatScreen::new(window, cx)),
            issues: cx.new(|cx| screen_issues::IssuesScreen::new(window, cx)),
            agents: cx.new(|cx| screen_agents::AgentsScreen::new(window, cx)),
        }
    }
}

/// `HARNESS_SCREEN=chat|home|editor|inbox|issues|agents` picks the start screen (default: chat).
/// Test hooks read by the screens: `HARNESS_ISSUE=<KEY>` opens that issue's detail,
/// `HARNESS_ISSUES_DIALOG=new` opens the "New issue" dialog, `HARNESS_AGENTS_TAB=pipeline`
/// opens the "Pipeline agents" tab.
fn initial_screen() -> Screen {
    match std::env::var("HARNESS_SCREEN").as_deref() {
        Ok("home") => Screen::Home,
        Ok("editor") => Screen::PipelineEditor,
        Ok("inbox") | Ok("review") => Screen::Inbox,
        Ok("issues") => Screen::Issues,
        Ok("agents") => Screen::Agents,
        _ => Screen::Chat,
    }
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self.nav.read(cx);
        let screen: AnyView = match nav.screen {
            Screen::Home => self.home.clone().into(),
            Screen::PipelineEditor => self.editor.clone().into(),
            Screen::Inbox => self.review.clone().into(),
            Screen::Chat => self.chat.clone().into(),
            Screen::Issues => self.issues.clone().into(),
            Screen::Agents => self.agents.clone().into(),
        };
        // The history picker is always in the tree (it renders nothing while closed) so
        // it keeps its state, can take focus when ⌘K opens it and can animate out.
        ui::root()
            .relative()
            .flex()
            .child(self.sidebar.clone())
            .child(div().flex_1().min_w_0().h_full().child(screen))
            // Rendered empty while closed; kept mounted so it can play its exit.
            .child(div().absolute().inset_0().child(self.history.clone()))
    }
}

fn main() {
    ui::run_app("Harness", 1440., 960., |window, cx| cx.new(|cx| Shell::new(window, cx)));
}
