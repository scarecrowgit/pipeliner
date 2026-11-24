//! [`IssueApi`] over the store and the `issues` rules. Every change is recorded in the
//! issue's activity feed. Starting runners from triggers is not wired yet: a wake-up the
//! triggers ask for is recorded as failed, with that reason.

use anyhow::{Result, anyhow, bail};
use issues::Happening;
use model::{Actor, Autopilot, AutopilotId, AutopilotTrigger, Event, Issue, IssueEvent, IssueEventKind, IssueKey, ProjectId, RunnableRef};

use crate::api::{Attachment, IssueApi, IssuePatch, NewIssue};
use crate::engine::{Engine, Inner};

/// Why a wake-up asked for by a trigger did not start anything.
const WAKE_NOT_WIRED: &str = "starting runs from issues is not wired yet";

impl Inner {
    fn issue_or_fail(&self, key: &IssueKey) -> Result<Issue> {
        self.store.issue(key)?.ok_or_else(|| anyhow!("There is no issue {key}"))
    }

    /// Appends to the feed and publishes the entry.
    fn activity(&self, key: &IssueKey, actor: Actor, kind: IssueEventKind) -> Result<()> {
        let event = IssueEvent { seq: 0, at: self.now(), actor, kind };
        let event = self.store.append_issue_event(key, event)?;
        self.publish(Event::IssueActivity { key: key.clone(), event });
        Ok(())
    }

    fn save_issue(&self, issue: &Issue) -> Result<()> {
        self.store.upsert_issue(issue)?;
        self.publish(Event::IssueChanged { issue: issue.clone() });
        Ok(())
    }

    /// Records what the triggers would wake for `happening`: nothing can start yet, so every
    /// intent becomes a failed wake-up, and every refusal one with its own reason.
    fn wake(&self, issue: &Issue, happening: &Happening) -> Result<()> {
        let catalog = self.catalog();
        let wakes = issues::triggers::wake_intents(issue, happening, catalog.runnables());
        for intent in wakes.intents {
            let kind = IssueEventKind::WakeFailed { target: intent.target, name: intent.name, reason: WAKE_NOT_WIRED.into() };
            self.activity(&issue.key, Actor::System, kind)?;
        }
        for refusal in wakes.refusals {
            let kind = IssueEventKind::WakeFailed { target: refusal.target, name: refusal.name, reason: refusal.reason };
            self.activity(&issue.key, Actor::System, kind)?;
        }
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
        inner.wake(&issue, &Happening::Created)?;
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
            inner.wake(&before_move, &Happening::MovedTo(to))?;
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
        let mentions = issues::mentions::targets(&issues::mentions::find_mentions(text, catalog.runnables()));
        let kind = IssueEventKind::Comment { text: text.to_string(), images, mentions: mentions.clone() };
        inner.activity(key, Actor::Me, kind)?;
        issue.updated = inner.now();
        inner.save_issue(&issue)?;
        inner.wake(&issue, &Happening::Comment { text: text.to_string(), mentions })
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
                inner.wake(&issue, &Happening::Attached)?;
            }
            None => inner.activity(key, Actor::Me, IssueEventKind::Detached)?,
        }
        Ok(issue)
    }

    fn start_issue(&self, key: &IssueKey) -> Result<()> {
        self.inner.issue_or_fail(key)?;
        bail!("Starting runs from issues is not wired yet")
    }

    fn retry_wake(&self, key: &IssueKey, _target: &RunnableRef) -> Result<()> {
        self.inner.issue_or_fail(key)?;
        bail!("Starting runs from issues is not wired yet")
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
        Ok(())
    }

    fn delete_autopilot(&self, id: &AutopilotId) -> Result<()> {
        if !self.inner.store.delete_autopilot(id)? {
            bail!("There is no autopilot {id}");
        }
        self.inner.publish(Event::AutopilotsChanged);
        Ok(())
    }
}
