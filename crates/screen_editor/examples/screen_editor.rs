//! Standalone preview: `cargo run -p screen_editor --example screen_editor`

use gpui::AppContext as _;

fn main() {
    ui::run_app("Pipeline editor (preview)", 1440., 960., |window, cx| {
        let view = cx.new(|cx| screen_editor::PipelineEditorScreen::new(window, cx));
        cx.new(|_| ui::Preview(view.into()))
    });
}
