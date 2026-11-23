//! Standalone preview: `cargo run -p sidebar --example sidebar`

use gpui::AppContext as _;

fn main() {
    ui::run_app("Sidebar (preview)", 232., 960., |window, cx| {
        // Nav holds the sidebar mode (and its saved choice), so the toggle works here too.
        ui::nav::install(ui::nav::Screen::Inbox, cx);
        let view = cx.new(|cx| sidebar::Sidebar::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
