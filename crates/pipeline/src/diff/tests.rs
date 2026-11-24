use std::path::Path;

use model::graph::{PORT_ELSE, PORT_OUT};
use model::{EdgeDef, EdgeKind, NodeDef, NodeId, Pos, RouterRule};

use super::*;

fn built_in() -> GraphDef {
    catalog::self_edit_pipeline(Path::new("/work/pipeliner"), Path::new("/work/mockups"), &Default::default())
}

fn kind_mut<'g>(graph: &'g mut GraphDef, id: &str) -> &'g mut NodeKind {
    let id = NodeId::new(id);
    graph.nodes.iter_mut().find(|n| n.id == id).map(|n| &mut n.kind).unwrap_or_else(|| panic!("no node"))
}

fn agent_mut<'g>(graph: &'g mut GraphDef, id: &str) -> &'g mut AgentNode {
    match kind_mut(graph, id) {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => agent,
        _ => panic!("not an agent"),
    }
}

fn changed(node: &str, name: &str, what: &str) -> GraphChange {
    GraphChange::Changed { node: node.into(), name: name.into(), what: what.into() }
}

fn wiring(what: &str) -> GraphChange {
    GraphChange::Wiring { what: what.into() }
}

#[test]
fn equal_graphs_and_moved_nodes_have_no_changes() {
    let old = built_in();
    assert_eq!(diff(&old, &old), []);
    let mut new = old.clone();
    for node in &mut new.nodes {
        node.pos = Pos { x: node.pos.x + 40.0, y: node.pos.y - 10.0 };
    }
    assert_eq!(diff(&old, &new), []);
}

#[test]
fn agent_fields_read_as_arrows() {
    let old = built_in();
    let mut new = old.clone();
    let designer = agent_mut(&mut new, "designer");
    designer.variants = 3;
    designer.model = Some(ModelRef::parse("nr/cc/claude-sonnet-5-5"));
    designer.role.push_str(" Be brief.");
    designer.budget_usd = Some(2.5);
    assert_eq!(
        diff(&old, &new),
        [changed(
            "designer",
            "Mockup designer",
            "role edited, model claude-opus-5-5 → claude-sonnet-5-5, variants 1 → 3, budget none → $2.50"
        )]
    );
}

#[test]
fn rules_are_added_edited_and_removed() {
    let old = built_in();
    let mut new = old.clone();
    let NodeKind::Router(router) = kind_mut(&mut new, "triage") else { panic!() };
    router.rules.push(RouterRule {
        id: "bug".into(),
        label: "Bug ≥High".into(),
        when: r#"type == "bug" && priority >= "high""#.into(),
        ask: None,
    });
    router.rules[0].label = "Needs mockups".into();
    assert_eq!(diff(&old, &new), [changed("triage", "Triage", "rule edited: Needs mockups, rule added: Bug ≥High")]);

    let mut newer = old.clone();
    let NodeKind::Router(router) = kind_mut(&mut newer, "triage") else { panic!() };
    router.rules.clear();
    assert_eq!(diff(&old, &newer), [changed("triage", "Triage", "rule removed: Needs a mockup")]);
}

#[test]
fn added_and_removed_nodes_carry_their_edges() {
    let old = built_in();
    let mut new = old.clone();
    new.nodes.push(NodeDef {
        id: "split".into(),
        name: "Severity split".into(),
        pos: Pos::default(),
        kind: NodeKind::Router(RouterNode { rules: vec![RouterRule::default(), RouterRule::default()] }),
    });
    new.edges.push(EdgeDef { from: "split".into(), port: PORT_ELSE.into(), to: "build".into(), kind: EdgeKind::Route });
    new.nodes.retain(|n| n.id.as_str() != "render");
    new.edges.retain(|e| e.from.as_str() != "render" && e.to.as_str() != "render");
    assert_eq!(
        diff(&old, &new),
        [
            GraphChange::Added { node: "split".into(), name: "Severity split".into(), what: "new router: 2 rules".into() },
            GraphChange::Removed { node: "render".into(), name: "Render mockups".into() },
        ]
    );
}

#[test]
fn wiring_workdirs_and_limits() {
    let old = built_in();
    let mut new = old.clone();
    new.edges.retain(|e| !(e.from.as_str() == "triage" && e.port == PORT_ELSE));
    new.edges.push(EdgeDef { from: "triage".into(), port: PORT_ELSE.into(), to: "pull-request".into(), kind: EdgeKind::Route });
    new.edges.push(EdgeDef { from: "designer".into(), port: PORT_OUT.into(), to: "coder".into(), kind: EdgeKind::Spawn });
    new.workdirs[1].base_branch = "dev".into();
    new.workdirs[1].setup.clear();
    new.limits.max_parallel = 4;
    new.limits.budget_usd = Some(3.0);
    assert_eq!(
        diff(&old, &new),
        [
            wiring("edge added: Triage (else) → Pull request"),
            wiring("edge added: spawn Mockup designer → Coder"),
            wiring("edge removed: Triage (else) → Build"),
            wiring("workdir Code: base branch main → dev, setup edited"),
            wiring("max parallel 2 → 4"),
            wiring("run budget none → $3.00"),
        ]
    );
}

#[test]
fn type_and_action_changes() {
    let old = built_in();
    let mut new = old.clone();
    *kind_mut(&mut new, "merge-mockup") = NodeKind::Action(ActionNode::Report);
    *kind_mut(&mut new, "approval") = NodeKind::Action(ActionNode::Report);
    let NodeKind::Action(ActionNode::PullRequest { draft, .. }) = kind_mut(&mut new, "pull-request") else { panic!() };
    *draft = false;
    new.nodes[0].name = "Board".into();
    assert_eq!(
        diff(&old, &new),
        [
            changed("source", "Board", "renamed Issues → Board"),
            changed("approval", "Your approval", "type review → action"),
            changed("merge-mockup", "Merge mockup", "action merge → report"),
            changed("pull-request", "Pull request", "draft on → off"),
        ]
    );
}
