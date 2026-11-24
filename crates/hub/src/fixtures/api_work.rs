//! The fixture's issues, runs and inbox.

use anyhow::{Result, anyhow, bail};

use crate::api::{
    Actor, Attachment, Autopilot, AutopilotId, Estimate, Event, FileDiff, InboxApi, InboxId, InboxItem, Issue, IssueApi,
    IssueEvent, IssueEventKind, IssueKey, IssuePatch, NewIssue, PipelineId, ProjectId, PullRequest, Review,
    ReviewDecision, ReviewId, ReviewState, Run, RunApi, RunId, RunInput, RunLogLine, RunState, RunStats, RunWait,
    RunnableRef, StartFrom,
};

/// Why a fixture run waits, from what it shows (the engine keeps it as the run moves).
fn wait_of(run: &Run) -> Option<RunWait> {
    match run.state {
        RunState::WaitingReview => Some(RunWait::Review),
        RunState::WaitingAnswer if run.nodes.iter().any(|n| n.question.is_some()) => Some(RunWait::Question),
        RunState::WaitingAnswer => Some(RunWait::Budget),
        _ => None,
    }
}

use super::{FixtureApi, World, data_inbox};

impl FixtureApi {
    /// Appends an activity entry and returns its event.
    fn activity(world: &mut World, key: &IssueKey, actor: Actor, kind: IssueEventKind) -> Event {
        let at = world.now;
        // Above every entry, deleted ones too: seqs are never reused.
        let deleted = world.deleted_seqs.get(key).copied().unwrap_or(0);
        let feed = world.issue_events.entry(key.clone()).or_default();
        let seq = feed.iter().map(|e| e.seq).max().unwrap_or(0).max(deleted) + 1;
        let event = IssueEvent { seq, at, actor, kind };
        feed.push(event.clone());
        Event::IssueActivity { key: key.clone(), event }
    }

    fn new_run(&self, pipeline: &PipelineId, input: RunInput, test: bool) -> Run {
        let mut world = self.world();
        let id = RunId::new(world.fresh_id("run"));
        let run = Run {
            id,
            pipeline: pipeline.clone(),
            version: if test { 0 } else { 14 },
            issue: None,
            number: 1,
            title: input.title,
            input: input.text,
            state: RunState::Running,
            test,
            started: world.now,
            finished: None,
            cost_usd: 0.0,
            usage: Default::default(),
            error: None,
            pr_url: None,
            nodes: Vec::new(),
        };
        world.runs.push(run.clone());
        run
    }

    fn set_run_state(&self, id: &RunId, state: RunState) -> Result<()> {
        let run = {
            let mut world = self.world();
            let run = world.runs.iter_mut().find(|run| &run.id == id).ok_or_else(|| anyhow!("no run {id}"))?;
            run.state = state;
            run.clone()
        };
        self.emit(Event::RunChanged { run });
        Ok(())
    }
}

impl IssueApi for FixtureApi {
    fn issues(&self, project: &ProjectId) -> Result<Vec<Issue>> {
        Ok(self.world().issues.iter().filter(|issue| &issue.project == project).cloned().collect())
    }

    fn issue(&self, key: &IssueKey) -> Result<Option<Issue>> {
        Ok(self.world().issues.iter().find(|issue| &issue.key == key).cloned())
    }

    fn issue_events(&self, key: &IssueKey) -> Result<Vec<IssueEvent>> {
        Ok(self.world().issue_events.get(key).cloned().unwrap_or_default())
    }

