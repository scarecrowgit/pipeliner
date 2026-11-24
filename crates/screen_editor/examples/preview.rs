//! The pipeline editor over the fixture world, in a window, under a stand-in for the shell's
//! header (crumbs, the view's badges and actions).
//!
//! `cargo run -p screen_editor --example preview [-- --fixture pipeline-editor]`

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
    while let Some(arg) = args.next() {
        if arg == "--fixture"
            && let Some(name) = args.next()
        {
            fixture = name;
        }
    }
    ui::run_app("Pipeline editor", 1440., 1060., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api, cx);
        let (pipeline, node) = match fixtures::start_route(&fixture) {
            Some((Route::Editor { pipeline, node }, _)) => (pipeline, node),
            _ => (PipelineId::new("pay-triage"), None),
        };
        hub::navigate(Route::Editor { pipeline: pipeline.clone(), node: node.clone() }, cx);
        cx.new(|cx| Stage::new(pipeline, node, window, cx))
    });
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
