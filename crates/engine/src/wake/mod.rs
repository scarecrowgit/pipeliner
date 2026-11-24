//! Waking runnables on issues: a pipeline starts a run with the issue as the item (or answers
//! the question its open run waits on); an agent or orchestrator gets a chat on the issue; an
//! agent of the issue's latest run continues its node's own session ([`session`]). Every
//! wake-up is recorded in the issue's feed, as `Woken` or `WakeFailed` with the reason, and an
//! agent's answer comes back to the thread as its comment ([`reply`]).

pub(crate) mod node;
mod reply;
mod session;

pub(crate) use reply::Replies;

/// What a comment's mentions were found among: the agents of the issue's latest run (worked
/// out once per comment) and the names the comment gave each mentioned target.
#[derive(Default)]
pub(crate) struct Mentioned {
    pub agents: issues::RunAgents,
    pub names: Vec<(RunnableRef, String)>,
}

use std::path::PathBuf;

use crate::api::RunApi as _;
use anyhow::{Result, anyhow, bail};
use issues::{Happening, WakeIntent, WakeMode};
use model::{Actor, Attachment, Issue, IssueEventKind, NodeKind, PipelineId, RunId, RunState, RunnableRef, WorkMode};
use store::SessionFilter;

use crate::api::{ChatApi, NewSession, UserMessage, WorkdirChoice};
use crate::engine::Engine;
use reply::PendingReply;

/// Prefix of the branches chats on issues get.
const ISSUE_BRANCH_PREFIX: &str = "issue/";

/// What a freshly started run is told about the issue.
pub(crate) fn issue_input(issue: &Issue) -> String {
    let (title, body) = (issue.title.trim(), issue.body.trim());
    if body.is_empty() { title.to_string() } else { format!("{title}\n\n{body}") }
}

/// What a wake tells its runner: the issue (`about`), and what was `said` in the comment that
/// caused the wake when that is not the issue's own text. A mention carries only the comment,
/// and a runner that is new to the issue has never seen the issue.
fn wake_input(about: String, said: &str) -> String {
    let said = said.trim();
    if said.is_empty() || said == about { about } else { format!("{about}\n\n## New comment\n\n{said}") }
}

impl Engine {
    /// Wakes whoever `happening` wakes on `issue`, and records who could not be.
    pub(crate) fn wake(&self, issue: &Issue, happening: &Happening) -> Result<()> {
        self.wake_with(issue, happening, &[], &Mentioned::default())
    }

    /// [`Engine::wake`] for a comment with `images` (the agents it wakes get them too) whose
    /// mentions were found among `mentioned`.
    pub(crate) fn wake_with(&self, issue: &Issue, happening: &Happening, images: &[Attachment], mentioned: &Mentioned) -> Result<()> {
        let catalog = self.inner.catalog();
        let wakes = issues::triggers::wake_intents_with(issue, happening, catalog.runnables(), &mentioned.agents);
        for intent in wakes.intents {
            let started = self.try_wake(issue, &intent, images, &mentioned.agents);
            self.record(issue, &intent, &started)?;
        }
        for refusal in wakes.refusals {
            // The rules lost the name of an agent that left the run; the comment still has it.
            let name = match mentioned.names.iter().find(|(r, _)| r == &refusal.target) {
                Some((_, name)) if refusal.name == issues::triggers::AGENT_GONE => name.clone(),
                _ => refusal.name,
            };
            let kind = IssueEventKind::WakeFailed { target: refusal.target, name, reason: refusal.reason };
            self.inner.activity(&issue.key, Actor::System, kind)?;
        }
        Ok(())
    }

    /// Starts `target` on the issue, as a person asked.
    pub(crate) fn wake_by_hand(&self, issue: &Issue, target: &RunnableRef, cause: &str) -> Result<()> {
        let catalog = self.inner.catalog();
        let found = catalog.get(target).ok_or_else(|| anyhow!("{target} is no longer in the library"))?;
        if found.paused || found.archived {
            bail!("{} is {} and takes no new work", found.name, if found.paused { "paused" } else { "archived" });
        }
        let intent = WakeIntent {
            target: target.clone(),
            name: found.name.clone(),
            mode: WakeMode::Start,
            cause: cause.to_string(),
            input: issue_input(issue),
        };
        let started = self.try_wake(issue, &intent, &[], &Default::default());
        self.record(issue, &intent, &started)?;
        started.map(|_| ())
    }

