//! Standalone preview: `cargo run -p screen_agents --example screen_agents`

use gpui::AppContext as _;

fn main() {
    ui::run_app("Agents & pipelines (preview)", 1440., 960., |window, cx| {
        let view = cx.new(|cx| screen_agents::AgentsScreen::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
