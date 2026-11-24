//! What a run's driver tells the engine: snapshots and runs go to the store, reviews and
//! questions to the Inbox, the issue follows the run.

use std::collections::BTreeMap;
use std::sync::Weak;

use model::{
    Actor, Event, GraphDef, InboxId, InboxItem, InboxKind, IssueEventKind, Outcome, RequestId,
    Review, ReviewState, Run, RunId, RunLogLine, RunState,
};
use pipeline::InstanceKey;
use pipeline::exec::{Observer, Snapshot};

use super::{question_inbox_id, review_inbox_id, snapshot_marker};
use crate::engine::Inner;

/// The observer of one run.
pub(crate) struct RunObserver {
    pub inner: Weak<Inner>,
    pub graph: GraphDef,
    /// A merge action of the run left its branch in the base (the latest snapshot says so).
    pub merged: bool,
    /// Set when a snapshot could not be stored; the run's jobs then refuse to work.
    pub broken: super::Broken,
    /// The state the issue last followed.
    pub followed: Option<RunState>,
    /// Questions posted to the Inbox and not answered yet.
    pub asked: BTreeMap<InstanceKey, InboxId>,
}

impl RunObserver {
    fn with(&self, act: impl FnOnce(&Inner) -> anyhow::Result<()>) {
        if let Some(inner) = self.inner.upgrade()
            && let Err(error) = act(&inner)
        {
            inner.notice_error(&format!("A pipeline run could not be saved: {error:#}"));
        }
    }

    /// What the run left for its issue: merged, a pull request, or neither.
    fn result(&self, run: &Run) -> issues::RunResult {
        if run.pr_url.is_some() {
            return issues::RunResult::PullRequestOpen;
        }
        if self.merged { issues::RunResult::Merged } else { issues::RunResult::NoResult }
    }

    fn follow_issue(&mut self, inner: &Inner, run: &Run) -> anyhow::Result<()> {
        if run.test || self.followed == Some(run.state) {
            return Ok(());
        }
        self.followed = Some(run.state);
        let Some(key) = &run.issue else { return Ok(()) };
        let Some(mut issue) = inner.store.issue(key)? else { return Ok(()) };
        let to = issues::status::status_after_run(issue.status, &run.state, self.result(run));
        let mut changed = false;
        if issue.pr_url.is_none() && run.pr_url.is_some() {
            issue.pr_url = run.pr_url.clone();
            changed = true;
        }
        let from = issue.status;
        if to != from {
            issue.status = to;
            changed = true;
        }
        if changed {
            issue.updated = inner.now();
            inner.save_issue(&issue)?;
        }
        if to != from {
            inner.activity(key, Actor::System, IssueEventKind::StatusChanged { from, to })?;
        }
        Ok(())
    }

    /// Questions whose instance no longer waits are taken out of the Inbox.
    fn settle_questions(&mut self, inner: &Inner, run: &Run) -> anyhow::Result<()> {
        let answered: Vec<InstanceKey> = self
            .asked
            .keys()
            .filter(|key| {
                !run.nodes.iter().any(|n| {
                    n.node == key.node && n.variant == key.variant && n.attempt == key.attempt && n.question.is_some()
                })
            })
            .cloned()
            .collect();
        for key in answered {
            if let Some(id) = self.asked.remove(&key) {
                resolve(inner, &id)?;
            }
        }
        Ok(())
    }
}

/// Marks an Inbox item as dealt with.
pub(crate) fn resolve(inner: &Inner, id: &InboxId) -> anyhow::Result<()> {
    if inner.store.set_inbox_archived(id, true)? {
        inner.store.set_inbox_read(id, true)?;
        if let Some(item) = inner.store.inbox_item(id)? {
            inner.publish(Event::InboxChanged { item });
        }
    }
    Ok(())
}

impl Observer for RunObserver {
    fn snapshot(&mut self, snapshot: &Snapshot) {
        self.merged = snapshot.meta.iter().any(|m| !m.superseded && m.output.as_ref().is_some_and(|o| o.merged));
        let Some(inner) = self.inner.upgrade() else { return };
        let stored = serde_json::to_string(snapshot).map_err(anyhow::Error::from).and_then(|text| {
            #[cfg(test)]
            if inner.fail_snapshots.load(std::sync::atomic::Ordering::SeqCst) {
                anyhow::bail!("the store refused the write (test)");
            }
            inner.store.set_marker(&snapshot_marker(&snapshot.run.id), &text)
        });
        if let Err(error) = stored {
            let text = format!("{error:#}");
            if self.broken.set(text.clone()).is_ok() {
                inner.notice_error(&format!("The run {} was stopped: its state could not be saved ({text})", snapshot.run.id));
            }
        }
    }

