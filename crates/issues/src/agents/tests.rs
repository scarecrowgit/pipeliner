use std::path::PathBuf;

use model::{
    AgentNode, GraphDef, IssueStatus, NodeDef, NodeId, NodeInstance, NodeKind, NodeStatus, RunAgent, RunState, RunnableKind,
    RunnableRef, SessionId, SpanKind,
};

use super::{RunAgents, run_agents, run_agents_shipped, wake_context, wake_message};
use crate::mentions::{find_mentions_with, targets};
use crate::testing::{issue, run, runnable};
use crate::triggers::{Happening, WakeMode, wake_intents_with};

fn record(node: &str, name: &str, status: NodeStatus, kind: SpanKind, session: Option<&str>) -> NodeInstance {
    NodeInstance {
        node: NodeId::new(node),
        name: name.into(),
        variant: 0,
        attempt: 1,
        status,
        kind,
        port: None,
        session: session.map(SessionId::new),
        worktree: session.map(|_| PathBuf::from(format!("/wt/{node}"))),
        branch: session.map(|_| format!("design/{node}")),
        started: Some(1),
        finished: Some(2),
        cost_usd: 0.0,
        summary: String::new(),
        spawned_by: None,
        question: None,
    }
}

fn graph() -> GraphDef {
    let agent = |id: &str, name: &str, kind: NodeKind| NodeDef { id: NodeId::new(id), name: name.into(), pos: Default::default(), kind };
    GraphDef {
        schema: 1,
        id: "mockup-gpui-build".into(),
        name: "Mockup → GPUI build".into(),
        description: String::new(),
        limits: Default::default(),
        workdirs: Vec::new(),
        nodes: vec![
            agent("mockup", "Mockup", NodeKind::Agent(AgentNode { runnable: Some(RunnableRef::agent("designer")), ..Default::default() })),
            agent("build", "Build", NodeKind::Orchestrator(AgentNode::default())),
        ],
        edges: Vec::new(),
    }
}

/// HAR-8's run 2: the triage router asked a question, Mockup finished, the merge failed.
fn har8() -> model::Run {
    let mut r = run("run-2", RunState::Failed);
    r.number = 2;
    r.error = Some("Merge mockup failed: the checkout is in the middle of a merge".into());
    let mut merge = record("merge-mockup", "Merge mockup", NodeStatus::Failed, SpanKind::Step, None);
    merge.summary = "the checkout is in the middle".into();
    r.nodes = vec![
        record("triage", "Triage", NodeStatus::Done, SpanKind::Step, Some("s-triage")),
        record("mockup", "Mockup", NodeStatus::Done, SpanKind::Agent, Some("s-mockup")),
        record("approval", "Your approval", NodeStatus::Done, SpanKind::Waiting, None),
        merge,
    ];
    r
}

fn mockup(agents: &[RunAgent]) -> &RunAgent {
    agents.iter().find(|a| a.name == "Mockup").expect("Mockup is an agent of the run")
}

#[test]
fn the_agents_of_a_run_are_its_nodes_with_a_session() {
    let designer = runnable(RunnableKind::Agent, "Designer");
    let agents = run_agents(&har8(), Some(&graph()), &[designer]);
    assert_eq!(agents.len(), 1, "the router's question session and the steps are not agents: {agents:#?}");
    let a = &agents[0];
    assert_eq!(a.reference, RunnableRef::session(&SessionId::new("s-mockup")));
    assert_eq!(a.runnable_name.as_deref(), Some("Designer"));
    assert_eq!((a.run_number, a.kind, a.busy), (2, RunnableKind::Agent, false));
    assert_eq!(a.branch.as_deref(), Some("design/mockup"));
}

#[test]
fn a_running_node_is_busy_and_only_the_latest_attempt_counts() {
    let mut r = har8();
    let mut again = record("mockup", "Mockup", NodeStatus::Running, SpanKind::Agent, Some("s-mockup-2"));
    again.attempt = 2;
    r.nodes.push(again);
    let orchestrator = record("build", "Build", NodeStatus::Done, SpanKind::Agent, Some("s-build"));
    r.nodes.push(orchestrator);
    let agents = run_agents(&r, Some(&graph()), &[]);
    assert_eq!(agents.len(), 2);
    assert_eq!(mockup(&agents).session.as_str(), "s-mockup-2");
    assert!(mockup(&agents).busy);
    assert_eq!(agents[1].kind, RunnableKind::Orchestrator);
}

#[test]
fn spawned_sub_agents_are_not_mentionable() {
    let mut r = har8();
    let mut sub = record("coder", "Coder", NodeStatus::Done, SpanKind::Spawned, Some("s-coder"));
    sub.spawned_by = Some(NodeId::new("build"));
    r.nodes.push(sub);
    assert_eq!(run_agents(&r, None, &[]).len(), 1);
}

#[test]
fn a_mention_of_a_run_agent_wins_over_a_library_runnable_of_the_same_name() {
    let library = runnable(RunnableKind::Agent, "Mockup");
    let designer = runnable(RunnableKind::Agent, "Designer");
    let agents = run_agents(&har8(), Some(&graph()), std::slice::from_ref(&designer));
    let found = find_mentions_with("@Mockup fix the merge, then ask @designer", &agents, &[library, designer]);
    let session = RunnableRef::session(&SessionId::new("s-mockup"));
    assert_eq!(targets(&found), vec![session.clone()], "both names answer to the run's agent");
    assert_eq!(found[1].name, "Mockup");
}

