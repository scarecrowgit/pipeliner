//! The canvas over the fixture backend: PAY triage as the catalog serves it, its problems as
//! the backend finds them, and a run's instances on the nodes.

use std::sync::Arc;

use gpui::{AppContext as _, TestAppContext};
use hub::Hub;
use hub::fixtures::{FixtureApi, World};
use hub::model::{PipelineId, RunId};

use super::super::CanvasView;
use super::drawn;
use crate::Draft;

/// The fixed clock of the test world.
const NOW: i64 = 1_795_000_000_000;

#[gpui::test]
fn pay_triage_shows_its_problems_and_the_statuses_of_a_run(cx: &mut TestAppContext) {
    cx.update(ui::init);
    ui::motion::set_reduced(true);
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let _hub = cx.update(|cx| Hub::new_fixture(api, cx));
    let pipeline = PipelineId::new("pay-triage");
    let catalog = cx.update(hub::catalog);
    catalog.update(cx, |catalog, cx| catalog.ensure_pipeline(&pipeline, cx));
    cx.run_until_parked();
    let graph = catalog
        .read_with(cx, |catalog, _| catalog.pipeline(&pipeline).and_then(|l| l.value().cloned()))
        .and_then(|g| (*g).clone())
        .expect("the fixture has PAY triage");

    let draft = cx.new(|_| Draft::new(pipeline.clone()));
    draft.update(cx, |draft, cx| draft.load(graph.clone(), cx));
    let task = cx.update(|cx| hub::queries(cx).pipeline_problems(graph, cx));
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let put = slot.clone();
    cx.update(|cx| cx.spawn(async move |_| *put.borrow_mut() = Some(task.await)).detach());
    cx.run_until_parked();
    let problems = slot.borrow_mut().take().expect("the backend answered");
    let flagged: Vec<String> = problems.iter().filter_map(|p| p.node.as_ref().map(|n| n.to_string())).collect();
    draft.update(cx, |draft, cx| draft.set_problems(draft.revision(), problems, cx));
    draft.update(cx, |draft, cx| draft.show_run(Some(RunId::new("run-1423")), cx));

    let for_view = draft.clone();
    let (canvas, cx) = cx.add_window_view(move |window, cx| CanvasView::new(for_view, window, cx));
    cx.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(900.0)));
    cx.run_until_parked();
    canvas.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();

    for id in ["node-jira", "node-router", "node-fixer", "node-testw", "node-review", "node-jiraupd"] {
        assert!(cx.debug_bounds(id).is_some(), "{id} is drawn");
    }
    for port in ["port-router-rule1-out", "port-router-else-out", "port-review-approved-out", "port-review-send_back-out", "port-fixer-in"] {
        assert!(cx.debug_bounds(port).is_some(), "{port} is drawn");
    }
    assert!(!flagged.is_empty(), "the fixture's PAY triage has node problems (Jira is not there yet)");
    for node in &flagged {
        let error = Box::leak(format!("problem-{node}-error").into_boxed_str());
        let warning = Box::leak(format!("problem-{node}-warning").into_boxed_str());
        assert!(cx.debug_bounds(error).or_else(|| cx.debug_bounds(warning)).is_some(), "{node} carries its mark");
    }

    // Run 1423: three Fixer variants done, two test writers, the review waiting.
    for pill in ["status-fixer-0", "status-fixer-1", "status-fixer-2", "status-testw-1", "status-review-0", "status-jira-0"] {
        assert!(cx.debug_bounds(pill).is_some(), "{pill} is drawn");
    }
    assert!(!drawn(&canvas, cx, "status-fixer-3"));
    assert!(!drawn(&canvas, cx, "status-repro-0"), "the reproducer did not run");

    draft.update(cx, |draft, cx| draft.show_run(None, cx));
    cx.run_until_parked();
    assert!(!drawn(&canvas, cx, "status-fixer-0"), "no run, no statuses");
}
