//! Loading, saving, checking, following the file, undo and redo, switching pipelines.

use gpui::TestAppContext;
use hub::api::Event;
use hub::model::{NodeId, PipelineId, Severity};

use super::{PAY, calls, click, edit, graph, on_disk, open, wait, wait_check, wait_save};
use crate::Selection;
use crate::frame::load_save::{Load, SaveState};

#[gpui::test]
async fn the_draft_loads_with_the_routed_node_selected(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert_eq!(graph(&t, cx).name, "PAY triage");
    t.view.read_with(cx, |v, cx| {
        assert_eq!(v.doc.load, Load::Ready);
        assert_eq!(v.title(cx).as_ref(), "PAY triage");
        assert_eq!(v.draft().read(cx).selection(), &Selection::Node(NodeId::new("fixer")));
        assert_eq!(v.save_state(cx), SaveState::Saved);
    });
}

#[gpui::test]
async fn a_missing_pipeline_says_so(cx: &mut TestAppContext) {
    let (t, cx) = super::open_with(cx, |w| w.pipelines.retain(|g| g.id.as_str() != PAY));
    t.view.read_with(cx, |v, _| assert_eq!(v.doc.load, Load::Missing));
    assert!(super::shown(cx, "editor-back"));
}

#[gpui::test]
async fn an_edit_is_saved_after_the_debounce(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Rename", |g| g.name = "PAY triage 2".into());
    t.view.read_with(cx, |v, cx| assert_eq!(v.save_state(cx), SaveState::Unsaved));
    assert!(calls(&t.api, "save_pipeline").is_empty(), "nothing is saved before the debounce");
    wait_save(cx);
    assert_eq!(calls(&t.api, "save_pipeline"), [PAY]);
    assert_eq!(on_disk(&t).name, "PAY triage 2");
    t.view.read_with(cx, |v, cx| {
        assert!(!v.draft().read(cx).is_dirty());
        assert_eq!(v.save_state(cx), SaveState::Saved);
        assert!(v.doc.conflict.is_none(), "our own save is not someone else's change");
    });
}

#[gpui::test]
async fn a_failed_save_keeps_the_edits_unsaved_and_says_why(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    t.api.fail_next("save_pipeline");
    edit(&t, cx, "Rename", |g| g.name = "Other".into());
    wait_save(cx);
    t.view.read_with(cx, |v, cx| {
        assert!(v.draft().read(cx).is_dirty());
        assert!(matches!(v.save_state(cx), SaveState::Failed(_)), "{:?}", v.save_state(cx));
    });
    assert_eq!(on_disk(&t).name, "PAY triage");
    // ⌘S tries again.
    cx.simulate_keystrokes("cmd-s");
    cx.run_until_parked();
    assert_eq!(on_disk(&t).name, "Other");
    t.view.read_with(cx, |v, cx| assert_eq!(v.save_state(cx), SaveState::Saved));
}

#[gpui::test]
async fn problems_are_asked_for_the_current_revision(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    let draft = t.view.read_with(cx, |v, _| v.draft().clone());
    draft.read_with(cx, |d, _| {
        assert!(d.problems_are_current());
        assert_eq!(d.problems().len(), 1, "the fixture flags the Jira source");
        assert_eq!(d.problems()[0].severity, Severity::Error);
    });
    // Two edits in a row: only the graph after the second is checked.
    edit(&t, cx, "Rename", |g| g.name = "A".into());
    wait(cx, super::super::load_save::CHECK_DELAY / 2);
    edit(&t, cx, "Source", |g| {
        if let Some(node) = g.nodes.iter_mut().find(|n| n.id.as_str() == "jira") {
            node.kind = hub::api::NodeKind::Source(hub::api::SourceNode { source: hub::api::SourceKind::Manual });
        }
    });
    assert!(!draft.read_with(cx, |d, _| d.problems_are_current()));
    wait_check(cx);
    draft.read_with(cx, |d, _| {
        assert!(d.problems_are_current());
        assert!(d.problems().is_empty(), "the answer is for the graph without Jira");
    });
}

#[gpui::test]
async fn undo_and_redo_buttons_follow_the_draft(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    click(cx, "editor-undo");
    assert_eq!(graph(&t, cx).name, "PAY triage", "nothing to undo yet");
    edit(&t, cx, "Rename", |g| g.name = "Renamed".into());
    click(cx, "editor-undo");
    assert_eq!(graph(&t, cx).name, "PAY triage");
    click(cx, "editor-redo");
    assert_eq!(graph(&t, cx).name, "Renamed");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(graph(&t, cx).name, "PAY triage");
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(graph(&t, cx).name, "Renamed");
}

#[gpui::test]
async fn another_pipeline_saves_the_old_draft_and_loads_the_new(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Rename", |g| g.name = "Edited".into());
    let audit = PipelineId::new("nightly-audit");
    t.view.update_in(cx, |v, window, cx| v.show(audit.clone(), None, window, cx));
    cx.run_until_parked();
    assert_eq!(calls(&t.api, "save_pipeline"), [PAY]);
    assert_eq!(on_disk(&t).name, "Edited");
    assert_eq!(graph(&t, cx).name, "Nightly audit");
    t.view.read_with(cx, |v, _| assert_eq!(v.pipeline(), &audit));
}

#[gpui::test]
async fn an_external_change_reloads_an_unedited_draft(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    t.api.with_world(|w| {
        if let Some(g) = w.pipelines.iter_mut().find(|g| g.id.as_str() == PAY) {
            g.name = "Changed elsewhere".into();
        }
    });
    t.api.emit(Event::CatalogChanged);
    cx.run_until_parked();
    assert_eq!(graph(&t, cx).name, "Changed elsewhere");
    t.view.read_with(cx, |v, cx| assert!(!v.draft().read(cx).is_dirty()));
}

#[gpui::test]
async fn an_external_change_under_edits_asks_reload_or_keep_mine(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Rename", |g| g.name = "Mine".into());
    t.api.with_world(|w| {
        if let Some(g) = w.pipelines.iter_mut().find(|g| g.id.as_str() == PAY) {
            g.description = "Theirs".into();
        }
    });
    t.api.emit(Event::CatalogChanged);
    cx.run_until_parked();
    assert!(super::shown(cx, "editor-reload"));
    wait_save(cx);
    assert!(calls(&t.api, "save_pipeline").is_empty(), "no save over their change until asked");
    click(cx, "editor-keep-mine");
    assert_eq!(on_disk(&t).name, "Mine");
    t.view.read_with(cx, |v, _| assert!(v.doc.conflict.is_none()));
}

#[gpui::test]
async fn reload_takes_the_file_and_drops_the_edits(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    edit(&t, cx, "Rename", |g| g.name = "Mine".into());
    t.api.with_world(|w| {
        if let Some(g) = w.pipelines.iter_mut().find(|g| g.id.as_str() == PAY) {
            g.name = "Theirs".into();
        }
    });
    t.api.emit(Event::CatalogChanged);
    cx.run_until_parked();
    click(cx, "editor-reload");
    assert_eq!(graph(&t, cx).name, "Theirs");
    t.view.read_with(cx, |v, cx| assert!(!v.draft().read(cx).is_dirty()));
}
