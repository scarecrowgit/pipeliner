//! Each kind shows its fields with the graph's values; steppers, switches and pickers edit the
//! right field; router rules, action kinds, edges, problems and the pipeline itself.

use std::sync::Arc;

use gpui::TestAppContext;
use hub::fixtures::{FixtureApi, World};
use hub::model::{ActionNode, FileMode, ModelRef, NodeId, NodeKind};

use super::{NOW, click, graph, graph_now, node_now, open, problem, select, shown, text};
use crate::Selection;

#[gpui::test]
fn every_kind_shows_its_fields(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::None);
    assert_eq!(text(&t, cx, "insp-name"), "Demo");
    assert_eq!(text(&t, cx, "insp-description"), "A demo.");
    for s in ["insp-max-parallel", "insp-max-loops", "insp-budget", "insp-workdir-add", "insp-wd-0-edit", "insp-wd-1-remove"] {
        assert!(shown(cx, s), "pipeline: {s}");
    }
    assert!(!shown(cx, "insp-delete"));

    let cases: [(&str, &[&str]); 10] = [
        ("src", &["insp-source"]),
        ("triage", &["insp-rule-0-label", "insp-rule-0-when", "insp-rule-1-ask", "insp-rule-1-model", "insp-rule-add", "insp-rule-0-down"]),
        ("fixer", &["insp-runnable", "insp-role", "insp-model-0", "insp-workdir", "insp-variants", "insp-tools-access-2", "insp-spawn", "insp-budget", "insp-max-steps", "insp-files-out-0", "insp-skill-add"]),
        ("lead", &["insp-runnable", "insp-variants"]),
        ("helper", &["insp-role", "insp-per-run", "insp-max-steps", "insp-tools-access-2"]),
        ("docs", &["insp-path", "insp-mode-0", "insp-workdir"]),
        ("review", &["insp-send-back", "insp-skip-if-empty"]),
        ("test", &["insp-action-kind", "insp-command", "insp-tests", "insp-parse", "insp-workdir"]),
        ("pr", &["insp-draft", "insp-push", "insp-workdir"]),
        ("nested", &["insp-name"]),
    ];
    for (id, selectors) in cases {
        select(&t, cx, Selection::Node(id.into()));
        for s in ["insp-name", "insp-id", "insp-delete"].iter().chain(selectors) {
            assert!(shown(cx, s), "{id}: {s}");
        }
        assert_eq!(text(&t, cx, "insp-id"), id);
    }
    select(&t, cx, Selection::Node("triage".into()));
    assert_eq!(text(&t, cx, "insp-rule-0-when"), "type == \"bug\"");
    assert_eq!(text(&t, cx, "insp-rule-1-ask"), "Touches the UI?");
    select(&t, cx, Selection::Node("fixer".into()));
    assert_eq!(text(&t, cx, "insp-role"), "Fix it.");
    select(&t, cx, Selection::Node("helper".into()));
    assert_eq!(text(&t, cx, "insp-max-steps"), "10");
    select(&t, cx, Selection::Node("docs".into()));
    assert_eq!(text(&t, cx, "insp-path"), "docs/**");
    select(&t, cx, Selection::Node("test".into()));
    assert_eq!(text(&t, cx, "insp-command"), "cargo test");
}

#[gpui::test]
fn steppers_switches_and_pickers_edit_the_right_field(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    let agent = |t: &super::Open, cx: &mut gpui::VisualTestContext| match node_now(t, cx, "fixer").kind {
        NodeKind::Agent(agent) => agent,
        _ => panic!("agent"),
    };
    click(cx, "insp-variants-inc");
    assert_eq!(agent(&t, cx).variants, 3);
    click(cx, "insp-spawn");
    assert_eq!(agent(&t, cx).spawn.map(|s| s.enabled), Some(true));
    click(cx, "insp-spawn-depth-inc");
    assert_eq!(agent(&t, cx).spawn.map(|s| s.max_depth), Some(3));
    click(cx, "insp-workdir");
    click(cx, "insp-workdir-1");
    assert_eq!(agent(&t, cx).workdir, "spare");
    click(cx, "insp-model-1");
    assert_eq!(agent(&t, cx).model, Some(ModelRef::new("anthropic", "sonnet-5.5")));
    click(cx, "insp-tools-access-0");
    assert_eq!(agent(&t, cx).tools.map(|t| t.access), Some(hub::model::ToolAccess::ReadOnly));

    select(&t, cx, Selection::Node("review".into()));
    click(cx, "insp-skip-if-empty");
    click(cx, "insp-send-back");
    click(cx, "insp-send-back-2");
    let NodeKind::Review(review) = node_now(&t, cx, "review").kind else { panic!("review") };
    assert!(review.skip_if_empty);
    assert_eq!(review.send_back_to, Some(NodeId::new("lead")));

    select(&t, cx, Selection::Node("docs".into()));
    click(cx, "insp-mode-1");
    let NodeKind::File(file) = node_now(&t, cx, "docs").kind else { panic!("file") };
    assert_eq!(file.mode, FileMode::Write);

    select(&t, cx, Selection::None);
    click(cx, "insp-max-parallel-inc");
    assert_eq!(graph_now(&t, cx).limits.max_parallel, 3);
}

