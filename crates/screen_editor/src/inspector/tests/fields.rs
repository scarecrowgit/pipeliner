//! Typing into fields: edits as the person types, one undo step per field, undo shows in the
//! field, id renames, Tab, and a selection change while a field has focus.

use gpui::TestAppContext;
use hub::model::{ActionNode, NodeId, NodeKind};

use super::{click, graph, graph_now, keys, node_now, open, select, text, type_text};
use crate::Selection;

#[gpui::test]
fn typing_edits_the_graph_one_undo_step_per_field(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    click(cx, "insp-name");
    keys(cx, "end");
    type_text(cx, " two");
    assert_eq!(node_now(&t, cx, "fixer").name, "Fixer two");

    click(cx, "insp-role");
    keys(cx, "ctrl-end");
    type_text(cx, " Now.");
    let NodeKind::Agent(agent) = node_now(&t, cx, "fixer").kind else { panic!("agent") };
    assert_eq!(agent.role, "Fix it. Now.");

    t.draft.update(cx, |d, cx| d.undo(cx));
    cx.run_until_parked();
    let NodeKind::Agent(agent) = node_now(&t, cx, "fixer").kind else { panic!("agent") };
    assert_eq!(agent.role, "Fix it.");
    assert_eq!(text(&t, cx, "insp-role"), "Fix it.", "undo shows in the field");
    assert_eq!(node_now(&t, cx, "fixer").name, "Fixer two", "the name was its own step");

    t.draft.update(cx, |d, cx| d.undo(cx));
    cx.run_until_parked();
    assert_eq!(node_now(&t, cx, "fixer").name, "Fixer");
    assert_eq!(text(&t, cx, "insp-name"), "Fixer");
    assert_eq!(t.draft.read_with(cx, |d, _| d.can_undo()), None);
}

#[gpui::test]
fn typing_a_command_is_one_step_and_undo_restores_it(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("test".into()));
    click(cx, "insp-command");
    keys(cx, "ctrl-end");
    type_text(cx, " --all");
    let command = |t: &super::Open, cx: &mut gpui::VisualTestContext| match node_now(t, cx, "test").kind {
        NodeKind::Action(ActionNode::Command { command, .. }) => command,
        _ => panic!("command"),
    };
    assert_eq!(command(&t, cx), "cargo test --all");
    t.draft.update(cx, |d, cx| d.undo(cx));
    cx.run_until_parked();
    assert_eq!(command(&t, cx), "cargo test");
    assert_eq!(text(&t, cx, "insp-command"), "cargo test");
}

#[gpui::test]
fn another_selection_while_typing_does_not_write_into_it(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    click(cx, "insp-name");
    keys(cx, "end");
    type_text(cx, "X");
    assert_eq!(node_now(&t, cx, "fixer").name, "FixerX");

    select(&t, cx, Selection::Node("lead".into()));
    type_text(cx, "Y");
    assert_eq!(node_now(&t, cx, "lead").name, "Lead");
    assert_eq!(node_now(&t, cx, "fixer").name, "FixerX");
    assert_eq!(text(&t, cx, "insp-name"), "Lead");
}

#[gpui::test]
fn renaming_an_id_re_points_edges_and_refuses_a_duplicate(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    click(cx, "insp-id");
    keys(cx, "end");
    type_text(cx, "2");
    let g = graph_now(&t, cx);
    assert!(g.node(&"fixer2".into()).is_some());
    assert!(g.edges.iter().any(|e| e.from.as_str() == "triage" && e.to.as_str() == "fixer2"));
    let NodeKind::Review(review) = &g.node(&"review".into()).map(|n| n.kind.clone()).unwrap_or_else(|| panic!()) else { panic!() };
    assert_eq!(review.send_back_to, Some(NodeId::new("fixer2")));
    assert_eq!(t.draft.read_with(cx, |d, _| d.selection().clone()), Selection::Node("fixer2".into()));
    assert_eq!(text(&t, cx, "insp-id"), "fixer2", "the field stays as typed");

    keys(cx, "ctrl-a");
    type_text(cx, "lead");
    let g = graph_now(&t, cx);
    assert_eq!(g.nodes.iter().filter(|n| n.id.as_str() == "lead").count(), 1, "no duplicate id");
    assert!(g.node(&"lea".into()).is_some(), "the last valid id stays");
    assert_eq!(text(&t, cx, "insp-id"), "lead");
}

#[gpui::test]
fn tab_moves_from_the_name_to_the_id(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("docs".into()));
    click(cx, "insp-name");
    keys(cx, "tab");
    type_text(cx, "x");
    assert!(graph_now(&t, cx).node(&"docsx".into()).is_some());
}

#[gpui::test]
fn a_dollar_field_checks_its_shape_only(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    click(cx, "insp-budget");
    type_text(cx, "0.5");
    let budget = |t: &super::Open, cx: &mut gpui::VisualTestContext| match node_now(t, cx, "fixer").kind {
        NodeKind::Agent(agent) => agent.budget_usd,
        _ => None,
    };
    assert_eq!(budget(&t, cx), Some(0.5));
    type_text(cx, "x");
    assert_eq!(budget(&t, cx), Some(0.5), "not a number: nothing written");
    assert_eq!(text(&t, cx, "insp-budget"), "0.5x", "what was typed stays");
}

#[gpui::test]
fn a_new_rule_and_a_new_workdir_take_the_caret(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("triage".into()));
    click(cx, "insp-rule-add");
    type_text(cx, "X");
    let NodeKind::Router(router) = node_now(&t, cx, "triage").kind else { panic!("router") };
    let label = &router.rules.last().expect("a rule was added").label;
    assert!(label.contains('X') && label != "X", "typing goes into the new rule's label: {label}");

    select(&t, cx, Selection::None);
    click(cx, "insp-workdir-add");
    type_text(cx, "X");
    let workdirs = graph_now(&t, cx).workdirs;
    let name = &workdirs.last().expect("a workdir was added").name;
    assert!(name.contains('X'), "typing goes into the new workdir's name: {name}");
}
