//! Waking an agent of the issue's latest run: its node's own session goes on, in its worktree,
//! with the comment as the next message. Not a new run and not a new chat; the run's state
//! does not move (a person resumes the run as before).

use anyhow::{Result, anyhow, bail};
use model::{Actor, Attachment, Issue, IssueEventKind, NodeStatus, RunAgent, RunId, RunnableRef, WorkMode};

use super::reply::PendingReply;
use crate::api::{ChatApi, UserMessage};
use crate::engine::Engine;

impl Engine {
    /// The agents and orchestrators of the issue's latest run that a comment can wake.
    pub(crate) fn run_agents_of(&self, issue: &model::IssueKey) -> Result<Vec<RunAgent>> {
        let inner = &self.inner;
        let runs = inner.store.runs_for_issue(issue)?;
        let Some(latest) = runs.iter().max_by_key(|r| (r.number, r.started)) else { return Ok(Vec::new()) };
        let graph = crate::pipelines::run_graph(inner, &latest.id);
        Ok(issues::run_agents(latest, graph.as_ref(), inner.catalog().runnables()))
    }

    /// Continues the session of the run's agent `target` with `comment`; its answer is posted
    /// on the issue when the turn ends.
    pub(crate) fn wake_session(&self, issue: &Issue, target: &RunnableRef, comment: &str, images: Vec<Attachment>) -> Result<(Option<RunId>, u32)> {
        let inner = &self.inner;
        let id = target.session_id().ok_or_else(|| anyhow!("{target} is not an agent of a run"))?;
        let agents = self.run_agents_of(&issue.key)?;
        let agent = agents.iter().find(|a| a.session == id).ok_or_else(|| anyhow!("it is no longer an agent of this issue's latest run"))?;
        let run = inner.store.run(&agent.run)?.ok_or_else(|| anyhow!("run {} of this issue is gone", agent.run_number))?;
        let node = run.instance(&agent.node, agent.variant);
        if agent.busy || node.is_some_and(|n| n.status == NodeStatus::Running) {
            bail!("its step of run {} is running right now; mention it again once the step is done", agent.run_number);
        }
        let session = inner.session(&id)?;
        if let Some(workdir) = session.workdir.as_ref().filter(|w| w.mode == WorkMode::Worktree && !w.path.is_dir()) {
            let (Some(source), Some(branch)) = (workdir.source.as_ref(), workdir.branch.as_ref()) else {
                bail!("its worktree {} is gone and nothing says where it came from", inner.paths.display(&workdir.path));
            };
            crate::workdir::restore_worktree(source, &workdir.path, branch)?;
        }
        let text = issues::wake_message(issue, &run, agent, comment);
        let actor = Actor::Runnable { reference: target.clone(), name: agent.name.clone(), spawned: false, session: Some(id.clone()) };
        // Owed before the turn can start, so even a turn that ends at once answers.
        inner.replies.expect(&id, PendingReply { issue: issue.key.clone(), actor, said: text.clone() });
        if let Err(error) = self.send(&id, UserMessage { text: text.clone(), images }) {
            inner.replies.forget(&id, &text);
            return Err(error);
        }
        Ok((None, 0))
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
        let name = agents.iter().find(|a| &a.reference == target).map_or_else(|| "The agent".to_string(), |a| a.name.clone());
        let cause = agents
            .iter()
            .find(|a| &a.reference == target)
            .map_or_else(|| "tried again".to_string(), |a| format!("tried again · {}", issues::triggers::session_cause(a).trim_start_matches("@mention · ")));
        let intent = issues::WakeIntent { target: target.clone(), name, mode: issues::WakeMode::Session, cause, input: text };
        let woken = self.wake_session(issue, target, &intent.input, images);
        self.record(issue, &intent, &woken)?;
        woken.map(|_| ())
    }
}
