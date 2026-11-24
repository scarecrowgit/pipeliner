//! The pipeline editor over the fixture world, in a window, under a stand-in for the shell's
//! header (crumbs, the view's badges and actions).
//!
//! `cargo run -p screen_editor --example preview [-- --fixture pipeline-editor] [--log-lines]`
//!
//! `--log-lines` writes a few lines to the log of the run on show (`run-test-42`) shortly
//! after start, for checking the Log tab: the fixture's test runs have no log of their own.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*, px};
use hub::fixtures::{self, FixtureApi};
use hub::model::PipelineId;
use hub::{Hub, Route};
use screen_editor::EditorView;
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "pipeline-editor".to_string();
    let mut log_lines = false;
    while let Some(arg) = args.next() {
        if arg == "--log-lines" {
            log_lines = true;
        } else if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Pipeline editor", 1440., 1060., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api.clone(), cx);
        if log_lines {
            write_log(api, cx);
        }
        let (pipeline, node) = match fixtures::start_route(&fixture) {
            Some((Route::Editor { pipeline, node }, _)) => (pipeline, node),
            _ => (PipelineId::new("pay-triage"), None),
        };
        hub::navigate(Route::Editor { pipeline: pipeline.clone(), node: node.clone() }, cx);
        cx.new(|cx| Stage::new(pipeline, node, window, cx))
    });
}

/// Sample lines in the log of the run on show, once the editor is up.
fn write_log(api: Arc<FixtureApi>, cx: &mut gpui::App) {
    use hub::api::{Event, RunLogLine};
    use hub::model::{LogLevel, NodeId, RunId};
    const LINES: [&str; 8] = [
        "worktree ready · fix/PAY-1423",
        "read src/charges/retry.ts",
        "grep idempotencyKey",
        "bash bun test src/charges",
        "1 test failed: retry charges twice",
        "edit src/charges/retry.ts",
        "bash bun test src/charges",
        "all 14 tests passed",
    ];
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
        let now = base::time::now_ms();
        for (ix, text) in LINES.iter().enumerate() {
            let line = RunLogLine {
                seq: ix as u64 + 1,
                at: now,
                node: Some(NodeId::new("fixer")),
                variant: 0,
                level: if text.starts_with("bash") || text.starts_with("read") || text.starts_with("grep") { LogLevel::Tool } else { LogLevel::Info },
                text: (*text).to_string(),
            };
            api.emit(Event::RunLog { run: RunId::new("run-test-42"), line });
        }
    })
    .detach();
}

/// The shell's part around the view: a header bar over the content, in an inset frame.
struct Stage {
    view: Entity<EditorView>,
}

impl Stage {
    fn new(pipeline: PipelineId, node: Option<hub::model::NodeId>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| EditorView::new(pipeline, node, window, cx));
        cx.observe(&view, |_, _, cx| cx.notify()).detach();
        let router = hub::router(cx);
        let follow = view.clone();
        cx.observe_in(&router, window, move |_, router, window, cx| {
            if let Route::Editor { pipeline, node } = router.read(cx).current().clone() {
                follow.update(cx, |view, cx| view.show(pipeline, node, window, cx));
            }
        })
        .detach();
        Self { view }
    }
}

impl Render for Stage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, badges, actions) =
            self.view.update(cx, |v, cx| (v.title(cx), v.header_badges(window, cx), v.header_actions(window, cx)));
        let crumbs = row()
            .flex_none()
            .gap(px(SPACE_1_5))
            .type_style(HEADING)
            .child(div().text_color(TEXT_MUTED).font_weight(gpui::FontWeight::NORMAL).child("Pipelines"))
            .child(icon(IconName::ChevronRight, 14., TEXT_DISABLED))
            .child(title);
        ui::root().bg(BG_SIDEBAR).flex().p(px(SPACE_2)).child(
            col()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(BG)
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .overflow_hidden()
                .child(
                    row()
                        .flex_none()
                        .h(px(HEADER_HEIGHT))
                        .gap(px(SPACE_2))
                        .pl(px(SPACE_5))
                        .pr(px(SPACE_2))
                        .border_b_1()
                        .border_color(LINE)
                        .child(crumbs)
                        .children(badges)
                        .child(grow())
                        .child(row().flex_none().gap(px(SPACE_2)).children(actions)),
                )
                .child(div().flex_1().min_h_0().flex().child(self.view.clone())),
        )
    }
}