    fn record(&self, issue: &Issue, intent: &WakeIntent, started: &Result<(Option<RunId>, u32)>) -> Result<()> {
        let kind = match started {
            Ok((run, number)) => IssueEventKind::Woken {
                target: intent.target.clone(),
                name: intent.name.clone(),
                run: run.clone(),
                number: *number,
                cause: intent.cause.clone(),
            },
            Err(error) => IssueEventKind::WakeFailed { target: intent.target.clone(), name: intent.name.clone(), reason: format!("{error:#}") },
        };
        self.inner.activity(&issue.key, Actor::System, kind)
    }

    fn try_wake(&self, issue: &Issue, intent: &WakeIntent, images: &[Attachment], agents: &issues::RunAgents) -> Result<(Option<RunId>, u32)> {
        if intent.mode == WakeMode::Session {
            return self.wake_session(issue, &intent.target, &intent.input, images.to_vec(), agents);
        }
        match intent.target.scheme() {
            "pipeline" => self.wake_pipeline(issue, intent),
            "agent" | "orchestrator" => self.wake_agent(issue, intent, images).map(|_| (None, 0)),
            other => bail!("a {other} cannot be started on an issue"),
        }
    }

    fn wake_pipeline(&self, issue: &Issue, intent: &WakeIntent) -> Result<(Option<RunId>, u32)> {
        let pipeline = PipelineId::new(intent.target.id());
        // Any open run on the issue counts, whichever pipeline it runs.
        let open = self.inner.store.runs_for_issue(&issue.key)?.into_iter().find(|run| run.state.is_open());
        let text = match (intent.mode, open) {
            (WakeMode::Continue, Some(run)) if run.state == RunState::WaitingAnswer && run.pipeline == pipeline => {
                self.answer_run(&run.id, None, &intent.input)?;
                return Ok((Some(run.id), run.number));
            }
            (WakeMode::Continue, Some(run)) => {
                bail!("run {} is still open ({:?}); the comment stays on the issue", run.number, run.state)
            }
            (WakeMode::Start, Some(run)) => bail!("run {} on this issue is still open; cancel it first", run.number),
            (WakeMode::Session, _) => bail!("a pipeline is not an agent of a run"),
            (WakeMode::Continue | WakeMode::Start, None) => wake_input(issue_input(issue), &intent.input),
        };
        // Checked again, atomically, where the run starts.
        let run = self.start_issue_run(issue, &pipeline, &text, false)?;
        Ok((Some(run.id), run.number))
    }

    /// A chat with the agent on the issue: the one it already has, else a new one in a
    /// worktree of the issue's repo when one is known. Its answer is posted on the issue.
    fn wake_agent(&self, issue: &Issue, intent: &WakeIntent, images: &[Attachment]) -> Result<()> {
        let inner = &self.inner;
        let existing = inner
            .store
            .sessions(&SessionFilter::default())?
            .into_iter()
            .filter(|s| s.parent.is_none() && s.issue.as_ref() == Some(&issue.key) && s.runnable == intent.target)
            .max_by_key(|s| s.updated);
        let session = match (intent.mode, existing) {
            (WakeMode::Continue, Some(session)) => session,
            _ => {
                let workdir = self.issue_repo(issue, &intent.target)?.map(|path| WorkdirChoice {
                    path,
                    mode: WorkMode::Worktree,
                    base: None,
                    branch: Some(vcs::branch_name(ISSUE_BRANCH_PREFIX, issue.key.as_str())),
                });
                self.new_session(NewSession {
                    runnable: Some(intent.target.clone()),
                    mode: Default::default(),
                    model: None,
                    workdir,
                    forced_skills: Vec::new(),
                    issue: Some(issue.key.clone()),
                })?
            }
        };
        let text = match intent.mode {
            WakeMode::Start => format!("Issue {}: {}", issue.key, wake_input(issue_input(issue), &intent.input)),
            WakeMode::Continue | WakeMode::Session => intent.input.clone(),
        };
        let actor = Actor::Runnable { reference: intent.target.clone(), name: intent.name.clone(), spawned: false, session: Some(session.id.clone()) };
        self.send_owing_reply(&session.id, UserMessage { text, images: images.to_vec() }, PendingReply { issue: issue.key.clone(), actor })
    }

