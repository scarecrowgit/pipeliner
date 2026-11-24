//! A pipeline node's session continued after its step: the node's own definition (the
//! library agent with the node's overrides), its worktree and environment, and the node's
//! permission policy. Pipeline agents run without a person to ask, so every tool call their
//! access level grants is allowed; a question to a person ends the turn instead of waiting.

use std::path::PathBuf;

use agent::{AgentEnv, AgentSpec, Finish};
use anyhow::{Result, anyhow, bail};
use model::{NodeId, NodeKind, RunId, Session, WorkMode};

use crate::engine::Inner;

/// The spec and the environment a turn of a node's session runs with, as the node ran during
/// its run, except that the turn ends with a reply instead of `step_done`.
pub(crate) fn node_turn(inner: &Inner, session: &Session, run: &RunId, node: &NodeId, catalog: &catalog::Catalog) -> Result<(AgentSpec, AgentEnv)> {
    let graph = crate::pipelines::run_graph(inner, run)
        .ok_or_else(|| anyhow!("the graph run {run} executed was not kept, so its {node} step cannot be continued"))?;
    let def = graph.nodes.iter().find(|n| &n.id == node).ok_or_else(|| anyhow!("run {run} has no step {node} any more"))?;
    let agent_node = match &def.kind {
        NodeKind::Agent(a) | NodeKind::Orchestrator(a) => a,
        other => bail!("the {} step is a {}, not an agent", def.name, other.label().to_lowercase()),
    };
    let library = agent_node.runnable.as_ref().and_then(|r| catalog.get(r)).cloned();
    let (name, reference, agent_def) =
        pipeline::runners::node_agent(&def.name, agent_node, library.as_ref(), graph.id.as_str(), inner.default_model())?;
    let mut spec = AgentSpec::from_def(name, reference, &agent_def);
    spec.finish = Finish::Reply;
    spec.limits = inner.limits.clone();
    if let Ok(settings) = inner.store.settings() {
        spec.limits.compact_at = f64::from(settings.compact_at);
    }
    let workdir = session.workdir.as_ref();
    let folder: Option<PathBuf> = workdir.map(|w| w.path.clone());
    let env = graph.workdirs.iter().find(|w| w.id == agent_node.workdir).map(|w| w.env.clone()).unwrap_or_default();
    let env = AgentEnv {
        session: session.id.clone(),
        depth: 0,
        workdir: folder.clone(),
        repo: workdir.map(|w| w.source.clone().unwrap_or_else(|| w.path.clone())),
        env,
        read_only: workdir.is_some_and(|w| w.mode == WorkMode::ReadOnly),
        mode: Default::default(),
        run: Some(run.clone()),
        node: Some(node.clone()),
        created_by: None,
    };
    Ok((spec, env))
}
