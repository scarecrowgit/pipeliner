//! The pipeline canvas alone, in a window, over a draft of the fixture's pipeline.
//!
//! `cargo run -p screen_editor --example canvas [-- --fixture pipeline-editor] [--run run-1423]`
//!
//! The pipeline is the one the fixture's start route opens in the editor (PAY triage for
//! `pipeline-editor`); `--run` puts a run of it on show, so the nodes carry their statuses.

use std::sync::Arc;

use gpui::{Context, Entity, IntoElement, Render, Window, div, prelude::*};
use hub::fixtures::{self, FixtureApi};
use hub::model::{PipelineId, RunId};
use hub::{Hub, Route};
use screen_editor::canvas::CanvasEvent;
use screen_editor::{CanvasView, Draft, Selection};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut fixture = "pipeline-editor".to_string();
    let mut run: Option<String> = None;
    let mut connect: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" => fixture = args.next().unwrap_or(fixture),
            "--run" => run = args.next(),
            "--connect" => connect = args.next(),
            _ => {}
        }
    }
    ui::run_app("Pipeline canvas", 1440., 900., move |window, cx| {
        let api = Arc::new(FixtureApi::scenario(&fixture).unwrap_or_default());
        Hub::new_fixture(api, cx);
        let (pipeline, node) = match fixtures::start_route(&fixture) {
            Some((Route::Editor { pipeline, node }, _)) => (pipeline, node),
            _ => (PipelineId::new("pay-triage"), None),
        };
        let draft = cx.new(|_| Draft::new(pipeline.clone()));
        if let Some(run) = run {
            draft.update(cx, |draft, cx| draft.show_run(Some(RunId::new(run)), cx));
        }
        cx.new(|cx| Stage::new(draft, node.map(Selection::Node), connect, window, cx))
    });
}

/// The canvas filling the window; loads the draft from the catalog and checks it.
struct Stage {
    canvas: Entity<CanvasView>,
}

impl Stage {
    fn new(
        draft: Entity<Draft>,
        select: Option<Selection>,
        connect: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let canvas = cx.new(|cx| CanvasView::new(draft.clone(), window, cx));
        cx.subscribe(&canvas, |_, _, event: &CanvasEvent, _| println!("canvas: {event:?}")).detach();
        let pipeline = draft.read(cx).pipeline().clone();
        let catalog = hub::catalog(cx);
        catalog.update(cx, |catalog, cx| catalog.ensure_pipeline(&pipeline, cx));
        let load = draft.clone();
        let pending = canvas.clone();
        cx.observe(&catalog, move |_, catalog, cx| {
            let graph = catalog.read(cx).pipeline(&pipeline).and_then(|l| l.value()).and_then(|g| (**g).clone());
            if let Some(graph) = graph
                && load.read(cx).graph().is_none()
            {
                load.update(cx, |draft, cx| {
                    draft.load(graph, cx);
                    if let Some(select) = select.clone() {
                        draft.select(select, cx);
                    }
                });
                if let Some([node, port, x, y]) = connect.as_deref().map(|c| c.split(':').collect::<Vec<_>>()).and_then(|v| <[&str; 4]>::try_from(v).ok()) {
                    let at = gpui::point(x.parse().unwrap_or(0.0), y.parse().unwrap_or(0.0));
                    pending.update(cx, |canvas, cx| canvas.start_connect(&node.into(), port, at, cx));
                }
            }
        })
        .detach();
        // Re-check the graph after every change, as the frame will.
        cx.subscribe(&draft, |_, draft, event, cx| {
            if *event != screen_editor::DraftEvent::Changed {
                return;
            }
            let Some(graph) = draft.read(cx).graph().cloned() else { return };
            let revision = draft.read(cx).revision();
            let task = hub::queries(cx).pipeline_problems(graph, cx);
            cx.spawn(async move |_, cx| {
                let problems = task.await;
                draft.update(cx, |draft, cx| draft.set_problems(revision, problems, cx)).ok();
            })
            .detach();
        })
        .detach();
        Self { canvas }
    }
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().flex().child(div().flex_1().min_w_0().h_full().child(self.canvas.clone()))
    }
}
