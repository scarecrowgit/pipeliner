//! Standalone preview: `cargo run -p screen_home --example screen_home`
//!
//! A blank 232px strip stands in for the app sidebar so the layout matches the app.
//! Reads the real pipeline and Inbox data (`$XDG_DATA_HOME/harness`).
//! `HOME_DEMO_CHATS=1` publishes two fake chat sessions (one running, one waiting) to
//! preview chats in "Working now" / "Needs you".

use gpui::{AnyView, AppContext as _, Context, IntoElement, Render, Window, div, prelude::*, px};
use ui::nav::{self, ChatSessionInfo, ChatStatus, Screen};
use ui::theme::*;

struct Shell(AnyView);

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .child(
                div()
                    .w(px(SIDEBAR_WIDTH))
                    .flex_none()
                    .h_full()
                    .bg(BG_SIDEBAR)
                    .border_r_1()
                    .border_color(LINE),
            )
            .child(div().flex_1().min_w_0().h_full().child(self.0.clone()))
    }
}

fn demo_chats() -> Vec<ChatSessionInfo> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let chat = |id: &str, title: &str, status, agent: &str, mins: u64, cost| ChatSessionInfo {
        id: id.to_string().into(),
        title: title.to_string().into(),
        status,
        pinned: false,
        updated: now - mins * 60,
        agent: agent.to_string().into(),
        workdir: "pipeliner · main".into(),
        cost_usd: cost,
    };
    vec![
        chat(
            "a",
            "Home dashboard screen",
            ChatStatus::Running,
            "Build",
            4,
            0.37,
        ),
        chat(
            "b",
            "Refactor the nav entity so screens can publish state",
            ChatStatus::Waiting,
            "Plan",
            17,
            1.2,
        ),
        chat("c", "Old idle chat", ChatStatus::Idle, "Build", 90, 0.05),
    ]
}

fn main() {
    ui::run_app("Home (preview)", 1440., 960., |window, cx| {
        nav::install(Screen::Home, cx);
        pipeline::init(cx);
        if std::env::var_os("HOME_DEMO_CHATS").is_some() {
            nav::set_chat_sessions(demo_chats(), None, 3, cx);
        }
        let view = cx.new(|cx| screen_home::HomeScreen::new(window, cx));
        cx.new(|_| Shell(view.into()))
    });
}