    fn create_issue(&self, new: NewIssue) -> Result<Issue> {
        self.record("create_issue", new.title.clone())?;
        let (issue, created) = {
            let mut world = self.world();
            let prefix = world.projects.iter().find(|p| p.id == new.project).map_or("PAY".to_string(), |p| p.key_prefix.clone());
            let number = 1443 + world.issues.len() as u64 - 10;
            let now = world.now;
            let issue = Issue {
                key: IssueKey::new(format!("{prefix}-{number}")),
                project: new.project,
                title: new.title,
                body: new.body,
                status: new.status,
                priority: new.priority,
                kind: new.kind,
                owner: new.owner,
                repo: new.repo,
                created: now,
                updated: now,
                filed_by: None,
                runner: new.runner,
                triggers: new.triggers,
                run: None,
                run_count: 0,
                pr_url: None,
                images: new.images,
            };
            world.issues.push(issue.clone());
            let created = Self::activity(&mut world, &issue.key, Actor::Me, IssueEventKind::Created);
            (issue, created)
        };
        self.emit_all([Event::IssueChanged { issue: issue.clone() }, created]);
        Ok(issue)
    }

    fn update_issue(&self, key: &IssueKey, patch: IssuePatch) -> Result<Issue> {
        self.record("update_issue", format!("{key} {patch:?}"))?;
        let (issue, activity) = {
            let mut world = self.world();
            let now = world.now;
            let issue = world.issue_mut(key)?;
            let from = issue.status;
            if let Some(title) = patch.title {
                issue.title = title;
            }
            if let Some(body) = patch.body {
                issue.body = body;
            }
            if let Some(status) = patch.status {
                issue.status = status;
            }
            if let Some(priority) = patch.priority {
                issue.priority = priority;
            }
            if let Some(kind) = patch.kind {
                issue.kind = kind;
            }
            if let Some(owner) = patch.owner {
                issue.owner = owner;
            }
            if let Some(repo) = patch.repo {
                issue.repo = repo;
            }
            if let Some(triggers) = patch.triggers {
                issue.triggers = triggers;
            }
            issue.updated = now;
            let issue = issue.clone();
            let activity = (issue.status != from)
                .then(|| Self::activity(&mut world, key, Actor::Me, IssueEventKind::StatusChanged { from, to: issue.status }));
            (issue, activity)
        };
        self.emit(Event::IssueChanged { issue: issue.clone() });
        self.emit_all(activity);
        Ok(issue)
    }

    fn delete_issue(&self, key: &IssueKey) -> Result<()> {
        self.record("delete_issue", key.to_string())?;
        {
            let mut world = self.world();
            world.issue_mut(key)?;
            world.issues.retain(|issue| &issue.key != key);
        }
        self.emit(Event::IssueRemoved { key: key.clone() });
        Ok(())
    }

    fn comment(&self, key: &IssueKey, text: &str, images: Vec<Attachment>) -> Result<()> {
        self.record("comment", format!("{key} {text}"))?;
        let event = {
            let mut world = self.world();
            world.issue_mut(key)?;
            let mentions = world
                .runnables
                .iter()
                .filter(|runnable| text.contains(&format!("@{}", runnable.name)))
                .map(|runnable| runnable.reference.clone())
                .collect();
            Self::activity(&mut world, key, Actor::Me, IssueEventKind::Comment { text: text.to_string(), images, mentions })
        };
        self.emit(event);
        Ok(())
    }

    fn delete_comment(&self, key: &IssueKey, seq: u64) -> Result<()> {
        self.record("delete_comment", format!("{key} {seq}"))?;
        let issue = {
            let mut world = self.world();
            let feed = world.issue_events.get(key).ok_or_else(|| anyhow!("{key} has no comment {seq}"))?;
            let index = feed.iter().position(|e| e.seq == seq).ok_or_else(|| anyhow!("{key} has no comment {seq}"))?;
            match &feed[index] {
                IssueEvent { actor: Actor::Me, kind: IssueEventKind::Comment { .. }, .. } => {}
                IssueEvent { kind: IssueEventKind::Comment { .. }, .. } => bail!("Only your own comments can be deleted"),
                _ => bail!("Only comments can be deleted"),
            }
            if let Some(feed) = world.issue_events.get_mut(key) {
                feed.remove(index);
            }
            let highest = world.deleted_seqs.entry(key.clone()).or_default();
            *highest = (*highest).max(seq);
            let now = world.now;
            let issue = world.issue_mut(key)?;
            issue.updated = now;
            issue.clone()
        };
        self.emit(Event::IssueChanged { issue });
        Ok(())
    }

