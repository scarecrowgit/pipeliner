//! Where keys go: a node added from the toolbar takes typing in its name, canvas keys work
//! again after a click on selectable text, and a text field keeps Delete and
//! Backspace.

use gpui::{TestAppContext, VisualTestContext};
use hub::model::{NodeId, NodeKind};

use super::{click, graph, open, wait_check};
use crate::Selection;

fn keys(cx: &mut VisualTestContext, keys: &str) {
    cx.simulate_keystrokes(keys);
    cx.run_until_parked();
}

fn selection(t: &super::Open, cx: &mut VisualTestContext) -> Selection {
    t.view.read_with(cx, |v, cx| v.draft().read(cx).selection().clone())
}

#[gpui::test]
async fn a_node_added_from_the_toolbar_takes_typing_in_its_name(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    click(cx, "toolbar-add-orchestrator");
    let Selection::Node(id) = selection(&t, cx) else { panic!("the new node is selected") };
    // Through the window's key dispatch: only that catches a field that never got focus.
    keys(cx, "space t w o");
    let node = graph(&t, cx).node(&id).cloned().expect("the node was added");
    assert!(matches!(node.kind, NodeKind::Orchestrator(_)));
    assert_eq!(node.name, "Orchestrator two", "the caret is in the new node's name");
    assert_eq!(graph(&t, cx).node(&NodeId::new("fixer")).map(|n| n.name.clone()).as_deref(), Some("Fixer"));
    // Backspace in the name edits the name; the node stays.
    keys(cx, "backspace");
    assert_eq!(graph(&t, cx).node(&id).map(|n| n.name.clone()).as_deref(), Some("Orchestrator tw"));
}

#[gpui::test]
async fn canvas_keys_work_after_a_click_on_a_problem(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    click(cx, "dock-tabs-2");
    // A click on the message's text: it takes focus (to copy) and selects the node.
    click(cx, "problem-0-message");
    let jira = NodeId::new("jira");
    assert_eq!(selection(&t, cx), Selection::Node(jira.clone()));
    keys(cx, "escape");
    assert_eq!(selection(&t, cx), Selection::None, "Escape reaches the canvas");

    click(cx, "problem-0-message");
    keys(cx, "delete");
    assert!(graph(&t, cx).node(&jira).is_none(), "Delete reaches the canvas");
}

#[gpui::test]
async fn delete_in_a_field_after_a_problem_click_stays_in_the_field(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    wait_check(cx);
    click(cx, "dock-tabs-2");
    click(cx, "problem-0-message");
    click(cx, "insp-name");
    keys(cx, "end backspace delete");
    let jira = graph(&t, cx).node(&NodeId::new("jira")).cloned().expect("the node stays");
    assert_eq!(jira.name, "Jira pulle");
}
