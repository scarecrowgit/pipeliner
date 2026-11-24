//! Waking runnables on issues: a pipeline starts a run with the issue as the item (or answers
//! the question its open run waits on); an agent or orchestrator gets a chat on the issue.
//! Every wake-up is recorded in the issue's feed, as `Woken` or `WakeFailed` with the reason.

use crate::api::RunApi as _;
use anyhow::{Result, anyhow, bail};
use issues::{Happening, WakeIntent, WakeMode};
use model::{Actor, Issue, IssueEventKind, PipelineId, RunId, RunState, RunnableRef, WorkMode};
use store::SessionFilter;

use crate::api::{ChatApi, NewSession, UserMessage, WorkdirChoice};
use crate::engine::Engine;

/// Prefix of the branches chats on issues get.
const ISSUE_BRANCH_PREFIX: &str = "issue/";

/// What a freshly started run is told about the issue.
pub(crate) fn issue_input(issue: &Issue) -> String {
    let (title, body) = (issue.title.trim(), issue.body.trim());
    if body.is_empty() { title.to_string() } else { format!("{title}\n\n{body}") }
}

impl Engine {
    /// Wakes whoever `happening` wakes on `issue`, and records who could not be.
    pub(crate) fn wake(&self, issue: &Issue, happening: &Happening) -> Result<()> {
        let catalog = self.inner.catalog();
        let wakes = issues::triggers::wake_intents(issue, happening, catalog.runnables());
        for intent in wakes.intents {
            self.wake_one(issue, &intent)?;
        }
        for refusal in wakes.refusals {
            let kind = IssueEventKind::WakeFailed { target: refusal.target, name: refusal.name, reason: refusal.reason };
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
        let started = self.try_wake(issue, &intent);
        self.record(issue, &intent, &started)?;
        started.map(|_| ())
    }

    fn wake_one(&self, issue: &Issue, intent: &WakeIntent) -> Result<()> {
        let started = self.try_wake(issue, intent);
        self.record(issue, intent, &started)
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

    fn try_wake(&self, issue: &Issue, intent: &WakeIntent) -> Result<(Option<RunId>, u32)> {
        match intent.target.scheme() {
            "pipeline" => self.wake_pipeline(issue, intent),
            "agent" | "orchestrator" => self.wake_agent(issue, intent).map(|_| (None, 0)),
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
            (WakeMode::Continue, None) => format!("{}\n\n## New comment\n\n{}", issue_input(issue), intent.input.trim()),
            (WakeMode::Start, None) => intent.input.clone(),
        };
        // Checked again, atomically, where the run starts.
        let run = self.start_issue_run(issue, &pipeline, &text, false)?;
        Ok((Some(run.id), run.number))
    }

    /// A chat with the agent on the issue: the one it already has, else a new one in a
    /// worktree of the project's repo when it has one.
    fn wake_agent(&self, issue: &Issue, intent: &WakeIntent) -> Result<()> {
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
                let workdir = self.issue_repo(issue)?.map(|path| WorkdirChoice {
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
            WakeMode::Start => format!("Issue {}: {}", issue.key, intent.input),
            WakeMode::Continue => intent.input.clone(),
        };
        self.send(&session.id, UserMessage { text, images: Vec::new() })
    }

    /// The checkout the issue's work belongs in: the project's repo named by the issue, else
    /// its first one.
    fn issue_repo(&self, issue: &Issue) -> Result<Option<std::path::PathBuf>> {
        let Some(project) = self.inner.store.project(&issue.project)? else { return Ok(None) };
        let named = issue.repo.as_ref().and_then(|name| {
            project.repos.iter().find(|path| path.file_name().is_some_and(|n| n.to_string_lossy() == name.as_str()))
        });
        Ok(named.or(project.repos.first()).filter(|p| p.is_dir()).cloned())
    }
}
