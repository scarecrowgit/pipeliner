//! [`IssueApi`] over the store and the `issues` rules. Every change is recorded in the
//! issue's activity feed; what the triggers and mentions wake is started by `wake`.

use anyhow::{Result, anyhow, bail};
use issues::Happening;
use model::{Actor, Autopilot, AutopilotId, AutopilotTrigger, Event, Issue, IssueEvent, IssueEventKind, IssueKey, ProjectId, RunAgent, RunnableRef};

use crate::api::{Attachment, IssueApi, IssuePatch, NewIssue};
use crate::engine::{Engine, Inner};

impl Inner {
    fn issue_or_fail(&self, key: &IssueKey) -> Result<Issue> {
        self.store.issue(key)?.ok_or_else(|| anyhow!("There is no issue {key}"))
    }

    /// Appends to the feed and publishes the entry.
    pub(crate) fn activity(&self, key: &IssueKey, actor: Actor, kind: IssueEventKind) -> Result<()> {
        let event = IssueEvent { seq: 0, at: self.now(), actor, kind };
        let event = self.store.append_issue_event(key, event)?;
        self.publish(Event::IssueActivity { key: key.clone(), event });
        Ok(())
    }

    pub(crate) fn save_issue(&self, issue: &Issue) -> Result<()> {
        self.store.upsert_issue(issue)?;
        self.publish(Event::IssueChanged { issue: issue.clone() });
        Ok(())
    }
}

impl IssueApi for Engine {
    fn issues(&self, project: &ProjectId) -> Result<Vec<Issue>> {
        self.inner.store.issues(project)
    }

    fn issue(&self, key: &IssueKey) -> Result<Option<Issue>> {
        self.inner.store.issue(key)
    }

    fn issue_events(&self, key: &IssueKey) -> Result<Vec<IssueEvent>> {
        self.inner.store.issue_events(key)
    }

    fn create_issue(&self, new: NewIssue) -> Result<Issue> {
        let inner = &self.inner;
        let title = new.title.trim();
        if title.is_empty() {
            bail!("An issue needs a title");
        }
        let project = inner.store.project(&new.project)?.ok_or_else(|| anyhow!("There is no project {}", new.project))?;
        let key = inner.store.next_issue_key(&project.key_prefix)?;
        let now = inner.now();
        let issue = Issue {
            key: key.clone(),
            project: project.id,
            title: title.to_string(),
            body: new.body,
            status: new.status,
            priority: new.priority,
            kind: new.kind,
            owner: new.owner,
            repo: new.repo,
            created: now,
            updated: now,
            filed_by: None,
            runner: new.runner.clone(),
            triggers: new.triggers,
            run: None,
            run_count: 0,
            pr_url: None,
            images: new.images,
        };
        inner.save_issue(&issue)?;
        inner.activity(&key, Actor::Me, IssueEventKind::Created)?;
        if let Some(runner) = new.runner {
            inner.activity(&key, Actor::Me, IssueEventKind::Attached { runner })?;
        }
        self.wake(&issue, &Happening::Created)?;
        Ok(issue)
    }

