//! Waking an agent of the issue's latest run: its node's own session goes on, in its worktree,
//! with the comment as the next message. Not a new run and not a new chat; the run's state
//! does not move (a person resumes the run as before), and the turn's spend is the session's,
//! not the run's.

use anyhow::{Result, anyhow, bail};
use issues::RunAgents;
use model::{Actor, Attachment, Issue, IssueEventKind, IssueKey, NodeStatus, RunId, RunnableRef, SessionOrigin, WorkMode, Workdir};

use super::reply::PendingReply;
use crate::api::UserMessage;
use crate::bus::lock;
use crate::engine::Engine;

impl Engine {
    /// The agents and orchestrators of the issue's latest run that a comment can wake, and the
    /// library names its variants share.
    pub(crate) fn run_agents_of(&self, issue: &IssueKey) -> Result<RunAgents> {
        let inner = &self.inner;
        let runs = inner.store.runs_for_issue(issue)?;
        let Some(latest) = runs.iter().max_by_key(|r| (r.number, r.started)) else { return Ok(RunAgents::default()) };
        let graph = crate::pipelines::run_graph(inner, &latest.id);
        let shipped: Vec<model::SessionId> = inner
            .store
            .reviews_for_run(&latest.id)?
            .iter()
            .filter_map(|review| {
                let picked = review.picked?;
                review.variants.iter().find(|v| v.index == picked).and_then(|v| v.session.clone())
            })
            .collect();
        Ok(issues::run_agents_shipped(latest, graph.as_ref(), inner.catalog().runnables(), &shipped))
    }

    /// Continues the session of the run's agent `target` with `comment`; its answer is posted
    /// on the issue when the turn that takes the comment in ends.
    pub(crate) fn wake_session(
        &self,
        issue: &Issue,
        target: &RunnableRef,
        comment: &str,
        images: Vec<Attachment>,
        agents: &RunAgents,
    ) -> Result<(Option<RunId>, u32)> {
        let inner = &self.inner;
        let id = target.session_id().ok_or_else(|| anyhow!("{target} is not an agent of a run"))?;
        let agent =
            agents.agents.iter().find(|a| a.session == id).ok_or_else(|| anyhow!("it is no longer an agent of this issue's latest run"))?;
        let run = inner.store.run(&agent.run)?.ok_or_else(|| anyhow!("run {} of this issue is gone", agent.run_number))?;
        let node = run.instance(&agent.node, agent.variant);
        if agent.busy || node.is_some_and(|n| n.status == NodeStatus::Running) {
            bail!("its step of run {} is running right now; mention it again once the step is done", agent.run_number);
        }
        // The answer counts toward the day's spend: past the limit nothing new starts.
        crate::pipelines::check_limit(inner)?;
        let session = inner.session(&id)?;
        // Only under the node's own definition and policy: a woken agent nobody watches must
        // never wait on a permission question.
        let SessionOrigin::Run { run: run_id, node: node_id, .. } = &session.origin else {
            bail!("its session is not a pipeline step's any more");
        };
        let folder = session.workdir.as_ref().map(|w| w.path.clone());
        crate::wake::node::node_turn(inner, &session, run_id, node_id, &inner.catalog_for(folder.as_deref()))?;
        if let Some(workdir) = session.workdir.as_ref().filter(|w| w.mode == WorkMode::Worktree) {
            // Two mentions at once must not both put the worktree back.
            let _restoring = lock(&inner.worktree_restores);
            self.check_worktree(workdir)?;
        }
        let text = issues::wake_message(issue, &run, agent, comment);
        let actor = Actor::Runnable { reference: target.clone(), name: agent.name.clone(), spawned: false, session: Some(id.clone()) };
        self.send_owing_reply(&id, UserMessage { text, images }, PendingReply { issue: issue.key.clone(), actor })?;
        Ok((None, 0))
    }

    /// Makes sure the worktree of a node's session is there, on its branch: put back from the
    /// branch when its folder was deleted, refused when the folder is something else now.
    fn check_worktree(&self, workdir: &Workdir) -> Result<()> {
        let shown = self.inner.paths.display(&workdir.path);
        let (Some(source), Some(branch)) = (workdir.source.as_ref(), workdir.branch.as_ref()) else {
            if workdir.path.is_dir() {
                return Ok(());
            }
            bail!("its worktree {shown} is gone and nothing says where it came from");
        };
        if !workdir.path.exists() {
            return crate::workdir::restore_worktree(source, &workdir.path, branch);
        }
        crate::workdir::check_worktree(source, &workdir.path, branch)
    }

    /// Tries a run's agent again with the last comment that mentioned it.
    pub(crate) fn rewake_session(&self, issue: &Issue, target: &RunnableRef) -> Result<()> {
        let events = self.inner.store.issue_events(&issue.key)?;
        let last = events.iter().rev().find_map(|e| match &e.kind {
            IssueEventKind::Comment { text, images, mentions } if e.actor == Actor::Me && mentions.contains(target) => {
                Some((text.clone(), images.clone()))
            }
            _ => None,
        });
        let Some((text, images)) = last else { bail!("no comment on {} mentions this agent any more", issue.key) };
        let agents = self.run_agents_of(&issue.key)?;
        let found = agents.agents.iter().find(|a| &a.reference == target);
        let name = found.map_or_else(|| "The agent".to_string(), |a| a.name.clone());
        let cause = found.map_or_else(
            || "tried again".to_string(),
            |a| format!("tried again · {}", issues::triggers::session_cause(a).trim_start_matches("@mention · ")),
        );
        let intent = issues::WakeIntent { target: target.clone(), name, mode: issues::WakeMode::Session, cause, input: text };
        let woken = self.wake_session(issue, target, &intent.input, images, &agents);
        self.record(issue, &intent, &woken)?;
        woken.map(|_| ())
    }
}