#[test]
fn a_mentioned_run_agent_continues_its_session_and_the_runner_waits() {
    let agents = run_agents(&har8(), Some(&graph()), &[]);
    let mut i = issue(IssueStatus::InProgress);
    i.runner = Some(RunnableRef::pipeline("mockup-gpui-build"));
    i.triggers.on_comment = true;
    let pipeline = runnable(RunnableKind::Pipeline, "mockup-gpui-build");
    let session = mockup(&agents).reference.clone();
    let comment = Happening::Comment { text: "@Mockup fix it".into(), mentions: vec![session.clone()] };
    let all = |agents: &[RunAgent]| RunAgents { agents: agents.to_vec(), shared: Vec::new() };
    let wakes = wake_intents_with(&i, &comment, &[pipeline], &all(&agents));
    assert_eq!(wakes.intents.len(), 1, "the comment is for Mockup, not for the pipeline: {wakes:#?}");
    let intent = &wakes.intents[0];
    assert_eq!((intent.mode, intent.name.as_str()), (WakeMode::Session, "Mockup"));
    assert_eq!(intent.cause, "@mention · continues its work from run 2 in its worktree");

    let mut busy = agents.clone();
    busy[0].busy = true;
    let wakes = wake_intents_with(&i, &comment, &[], &all(&busy));
    assert!(wakes.intents.is_empty());
    assert!(wakes.refusals[0].reason.contains("running right now"), "{wakes:#?}");

    let wakes = wake_intents_with(&i, &comment, &[], &RunAgents::default());
    assert!(wakes.refusals[0].reason.contains("no longer an agent"), "{wakes:#?}");
}

#[test]
fn the_woken_agent_is_told_which_step_failed_and_why() {
    let r = har8();
    let agents = run_agents(&r, Some(&graph()), &[]);
    let mut i = issue(IssueStatus::InProgress);
    i.key = "HAR-8".into();
    i.title = "ai pipeline editor".into();
    let text = wake_context(&i, &r, mockup(&agents));
    assert!(text.starts_with("You are back on issue HAR-8: ai pipeline editor. You worked on it as the Mockup step of run 2."), "{text}");
    assert!(text.contains("Run 2 failed after your step, at the Merge mockup step: the checkout is in the middle of a merge"), "{text}");
    assert!(text.contains("same worktree (/wt/mockup, branch `design/mockup`)"), "{text}");

    let mut done = r.clone();
    done.state = RunState::Done;
    assert!(wake_context(&i, &done, mockup(&agents)).contains("Run 2 is done."));
    let message = wake_message(&i, &r, mockup(&agents), "  @Mockup the merge failed  ");
    assert!(message.ends_with("## New comment\n\n@Mockup the merge failed"), "{message}");
}

#[test]
fn a_new_attempt_without_a_session_yet_still_answers_with_the_last_one() {
    let mut r = har8();
    r.state = RunState::Running;
    let mut again = record("mockup", "Mockup", NodeStatus::Pending, SpanKind::Agent, None);
    again.attempt = 2;
    r.nodes.push(again);
    let agents = run_agents(&r, Some(&graph()), &[]);
    assert_eq!(mockup(&agents).session.as_str(), "s-mockup", "the previous attempt's session: {agents:#?}");
    assert!(!mockup(&agents).busy);
    r.nodes.last_mut().unwrap().status = NodeStatus::Running;
    assert!(mockup(&run_agents(&r, Some(&graph()), &[])).busy, "the new attempt runs now");
}

fn variants() -> model::Run {
    let mut r = har8();
    r.nodes.retain(|n| n.node.as_str() != "mockup");
    for (variant, session) in [(1, "s-a"), (2, "s-b")] {
        let mut v = record("mockup", "Mockup", NodeStatus::Done, SpanKind::Agent, Some(session));
        v.variant = variant;
        r.nodes.push(v);
    }
    r
}

#[test]
fn variants_are_named_apart_and_their_runnable_name_is_never_a_guess() {
    let designer = runnable(RunnableKind::Agent, "Designer");
    let found = run_agents_shipped(&variants(), Some(&graph()), std::slice::from_ref(&designer), &[]);
    let names: Vec<&str> = found.agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Mockup A", "Mockup B"]);
    assert!(found.agents.iter().all(|a| a.runnable_name.is_none()));
    assert_eq!(found.shared[0].reason(), "Designer worked as Mockup A and Mockup B in run 2; mention one of them by that name");

    let comment_on = |text: &str, found: &RunAgents| {
        let mentions = targets(&find_mentions_with(text, &found.mentionable(), std::slice::from_ref(&designer)));
        let happening = Happening::Comment { text: text.into(), mentions };
        wake_intents_with(&issue(IssueStatus::InProgress), &happening, std::slice::from_ref(&designer), found)
    };
    let wakes = comment_on("@Designer fix it", &found);
    assert!(wakes.intents.is_empty(), "neither the library's Designer nor a variant: {wakes:#?}");
    assert_eq!(wakes.refusals[0].name, "Designer");
    let wakes = comment_on("@Mockup B fix it", &found);
    assert_eq!(wakes.intents[0].target, RunnableRef::session(&SessionId::new("s-b")));

    let shipped = run_agents_shipped(&variants(), Some(&graph()), std::slice::from_ref(&designer), &[SessionId::new("s-b")]);
    assert!(shipped.shared.is_empty());
    let wakes = comment_on("@Designer fix it", &shipped);
    assert_eq!(wakes.intents[0].target, RunnableRef::session(&SessionId::new("s-b")), "the shipped variant answers to it");
}