    fn update_issue(&self, key: &IssueKey, patch: IssuePatch) -> Result<Issue> {
        let inner = &self.inner;
        let mut issue = inner.issue_or_fail(key)?;
        let before = issue.status;
        if let Some(title) = patch.title {
            if title.trim().is_empty() {
                bail!("An issue needs a title");
            }
            issue.title = title.trim().to_string();
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
        issue.updated = inner.now();
        // The triggers look at the issue as it was before the move.
        let moved = (issue.status != before).then_some(issue.status);
        let mut before_move = issue.clone();
        before_move.status = before;
        inner.save_issue(&issue)?;
        if let Some(to) = moved {
            inner.activity(key, Actor::Me, IssueEventKind::StatusChanged { from: before, to })?;
            self.wake(&before_move, &Happening::MovedTo(to))?;
        }
        Ok(issue)
    }

    fn delete_issue(&self, key: &IssueKey) -> Result<()> {
        if !self.inner.store.delete_issue(key)? {
            bail!("There is no issue {key}");
        }
        self.inner.publish(Event::IssueRemoved { key: key.clone() });
        Ok(())
    }

    fn comment(&self, key: &IssueKey, text: &str, images: Vec<Attachment>) -> Result<()> {
        let inner = &self.inner;
        if text.trim().is_empty() && images.is_empty() {
            bail!("The comment is empty");
        }
        let mut issue = inner.issue_or_fail(key)?;
        let catalog = inner.catalog();
        // The agents of the latest run come first: a name they share with the library is theirs.
        let agents = self.run_agents_of(key)?;
        let mentions = issues::mentions::targets(&issues::mentions::find_mentions_with(text, &agents, catalog.runnables()));
        let kind = IssueEventKind::Comment { text: text.to_string(), images: images.clone(), mentions: mentions.clone() };
        inner.activity(key, Actor::Me, kind)?;
        issue.updated = inner.now();
        inner.save_issue(&issue)?;
        // Only a person's comment wakes anyone; agents answer through `wake::reply`, which
        // never comes here.
        self.wake_with(&issue, &Happening::Comment { text: text.to_string(), mentions }, &images)
    }

    fn issue_agents(&self, key: &IssueKey) -> Result<Vec<RunAgent>> {
        self.run_agents_of(key)
    }

    fn delete_comment(&self, key: &IssueKey, seq: u64) -> Result<()> {
        let inner = &self.inner;
        let mut issue = inner.issue_or_fail(key)?;
        let events = inner.store.issue_events(key)?;
        let event = events.iter().find(|e| e.seq == seq).ok_or_else(|| anyhow!("{key} has no comment {seq}"))?;
        if !matches!(event.kind, IssueEventKind::Comment { .. }) {
            bail!("Only comments can be deleted");
        }
        if event.actor != Actor::Me {
            bail!("Only your own comments can be deleted");
        }
        if !inner.store.delete_issue_event(key, seq)? {
            bail!("{key} has no comment {seq}");
        }
        issue.updated = inner.now();
        inner.save_issue(&issue)
    }

    fn attach_runner(&self, key: &IssueKey, runner: Option<RunnableRef>) -> Result<Issue> {
        let inner = &self.inner;
        let mut issue = inner.issue_or_fail(key)?;
        if issue.runner == runner {
            return Ok(issue);
        }
        issue.runner = runner.clone();
        issue.updated = inner.now();
        inner.save_issue(&issue)?;
        match runner {
            Some(runner) => {
                inner.activity(key, Actor::Me, IssueEventKind::Attached { runner })?;
                self.wake(&issue, &Happening::Attached)?;
            }
            None => inner.activity(key, Actor::Me, IssueEventKind::Detached)?,
        }
        Ok(issue)
    }

    fn start_issue(&self, key: &IssueKey) -> Result<()> {
        let issue = self.inner.issue_or_fail(key)?;
        let Some(runner) = issue.runner.clone() else { bail!("{key} has no runner attached under “Runs with”") };
        self.wake_by_hand(&issue, &runner, "started by hand")
    }

    fn retry_wake(&self, key: &IssueKey, target: &RunnableRef) -> Result<()> {
        let issue = self.inner.issue_or_fail(key)?;
        if target.session_id().is_some() {
            return self.rewake_session(&issue, target);
        }
        self.wake_by_hand(&issue, target, "tried again")
    }

    fn autopilots(&self, project: &ProjectId) -> Result<Vec<Autopilot>> {
        self.inner.store.autopilots(Some(project))
    }

    fn save_autopilot(&self, mut autopilot: Autopilot) -> Result<()> {
        let inner = &self.inner;
        let now = inner.now();
        autopilot.next_run = match &autopilot.trigger {
            AutopilotTrigger::Schedule { schedule } if autopilot.enabled => {
                Some(issues::autopilot::next_run(schedule, now, base::time::utc_offset(now)))
            }
            _ => None,
        };
        inner.store.upsert_autopilot(&autopilot)?;
        inner.publish(Event::AutopilotsChanged);
        inner.wakeup.poke();
        Ok(())
    }

    fn delete_autopilot(&self, id: &AutopilotId) -> Result<()> {
        if !self.inner.store.delete_autopilot(id)? {
            bail!("There is no autopilot {id}");
        }
        self.inner.publish(Event::AutopilotsChanged);
        self.inner.wakeup.poke();
        Ok(())
    }

    fn next_scheduled_runs(&self) -> Result<Vec<(RunnableRef, model::Millis)>> {
        Ok(crate::api::next_scheduled_runs_of(&self.inner.store.autopilots(None)?))
    }

    fn run_autopilot(&self, id: &AutopilotId) -> Result<()> {
        let autopilot = self
            .inner
            .store
            .autopilots(None)?
            .into_iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| anyhow!("There is no autopilot {id}"))?;
        self.fire_autopilot(autopilot, false)
    }
}