    /// Sends `message` to `session` tagged with an owed reply: the turn that takes it in
    /// answers on the issue. The reply is owed before the message can be taken, and forgotten
    /// when it never arrives.
    pub(crate) fn send_owing_reply(&self, session: &model::SessionId, message: UserMessage, reply: PendingReply) -> Result<()> {
        let tag = reply::new_tag();
        self.inner.owe_reply(&tag, session, reply);
        if let Err(error) = self.send_tagged(session, message, Some(tag.clone())) {
            self.inner.forget_reply(&tag);
            return Err(error);
        }
        Ok(())
    }

    /// The checkout the issue's work belongs in, for `target`: the project's repo named by the
    /// issue, else its first one. A project without repos (the pipeline names its own) falls
    /// back to the workdirs of the pipeline the issue runs with: the one the issue names, else
    /// the one a node run by `target` works in, else the first.
    fn issue_repo(&self, issue: &Issue, target: &RunnableRef) -> Result<Option<PathBuf>> {
        let named = |path: &PathBuf| issue.repo.as_ref().is_some_and(|name| path.file_name().is_some_and(|n| n.to_string_lossy() == name.as_str()));
        if let Some(project) = self.inner.store.project(&issue.project)? {
            let found = project.repos.iter().find(|p| named(p)).or(project.repos.first()).filter(|p| p.is_dir());
            if let Some(path) = found {
                return Ok(Some(path.clone()));
            }
        }
        Ok(self.pipeline_repo(issue, target).filter(|p| p.is_dir()))
    }

    /// The workdir of the issue's pipeline that `target` would work in (see [`Engine::issue_repo`]).
    fn pipeline_repo(&self, issue: &Issue, target: &RunnableRef) -> Option<PathBuf> {
        let inner = &self.inner;
        let latest = inner.store.runs_for_issue(&issue.key).ok()?.into_iter().max_by_key(|r| (r.number, r.started));
        let graph = match (&latest, issue.runner.as_ref().filter(|r| r.scheme() == "pipeline")) {
            (Some(run), _) => crate::pipelines::run_graph(inner, &run.id),
            (None, Some(runner)) => crate::pipelines::live_graph(inner, &PipelineId::new(runner.id())).ok().map(|(g, _)| g),
            (None, None) => None,
        }?;
        let by_name = issue.repo.as_ref().and_then(|name| {
            graph.workdirs.iter().find(|w| {
                w.id == *name || w.name.eq_ignore_ascii_case(name) || w.path.file_name().is_some_and(|n| n.to_string_lossy() == name.as_str())
            })
        });
        let by_node = graph.nodes.iter().find_map(|n| match &n.kind {
            NodeKind::Agent(a) | NodeKind::Orchestrator(a) if a.runnable.as_ref() == Some(target) => graph.workdirs.iter().find(|w| w.id == a.workdir),
            _ => None,
        });
        by_name.or(by_node).or(graph.workdirs.first()).map(|w| inner.paths.expand(&w.path.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::wake_input;

    #[test]
    fn a_wake_always_carries_the_issue() {
        let about = || "Fix login\n\nThe form loses focus.".to_string();
        assert_eq!(wake_input(about(), &about()), about(), "started from the issue itself: nothing is repeated");
        assert_eq!(wake_input(about(), "  "), about());
        assert_eq!(
            wake_input(about(), " @Build take this "),
            "Fix login\n\nThe form loses focus.\n\n## New comment\n\n@Build take this",
            "a mention's comment comes after the issue it is about"
        );
    }
}
