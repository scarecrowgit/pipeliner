//! Standalone preview: `cargo run -p screen_issues --example screen_issues` (real pipeline data).

use gpui::AppContext as _;

fn main() {
    ui::run_app("Issue board (preview)", 1440., 960., |window, cx| {
        let view = cx.new(|cx| screen_issues::IssuesScreen::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