    fn run_changed(&mut self, run: &Run) {
        let Some(inner) = self.inner.upgrade() else { return };
        let mut saved = || -> anyhow::Result<()> {
            inner.store.upsert_run(run)?;
            inner.publish(Event::RunChanged { run: run.clone() });
            self.follow_issue(&inner, run)?;
            self.settle_questions(&inner, run)
        };
        if let Err(error) = saved() {
            inner.notice_error(&format!("A pipeline run could not be saved: {error:#}"));
        }
    }

    fn review_changed(&mut self, review: &Review) {
        let graph_name = self.graph.name.clone();
        self.with(|inner| {
            inner.store.upsert_review(review)?;
            inner.publish(Event::ReviewChanged { review: review.clone() });
            let id = review_inbox_id(&review.id);
            if review.state != ReviewState::Open {
                return resolve(inner, &id);
            }
            let count = review.variants.len() as u64;
            let source = if count > 1 {
                format!("{} · {}", review.agent, base::fmt::plural(count, "variant", "variants"))
            } else if review.agent.is_empty() {
                graph_name
            } else {
                review.agent.clone()
            };
            let summary = review.variants.iter().map(|v| format!("- **{}**: {}", v.name, base::fmt::first_line(&v.approach))).collect::<Vec<_>>().join("\n");
            let item = InboxItem {
                id,
                kind: InboxKind::Review { review: review.id.clone() },
                title: review.title.clone(),
                summary,
                outcome: Some(Outcome::NeedsReview),
                source,
                issue: review.issue.clone(),
                session: review.variants.first().and_then(|v| v.session.clone()),
                run: Some(review.run.clone()),
                links: Vec::new(),
                created: inner.now(),
                read: false,
                archived: false,
            };
            inner.store.upsert_inbox_item(&item)?;
            inner.publish(Event::InboxChanged { item });
            Ok(())
        });
    }

    fn log(&mut self, run: &RunId, line: &RunLogLine) {
        self.with(|inner| {
            for line in inner.store.append_run_log(run, std::slice::from_ref(line))? {
                inner.publish(Event::RunLog { run: run.clone(), line });
            }
            Ok(())
        });
    }

    fn asked(&mut self, run: &RunId, key: &InstanceKey, question: &str) {
        let id = question_inbox_id(run, key);
        self.asked.insert(key.clone(), id.clone());
        let name = self.graph.node(&key.node).map_or_else(|| key.node.to_string(), |d| d.name.clone());
        self.with(|inner| {
            let stored = inner.store.run(run)?;
            let item = InboxItem {
                id: id.clone(),
                kind: InboxKind::Question { request: RequestId::new(id.as_str()) },
                title: base::fmt::truncate_chars(base::fmt::first_line(question), 120),
                summary: question.to_string(),
                outcome: Some(Outcome::Blocked),
                source: format!("{} · {name}", self.graph.name),
                issue: stored.as_ref().and_then(|r| r.issue.clone()),
                session: stored.as_ref().and_then(|r| r.instance(&key.node, key.variant)).and_then(|n| n.session.clone()),
                run: Some(run.clone()),
                links: Vec::new(),
                created: inner.now(),
                read: false,
                archived: false,
            };
            inner.store.upsert_inbox_item(&item)?;
            inner.publish(Event::InboxChanged { item });
            Ok(())
        });
    }

    fn finished(&mut self, run: &Run) {
        let asked: Vec<InboxId> = std::mem::take(&mut self.asked).into_values().collect();
        let name = self.graph.name.clone();
        self.with(|inner| {
            for id in &asked {
                resolve(inner, id)?;
            }
            if run.state == RunState::Failed && !run.test {
                let item = InboxItem {
                    id: InboxId::new(format!("failed:{}", run.id)),
                    kind: InboxKind::Failure,
                    title: run.title.clone(),
                    summary: run.error.clone().unwrap_or_else(|| "The run failed.".into()),
                    outcome: Some(Outcome::Failed),
                    source: name,
                    issue: run.issue.clone(),
                    session: None,
                    run: Some(run.id.clone()),
                    links: Vec::new(),
                    created: inner.now(),
                    read: false,
                    archived: false,
                };
                inner.store.upsert_inbox_item(&item)?;
                inner.publish(Event::InboxChanged { item });
            }
            Ok(())
        });
        // The driver is dropped off its own thread: dropping it joins that thread.
        let (weak, id) = (self.inner.clone(), run.id.clone());
        let _ = std::thread::Builder::new().name("run-reaper".into()).spawn(move || {
            // The closing state may reach the store just after this call.
            for _ in 0..100 {
                let Some(inner) = weak.upgrade() else { return };
                if inner.store.run(&id).ok().flatten().is_some_and(|r| !r.state.is_open()) {
                    super::reap(&inner, &id);
                    return;
                }
                drop(inner);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
    }
}
