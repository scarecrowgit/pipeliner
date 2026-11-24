//! What nodes show, where their ports are, which edges fit, and new nodes: plain functions.

use gpui::{SharedString, point};
use hub::model::{
    EdgeKind, NodeInstance, NodeKind, NodeStatus, PORT_ELSE, PORT_OUT, PipelineId, PipelineNode, Problem, Run, RunId,
    RunState, Severity, SpanKind, Usage,
};
use ui_rich::canvas::{HEADER_PORT_Y, NODE_WIDTH, Outputs, row_center_y};

use super::super::add::{NewNode, new_node};
use super::super::look::{PORT_SEND_BACK, look};
use super::super::marks::{instance_marks, problem_marks};
use super::super::rules::{edge_kind, remove_edge, remove_node};
use super::super::scene::Scene;
use super::{graph, node};
use crate::Selection;

fn rows(graph: &hub::model::GraphDef, id: &str) -> Vec<(String, String)> {
    let node = graph.node(&id.into()).expect("node exists");
    look(graph, node).rows.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn sub(graph: &hub::model::GraphDef, id: &str) -> SharedString {
    look(graph, graph.node(&id.into()).expect("node exists")).subtitle
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn every_kind_shows_its_rows() {
    let mut g = graph();
    g.nodes.push(node("nested", "Nested", 0.0, 600.0, NodeKind::Pipeline(PipelineNode { pipeline: PipelineId::new("other") })));

    assert_eq!(sub(&g, "src"), "issues on the board");
    assert_eq!(rows(&g, "src"), pairs(&[("pulls", "issue")]));

    assert_eq!(sub(&g, "router"), "first match wins");
    assert_eq!(rows(&g, "router"), pairs(&[("rule 1", "Bug"), ("rule 2", "Story"), ("else", "skip")]));

    assert_eq!(sub(&g, "fixer"), "opus-5.5 · ×3");
    // Read-only is three tools, plus one allowed by name.
    assert_eq!(rows(&g, "fixer"), pairs(&[("tools", "4"), ("spawns", "depth ≤2")]));

    assert_eq!(sub(&g, "testw"), "by Fixer");
    assert_eq!(rows(&g, "testw"), pairs(&[("per run", "≤3"), ("returns to", "Fixer")]));

    assert_eq!(sub(&g, "docs"), "docs/**");
    assert_eq!(rows(&g, "docs"), pairs(&[("mode", "read"), ("in", "payments-api")]));

    assert_eq!(sub(&g, "review"), "ship 1 of N");
    assert_eq!(rows(&g, "review"), pairs(&[("on approve", "Open PR"), ("on send back", "off")]));

    assert_eq!(sub(&g, "pr"), "fix/{key} → main");
    assert_eq!(rows(&g, "pr"), pairs(&[("repo", "payments-api")]));

    let nested = look(&g, g.node(&"nested".into()).expect("added"));
    assert!(nested.soon);
    assert_eq!(rows(&g, "nested"), pairs(&[("status", "Coming soon")]));
}

#[test]
fn each_kind_has_the_ports_it_can_use() {
    let g = graph();
    let of = |id: &str| look(&g, g.node(&id.into()).expect("node exists"));
    let src = of("src");
    assert!(!src.input, "a source has nothing coming in");
    assert_eq!(src.ports, [PORT_OUT]);
    let router = of("router");
    assert_eq!(router.outputs, Outputs::PerRow);
    assert_eq!(router.ports, ["bug", "story", PORT_ELSE]);
    let review = of("review");
    assert_eq!(review.outputs, Outputs::PerRow);
    assert_eq!(review.ports, ["approved", PORT_SEND_BACK]);
    let testw = of("testw");
    assert!(testw.spawned);
    assert_eq!(testw.outputs, Outputs::None);
    assert!(!of("docs").input, "a file that is read has nothing coming in");
}

#[test]
fn router_ports_sit_on_their_rule_rows() {
    let g = graph();
    let scene = Scene::build(&g, &Selection::None);
    let router = scene.node(&"router".into()).expect("placed");
    let right = 212.0 + NODE_WIDTH;
    assert_eq!(router.out_port("bug"), Some(point(right, 120.0 + row_center_y(0))));
    assert_eq!(router.out_port("story"), Some(point(right, 120.0 + row_center_y(1))));
    assert_eq!(router.out_port(PORT_ELSE), Some(point(right, 120.0 + row_center_y(2))));
    assert_eq!(router.out_port("nope"), None);
    assert_eq!(router.in_port(), Some(point(212.0, 120.0 + HEADER_PORT_Y)));

    // The edge of rule "bug" leaves that row and arrives at the fixer's in-port.
    let bug = scene.edges.iter().find(|e| e.index == 1).expect("drawn");
    assert_eq!(bug.edge.from, point(right, 120.0 + row_center_y(0)));
    assert_eq!(bug.edge.to, point(404.0, 40.0 + HEADER_PORT_Y));
    assert_eq!(bug.edge.label.as_ref().map(|l| l.as_ref()), Some("×3"), "a router's edge into an agent shows its variants");
}

#[test]
fn the_selected_node_lights_its_routes_only() {
    use ui_rich::canvas::EdgeKind as Stroke;
    let g = graph();
    let scene = Scene::build(&g, &Selection::Node("fixer".into()));
    let stroke = |i: usize| scene.edges.iter().find(|e| e.index == i).map(|e| e.edge.kind);
    assert_eq!(stroke(1), Some(Stroke::Selected), "router → fixer");
    assert_eq!(stroke(4), Some(Stroke::Selected), "fixer → review");
    assert_eq!(stroke(2), Some(Stroke::Dim), "context stays dim");
    assert_eq!(stroke(3), Some(Stroke::Spawn), "spawn stays a spawn");
    assert_eq!(stroke(0), Some(Stroke::Route));
}

#[test]
fn an_edge_fits_its_ends_or_is_refused() {
    let mut g = graph();
    g.nodes.push(node("helper", "Helper", 600.0, 300.0, NodeKind::Subagent(Default::default())));
    let kind = |from: &str, port: &str, to: &str| edge_kind(&g, &from.into(), port, &to.into());

    assert_eq!(kind("router", "story", "fixer"), Some(EdgeKind::Route));
    assert_eq!(kind("router", PORT_ELSE, "review"), Some(EdgeKind::Route));
    assert_eq!(kind("fixer", PORT_OUT, "helper"), Some(EdgeKind::Spawn));
    assert_eq!(kind("review", PORT_SEND_BACK, "fixer"), Some(EdgeKind::Back), "the fixer is upstream");
    assert_eq!(kind("review", PORT_SEND_BACK, "pr"), None, "the PR is downstream");
    assert_eq!(kind("docs", PORT_OUT, "review"), None, "files feed agents only");
    assert_eq!(kind("docs", PORT_OUT, "fixer"), None, "already drawn");
    assert_eq!(kind("fixer", PORT_OUT, "src"), None, "nothing goes into a source");
    assert_eq!(kind("router", "story", "helper"), None, "only an agent attaches a sub-agent");
    assert_eq!(kind("testw", PORT_OUT, "review"), None, "a sub-agent has no way out");
    assert_eq!(kind("fixer", PORT_OUT, "fixer"), None);
    assert_eq!(kind("fixer", PORT_OUT, "missing"), None);
}

#[test]
fn removing_a_node_takes_its_edges_and_send_back() {
    let mut g = graph();
    super::super::rules::add_edge(&mut g, &"review".into(), PORT_SEND_BACK, &"fixer".into(), EdgeKind::Back);
    let NodeKind::Review(review) = &g.node(&"review".into()).expect("there").kind else { panic!("a review") };
    assert_eq!(review.send_back_to, Some("fixer".into()), "a back edge sets where work goes back to");

    remove_node(&mut g, &"fixer".into());
    assert!(g.node(&"fixer".into()).is_none());
    assert!(g.edges.iter().all(|e| e.from.as_str() != "fixer" && e.to.as_str() != "fixer"));
    let NodeKind::Review(review) = &g.node(&"review".into()).expect("there").kind else { panic!("a review") };
    assert_eq!(review.send_back_to, None);

    let before = g.edges.len();
    remove_edge(&mut g, 0);
    remove_edge(&mut g, 99);
    assert_eq!(g.edges.len(), before - 1);
}

#[test]
fn a_new_node_gets_a_fresh_id_a_name_and_a_free_spot() {
    let mut g = graph();
    let first = new_node(&g, NewNode::Agent, point(500.0, 500.0)).expect("agents can be added");
    assert_eq!(first.id.as_str(), "agent");
    assert_eq!(first.name, "Agent");
    let NodeKind::Agent(agent) = &first.kind else { panic!("an agent") };
    assert_eq!(agent.variants, 1);
    assert_eq!(agent.workdir, "code", "it works in the pipeline's first folder");
    assert_eq!((first.pos.x % 20.0, first.pos.y % 20.0), (0.0, 0.0), "on the grid");
    g.nodes.push(first.clone());

    let second = new_node(&g, NewNode::Agent, point(500.0, 500.0)).expect("again");
    assert_eq!(second.id.as_str(), "agent-2");
    assert_eq!(second.name, "Agent 2");
    assert_ne!(second.pos, first.pos, "not on top of the first");

    let router = new_node(&g, NewNode::Router, point(0.0, 0.0)).expect("routers can be added");
    assert_eq!(router.id.as_str(), "router-2", "the graph has a router already");
    assert!(new_node(&g, NewNode::Pipeline, point(0.0, 0.0)).is_none(), "pipelines are not there yet");
    assert!(!NewNode::Pipeline.available());
    assert_eq!(NewNode::ALL.len(), 9);
}

#[test]
fn problems_mark_the_nodes_they_name() {
    let problem = |severity, node: Option<&str>, message: &str| Problem {
        severity,
        node: node.map(Into::into),
        message: message.into(),
    };
    let marks = problem_marks(&[
        problem(Severity::Warning, Some("fixer"), "No tools"),
        problem(Severity::Error, Some("fixer"), "No model"),
        problem(Severity::Warning, Some("pr"), "Draft PR"),
        problem(Severity::Error, None, "Max parallel is 0"),
    ]);
    assert_eq!(marks.len(), 2, "problems of the whole pipeline are not on a node");
    let fixer = &marks[&"fixer".into()];
    assert_eq!(fixer.severity, Severity::Error, "the worst one wins");
    assert_eq!(fixer.message, "No tools · No model");
    assert_eq!(marks[&"pr".into()].severity, Severity::Warning);
}

fn instance(node: &str, variant: u32, attempt: u32, status: NodeStatus) -> NodeInstance {
    NodeInstance {
        node: node.into(),
        name: node.into(),
        variant,
        attempt,
        status,
        kind: SpanKind::Agent,
        port: None,
        session: None,
        worktree: None,
        branch: None,
        started: None,
        finished: None,
        cost_usd: 0.0,
        summary: String::new(),
        spawned_by: None,
        question: None,
    }
}

pub(super) fn run(nodes: Vec<NodeInstance>) -> Run {
    Run {
        id: RunId::new("run-1"),
        pipeline: PipelineId::new("demo"),
        version: 0,
        issue: None,
        number: 1,
        title: "Test run".into(),
        input: String::new(),
        state: RunState::Running,
        test: true,
        started: 0,
        finished: None,
        cost_usd: 0.0,
        usage: Usage::default(),
        error: None,
        pr_url: None,
        nodes,
    }
}

#[test]
fn a_run_marks_each_instance_with_its_status_and_letter() {
    let marks = instance_marks(&run(vec![
        instance("fixer", 2, 1, NodeStatus::Running),
        instance("fixer", 1, 1, NodeStatus::Failed),
        instance("fixer", 1, 2, NodeStatus::Done),
        instance("src", 0, 1, NodeStatus::Done),
    ]));
    let fixer = &marks[&"fixer".into()];
    assert_eq!(fixer.iter().map(|m| (m.letter, m.status)).collect::<Vec<_>>(), [
        (Some('A'), NodeStatus::Done),
        (Some('B'), NodeStatus::Running),
    ]);
    assert_eq!(marks[&"src".into()][0].letter, None);
}