    fn attach_runner(&self, key: &IssueKey, runner: Option<RunnableRef>) -> Result<Issue> {
        self.record("attach_runner", format!("{key} {runner:?}"))?;
        let (issue, event) = {
            let mut world = self.world();
            let issue = world.issue_mut(key)?;
            issue.runner = runner.clone();
            let issue = issue.clone();
            let kind = match runner {
                Some(runner) => IssueEventKind::Attached { runner },
                None => IssueEventKind::Detached,
            };
            (issue, Self::activity(&mut world, key, Actor::Me, kind))
        };
        self.emit_all([Event::IssueChanged { issue: issue.clone() }, event]);
        Ok(issue)
    }

    fn start_issue(&self, key: &IssueKey) -> Result<()> {
        self.record("start_issue", key.to_string())?;
        let issue = {
            let mut world = self.world();
            let issue = world.issue_mut(key)?;
            if issue.runner.is_none() {
                bail!("nothing is attached to {key}");
            }
            issue.status = crate::api::IssueStatus::InProgress;
            issue.clone()
        };
        self.emit(Event::IssueChanged { issue });
        Ok(())
    }

    fn retry_wake(&self, key: &IssueKey, target: &RunnableRef) -> Result<()> {
        self.record("retry_wake", format!("{key} {target}"))?;
        bail!("runtime laptop is still offline")
    }

    fn next_scheduled_runs(&self) -> Result<Vec<(RunnableRef, i64)>> {
        Ok(crate::api::next_scheduled_runs_of(&self.world().autopilots))
    }

    fn autopilots(&self, project: &ProjectId) -> Result<Vec<Autopilot>> {
        Ok(self.world().autopilots.iter().filter(|autopilot| &autopilot.project == project).cloned().collect())
    }

    fn save_autopilot(&self, autopilot: Autopilot) -> Result<()> {
        self.record("save_autopilot", autopilot.name.clone())?;
        {
            let mut world = self.world();
            world.autopilots.retain(|known| known.id != autopilot.id);
            world.autopilots.push(autopilot);
        }
        self.emit(Event::AutopilotsChanged);
        Ok(())
    }

    fn delete_autopilot(&self, id: &AutopilotId) -> Result<()> {
        self.record("delete_autopilot", id.to_string())?;
        self.world().autopilots.retain(|known| &known.id != id);
        self.emit(Event::AutopilotsChanged);
        Ok(())
    }

    fn run_autopilot(&self, id: &AutopilotId) -> Result<()> {
        self.record("run_autopilot", id.to_string())?;
        {
            let mut world = self.world();
            let now = world.now;
            let autopilot = world.autopilots.iter_mut().find(|known| &known.id == id).ok_or_else(|| anyhow!("There is no autopilot {id}"))?;
            autopilot.last_run = Some(now);
            autopilot.last_result = "started a run".to_string();
        }
        self.emit(Event::AutopilotsChanged);
        Ok(())
    }
}

impl RunApi for FixtureApi {
    fn run(&self, id: &RunId) -> Result<Option<Run>> {
        Ok(self.world().runs.iter().find(|run| &run.id == id).cloned())
    }

    fn latest_runs(&self) -> Result<Vec<Run>> {
        let world = self.world();
        let mut latest: Vec<Run> = Vec::new();
        for run in &world.runs {
            match &run.issue {
                Some(issue) => {
                    latest.retain(|known| known.issue.as_ref() != Some(issue) || known.number > run.number);
                    if !latest.iter().any(|known| known.issue.as_ref() == Some(issue)) {
                        latest.push(run.clone());
                    }
                }
                None if run.state.is_open() => latest.push(run.clone()),
                None => {}
            }
        }
        Ok(latest)
    }