#[gpui::test]
fn router_rules_add_remove_and_reorder(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("triage".into()));
    let ids = |t: &super::Open, cx: &mut gpui::VisualTestContext| match node_now(t, cx, "triage").kind {
        NodeKind::Router(router) => router.rules.into_iter().map(|r| r.id).collect::<Vec<_>>(),
        _ => panic!("router"),
    };
    click(cx, "insp-rule-add");
    assert_eq!(ids(&t, cx), ["bug", "ui", "rule3"]);
    click(cx, "insp-rule-0-down");
    assert_eq!(ids(&t, cx), ["ui", "bug", "rule3"]);
    assert_eq!(text(&t, cx, "insp-rule-1-when"), "type == \"bug\"", "fields follow their rule");
    click(cx, "insp-rule-1-remove");
    assert_eq!(ids(&t, cx), ["ui", "rule3"]);
    assert!(!graph_now(&t, cx).edges.iter().any(|e| e.port == "bug"), "the rule's edge goes with it");
    click(cx, "insp-rule-0-mode-0");
    let NodeKind::Router(router) = node_now(&t, cx, "triage").kind else { panic!() };
    assert!(router.rules[0].ask.is_none());
}

#[gpui::test]
fn changing_the_action_kind_keeps_the_workdir(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("pr".into()));
    click(cx, "insp-action-kind");
    click(cx, "insp-action-kind-0");
    let NodeKind::Action(action) = node_now(&t, cx, "pr").kind else { panic!() };
    assert!(matches!(action, ActionNode::Command { ref workdir, .. } if workdir == "code"));
    assert!(shown(cx, "insp-command"));
    click(cx, "insp-action-kind");
    click(cx, "insp-action-kind-3");
    let NodeKind::Action(action) = node_now(&t, cx, "pr").kind else { panic!() };
    assert!(matches!(action, ActionNode::PullRequest { ref workdir, .. } if workdir == "code"));
}

#[gpui::test]
fn the_drafts_problems_for_a_node_show(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Node("fixer".into()));
    assert!(!shown(cx, "insp-problems"));
    t.draft.update(cx, |d, cx| {
        let revision = d.revision();
        d.set_problems(revision, vec![problem("fixer", "Fixer uses an unknown workdir “x”"), problem("lead", "Lead has no role")], cx);
    });
    assert!(shown(cx, "insp-problems"));
    select(&t, cx, Selection::Node("docs".into()));
    assert!(!shown(cx, "insp-problems"), "only the selected node's problems");
}

#[gpui::test]
fn an_edge_changes_port_and_is_removed(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::Edge(1));
    assert!(shown(cx, "insp-edge-port"));
    click(cx, "insp-edge-port");
    click(cx, "insp-edge-port-1");
    assert_eq!(graph_now(&t, cx).edges[1].port, "ui");
    click(cx, "insp-edge-kind-3");
    assert_eq!(graph_now(&t, cx).edges[1].kind, hub::model::EdgeKind::Back);
    click(cx, "insp-delete");
    assert_eq!(graph_now(&t, cx).edges.len(), 8);
    assert_eq!(t.draft.read_with(cx, |d, _| d.selection().clone()), Selection::None);
}

#[gpui::test]
fn a_workdir_in_use_is_not_removed(cx: &mut TestAppContext) {
    let (t, cx) = open(cx, graph(), Selection::None);
    click(cx, "insp-wd-0-remove");
    assert_eq!(graph_now(&t, cx).workdirs.len(), 2, "Code is used by nodes");
    click(cx, "insp-wd-1-remove");
    assert_eq!(graph_now(&t, cx).workdirs.len(), 1);
    click(cx, "insp-workdir-add");
    assert_eq!(graph_now(&t, cx).workdirs.len(), 2);
    assert!(shown(cx, "insp-wd-1-path"), "a new workdir opens its fields");
}

#[gpui::test]
fn the_fixture_pipeline_shows_fixer(cx: &mut TestAppContext) {
    let api = FixtureApi::new(World::acme(NOW));
    let pay = Arc::new(api).with_world(|w| w.pipelines.iter().find(|g| g.id.as_str() == "pay-triage").cloned());
    let Some(pay) = pay else { panic!("the fixture has PAY triage") };
    let (t, cx) = open(cx, pay, Selection::Node("fixer".into()));
    assert_eq!(text(&t, cx, "insp-name"), "Fixer");
    assert!(text(&t, cx, "insp-role").starts_with("Reproduce the bug"));
    assert!(shown(cx, "insp-model-0"), "opus, sonnet and haiku as one control");
    click(cx, "insp-variants-dec");
    let NodeKind::Agent(agent) = node_now(&t, cx, "fixer").kind else { panic!() };
    assert_eq!(agent.variants, 2);
}
