//! Firing an autopilot: its target starts on its prompt, a pipeline as a run, an agent or an
//! orchestrator as a chat in the project's repo. The scheduler that fires due autopilots on
//! their clock is not here yet; [`crate::api::IssueApi::run_autopilot`] fires one by hand
//! through the same step.

use anyhow::{Result, anyhow, bail};
use model::{Autopilot, AutopilotTrigger, Event, PipelineId, WorkMode};

use crate::api::{ChatApi as _, NewSession, RunApi as _, RunInput, UserMessage, WorkdirChoice};
use crate::engine::Engine;

/// Prefix of the branches autopilot chats get.
const AUTOPILOT_BRANCH_PREFIX: &str = "autopilot/";

impl Engine {
    /// Starts the autopilot's target now and records the tick: `last_run`, `last_result`,
    /// and the next `next_run` of a schedule that is on. A failed start is recorded too, and
    /// returned.
    pub(crate) fn fire_autopilot(&self, mut autopilot: Autopilot) -> Result<()> {
        let inner = &self.inner;
        let started = self.start_autopilot_target(&autopilot);
        let now = inner.now();
        autopilot.last_run = Some(now);
        autopilot.last_result = match &started {
            Ok(result) => result.clone(),
            Err(error) => format!("failed: {}", base::fmt::first_line(&format!("{error:#}"))),
        };
        if let AutopilotTrigger::Schedule { schedule } = &autopilot.trigger
            && autopilot.enabled
        {
            autopilot.next_run = Some(issues::autopilot::next_run(schedule, now, base::time::utc_offset(now)));
        }
        inner.store.upsert_autopilot(&autopilot)?;
        inner.publish(Event::AutopilotsChanged);
        started.map(|_| ())
    }

    /// Starts the target on the prompt; what happened, in a few words ("started a run").
    fn start_autopilot_target(&self, autopilot: &Autopilot) -> Result<String> {
        let target = &autopilot.target;
        let found = self.inner.catalog().get(target).cloned().ok_or_else(|| anyhow!("{target} is no longer in the library"))?;
        if found.paused || found.archived {
            bail!("{} is {} and takes no new work", found.name, if found.paused { "paused" } else { "archived" });
        }
        let prompt = autopilot.prompt.trim();
        let text = if prompt.is_empty() { autopilot.name.trim().to_string() } else { prompt.to_string() };
        match target.scheme() {
            "pipeline" => {
                let input = RunInput { title: autopilot.name.clone(), text };
                self.start_run(&PipelineId::new(target.id()), input)?;
                Ok("started a run".to_string())
            }
            "agent" | "orchestrator" => {
                let workdir = self.project_repo(autopilot)?.map(|path| WorkdirChoice {
                    path,
                    mode: WorkMode::Worktree,
                    base: None,
                    branch: Some(vcs::branch_name(AUTOPILOT_BRANCH_PREFIX, &autopilot.name)),
                });
                let session = self.new_session(NewSession {
                    runnable: Some(target.clone()),
                    mode: Default::default(),
                    model: None,
                    workdir,
                    forced_skills: Vec::new(),
                    issue: None,
                })?;
                self.send(&session.id, UserMessage { text, images: Vec::new() })?;
                Ok("started a chat".to_string())
            }
            other => bail!("a {other} cannot be started by an autopilot"),
        }
    }

    /// The first repo of the autopilot's project that is a folder on disk.
    fn project_repo(&self, autopilot: &Autopilot) -> Result<Option<std::path::PathBuf>> {
        let Some(project) = self.inner.store.project(&autopilot.project)? else { return Ok(None) };
        Ok(project.repos.into_iter().find(|path| path.is_dir()))
    }
}
