//! The inspector alone, at its width on the right of a window, over a draft of the fixture
//! world's pipeline. Problems are checked through the hub after every change, as the frame
//! does.
//!
//! `cargo run -p screen_editor --example inspector [-- --fixture pipeline-editor] [--node fixer|none] [--edge 3] [--run]`
//!
//! `--node none` shows the pipeline itself; `--edge <index>` selects an edge; `--run` puts the
//! fixture's run of PAY-1423 on show for the spend line.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*};
use hub::fixtures::{self, FixtureApi};
use hub::model::{NodeId, PipelineId, RunId};
use hub::{Hub, Route};
use screen_editor::{Draft, DraftEvent, InspectorView, Selection};
use ui::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "pipeline-editor".to_string();
    let mut node: Option<String> = None;
    let mut edge: Option<usize> = None;
    let mut run = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => fixture = args.next().unwrap_or(fixture),
            "--node" => node = args.next(),
            "--edge" => edge = args.next().and_then(|e| e.parse().ok()),
            "--run" => run = true,
            _ => {}
        }
    }
    ui::run_app("Inspector", 1440., 1060., move |window, cx| {
        ui_rich::bind_keys(cx);
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api.clone(), cx);
        let (pipeline, start) = match fixtures::start_route(&fixture) {
            Some((Route::Editor { pipeline, node }, _)) => (pipeline, node),
            _ => (PipelineId::new("pay-triage"), None),
        };
        let selection = match (edge, node.as_deref()) {
            (Some(index), _) => Selection::Edge(index),
            (None, Some("none")) => Selection::None,
            (None, Some(id)) => Selection::Node(NodeId::new(id)),
            (None, None) => start.map(Selection::Node).unwrap_or_default(),
        };
        let graph = api.with_world(|world| world.pipelines.iter().find(|g| g.id == pipeline).cloned());
        let draft = cx.new(|cx| {
            let mut draft = Draft::new(pipeline.clone());
            if let Some(graph) = graph {
                draft.load(graph, cx);
            }
            draft.select(selection, cx);
            if run {
                draft.show_run(Some(RunId::new("run-1423")), cx);
            }
            draft
        });
        cx.new(|cx| Stage::new(draft, window, cx))
    });
}

/// Empty ground where the canvas would be, the inspector on the right.
struct Stage {
    inspector: Entity<InspectorView>,
}

impl Stage {
    fn new(draft: Entity<Draft>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        check(&draft, cx);
        cx.subscribe(&draft, |_, draft, event: &DraftEvent, cx| {
            if *event == DraftEvent::Changed {
                check(&draft, cx);
            }
        })
        .detach();
        let inspector = cx.new(|cx| InspectorView::new(draft, window, cx));
        Self { inspector }
    }
}

/// Checks the draft's graph off the UI thread and stores the problems, as the frame does.
fn check(draft: &Entity<Draft>, cx: &mut gpui::App) {
    let Some(graph) = draft.read(cx).graph().cloned() else { return };
    let revision = draft.read(cx).revision();
    let task = hub::queries(cx).pipeline_problems(graph, cx);
    let draft = draft.downgrade();
    cx.spawn(async move |cx| {
        let problems = task.await;
        draft.update(cx, |draft, cx| draft.set_problems(revision, problems, cx)).ok();
    })
    .detach();
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root()
            .flex()
            .child(div().flex_1().min_w_0().h_full().bg(BG))
            .child(div().h_full().flex_none().border_l_1().border_color(LINE).child(self.inspector.clone()))
    }
}