    fn runs_for_issue(&self, key: &IssueKey) -> Result<Vec<Run>> {
        Ok(self.world().runs.iter().filter(|run| run.issue.as_ref() == Some(key)).cloned().collect())
    }

    fn runs_for_pipeline(&self, id: &PipelineId, limit: usize) -> Result<Vec<Run>> {
        let mut runs: Vec<Run> = self.world().runs.iter().filter(|run| &run.pipeline == id).cloned().collect();
        runs.sort_by_key(|run| std::cmp::Reverse(run.started));
        runs.truncate(limit);
        Ok(runs)
    }

    fn run_waiting(&self, id: &RunId) -> Result<Option<RunWait>> {
        Ok(self.run(id)?.as_ref().and_then(wait_of))
    }

    fn waiting_runs(&self) -> Result<Vec<(RunId, RunWait)>> {
        Ok(self.world().runs.iter().filter_map(|run| Some((run.id.clone(), wait_of(run)?))).collect())
    }

    fn run_log(&self, id: &RunId, after_seq: u64, limit: usize) -> Result<Vec<RunLogLine>> {
        let world = self.world();
        Ok(world.logs.get(id).map_or_else(Vec::new, |log| log.iter().filter(|l| l.seq > after_seq).take(limit).cloned().collect()))
    }

    fn run_log_tail(&self, id: &RunId, count: usize) -> Result<Vec<RunLogLine>> {
        let world = self.world();
        Ok(world.logs.get(id).map_or_else(Vec::new, |log| log[log.len().saturating_sub(count)..].to_vec()))
    }

    fn run_stats(&self, id: &RunId) -> Result<RunStats> {
        let run = self.run(id)?.ok_or_else(|| anyhow!("no run {id}"))?;
        Ok(RunStats {
            agents_running: run.nodes.iter().filter(|n| n.status == crate::api::NodeStatus::Running).count() as u32,
            agents_max: 4,
            spawned: run.nodes.iter().filter(|n| n.kind == crate::api::SpanKind::Spawned).count() as u32,
            waiting_variants: run.nodes.iter().filter(|n| n.status == crate::api::NodeStatus::Waiting).count() as u32,
            cost_usd: run.cost_usd,
            budget_usd: Some(3.0),
        })
    }

    fn start_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run> {
        self.record("start_run", format!("{pipeline} {}", input.title))?;
        let run = self.new_run(pipeline, input, false);
        self.emit(Event::RunChanged { run: run.clone() });
        Ok(run)
    }

    fn test_run(&self, pipeline: &PipelineId, input: RunInput) -> Result<Run> {
        self.record("test_run", format!("{pipeline} {}", input.title))?;
        let run = self.new_run(pipeline, input, true);
        self.emit(Event::RunChanged { run: run.clone() });
        Ok(run)
    }

    fn cancel_run(&self, id: &RunId) -> Result<()> {
        self.record("cancel_run", id.to_string())?;
        self.set_run_state(id, RunState::Cancelled)
    }

    fn resume_run(&self, id: &RunId) -> Result<()> {
        self.record("resume_run", id.to_string())?;
        self.set_run_state(id, RunState::Running)
    }

    fn answer_run(&self, id: &RunId, node: Option<&crate::api::NodeId>, text: &str) -> Result<()> {
        let node = node.map(ToString::to_string).unwrap_or_default();
        self.record("answer_run", format!("{id} {node} {text}"))?;
        self.set_run_state(id, RunState::Running)
    }

    fn raise_run_budget(&self, id: &RunId, usd: f64) -> Result<()> {
        self.record("raise_run_budget", format!("{id} {usd}"))?;
        self.set_run_state(id, RunState::Running)
    }
}

impl InboxApi for FixtureApi {
    fn inbox(&self, archived: bool) -> Result<Vec<InboxItem>> {
        let mut items: Vec<InboxItem> = self.world().inbox.iter().filter(|item| item.archived == archived).cloned().collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.created));
        Ok(items)
    }

    fn set_inbox_read(&self, id: &InboxId, read: bool) -> Result<()> {
        self.record("set_inbox_read", format!("{id} {read}"))?;
        self.change_item(id, |item| item.read = read)
    }

    fn archive_inbox(&self, id: &InboxId, archived: bool) -> Result<()> {
        self.record("archive_inbox", format!("{id} {archived}"))?;
        self.change_item(id, |item| item.archived = archived)
    }

    fn open_reviews(&self) -> Result<Vec<Review>> {
        Ok(self.world().reviews.iter().filter(|review| review.state == ReviewState::Open).cloned().collect())
    }

    fn review(&self, id: &ReviewId) -> Result<Option<Review>> {
        Ok(self.world().reviews.iter().find(|review| &review.id == id).cloned())
    }

    fn variant_diff(&self, id: &ReviewId, variant: u32, path: &str) -> Result<FileDiff> {
        let review = self.review(id)?.ok_or_else(|| anyhow!("no review {id}"))?;
        data_inbox::variant_diff(&review, variant, path).ok_or_else(|| anyhow!("variant {variant} did not change {path}"))
    }

    fn send_back_estimate(&self, id: &ReviewId, start: StartFrom, variants: u32) -> Result<Estimate> {
        let review = self.review(id)?.ok_or_else(|| anyhow!("no review {id}"))?;
        let count = review.variants.len().max(1) as f64;
        let secs = review.variants.iter().map(|v| v.secs).sum::<u64>() as f64 / count;
        let cost = review.variants.iter().map(|v| v.cost_usd).sum::<f64>() / count;
        let factor = match start {
            StartFrom::Variant { .. } => 0.6,
            StartFrom::All | StartFrom::Over => 1.0,
        };
        Ok(Estimate { secs: (secs * factor) as u64, cost_usd: cost * factor * f64::from(variants) })
    }

    fn decide_review(&self, id: &ReviewId, decision: ReviewDecision) -> Result<()> {
        self.record("decide_review", format!("{id} {decision:?}"))?;
        let (review, items) = {
            let mut world = self.world();
            let review = world.reviews.iter_mut().find(|review| &review.id == id).ok_or_else(|| anyhow!("no review {id}"))?;
            if review.state != ReviewState::Open {
                bail!("this review was already decided");
            }
            match &decision {
                ReviewDecision::Ship { variant } => {
                    review.state = ReviewState::Shipped;
                    review.picked = Some(*variant);
                }
                ReviewDecision::Skip => review.state = ReviewState::Skipped,
                ReviewDecision::SendBack { .. } => review.state = ReviewState::SentBack,
            }
            let review = review.clone();
            let mut items = Vec::new();
            for item in world.inbox.iter_mut() {
                if matches!(&item.kind, crate::api::InboxKind::Review { review } if review == id) {
                    item.archived = true;
                    items.push(item.clone());
                }
            }
            (review, items)
        };
        self.emit(Event::ReviewChanged { review });
        self.emit_all(items.into_iter().map(|item| Event::InboxChanged { item }));
        self.emit(Event::SpendChanged);
        Ok(())
    }

    fn pull_request(&self, url: &str) -> Result<PullRequest> {
        let number = url.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(2280);
        Ok(PullRequest {
            url: url.to_string(),
            number,
            title: "Webhook signature check".to_string(),
            state: "merged".to_string(),
            checks: "passing".to_string(),
            review: "approved".to_string(),
            added: 84,
            removed: 12,
            head: "fix/PAY-1419".to_string(),
            base: "main".to_string(),
        })
    }
}

impl FixtureApi {
    fn change_item(&self, id: &InboxId, f: impl FnOnce(&mut InboxItem)) -> Result<()> {
        let item = {
            let mut world = self.world();
            let item = world.inbox.iter_mut().find(|item| &item.id == id).ok_or_else(|| anyhow!("no inbox item {id}"))?;
            f(item);
            item.clone()
        };
        self.emit(Event::InboxChanged { item });
        Ok(())
    }
}
