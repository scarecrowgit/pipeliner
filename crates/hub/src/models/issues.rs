//! Issues of the current project, keyed by issue key, with activity feeds loaded on demand.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use gpui::Context;

use crate::api::{Api, Issue, IssueEvent, IssueKey, IssueStatus, ProjectId};
use crate::load::{LoadState, Loadable, failed, fetch, query};

pub(crate) enum IssueInput {
    Changed(Issue),
    Removed(IssueKey),
    Activity(IssueKey, IssueEvent),
    /// Settings changed: the current project may be another one.
    Settings,
}

/// The board's data. Updated by `IssueChanged`, `IssueRemoved` and `IssueActivity`.
pub struct IssuesModel {
    api: Arc<dyn Api>,
    project: Option<ProjectId>,
    /// The project follows the settings until a view picks one.
    follow_settings: bool,
    state: LoadState,
    started: bool,
    issues: HashMap<IssueKey, Arc<Issue>>,
    activity: HashMap<IssueKey, Loadable<Vec<IssueEvent>>>,
    buffer: Vec<IssueInput>,
    generation: u64,
}

impl IssuesModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            project: None,
            follow_settings: true,
            state: LoadState::Loading,
            started: false,
            issues: HashMap::new(),
            activity: HashMap::new(),
            buffer: Vec::new(),
            generation: 0,
        }
    }

    /// The project shown; `None` until loaded.
    pub fn project(&self) -> Option<&ProjectId> {
        self.project.as_ref()
    }

    /// Whether the issues of the project are loaded.
    pub fn state(&self) -> &LoadState {
        &self.state
    }

    /// One issue by key.
    pub fn issue(&self, key: &IssueKey) -> Option<&Arc<Issue>> {
        self.issues.get(key)
    }

    /// How many issues the project has.
    pub fn len(&self) -> usize {
        self.issues.len()
    }

    /// The project has no issue.
    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    /// Every issue of the project, in no order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Issue>> {
        self.issues.values()
    }

    /// Issues in one board column: higher priority first, then most recently changed.
    pub fn column(&self, status: IssueStatus) -> Vec<Arc<Issue>> {
        let mut column: Vec<Arc<Issue>> = self.issues.values().filter(|issue| issue.status == status).cloned().collect();
        column.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then_with(|| b.updated.cmp(&a.updated)).then_with(|| a.key.cmp(&b.key))
        });
        column
    }

    /// Issues not done ("8 open").
    pub fn open_count(&self) -> usize {
        self.issues.values().filter(|issue| issue.status != IssueStatus::Done).count()
    }

    /// The activity feed of an issue, once [`Self::ensure_activity`] asked for it.
    pub fn activity(&self, key: &IssueKey) -> Option<&Loadable<Vec<IssueEvent>>> {
        self.activity.get(key)
    }

    /// Reads the current project's issues, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.started {
            self.reload(cx);
        }
    }

    /// Shows another project's issues.
    pub fn set_project(&mut self, project: ProjectId, cx: &mut Context<Self>) {
        self.follow_settings = false;
        if self.started && self.project.as_ref() == Some(&project) {
            return;
        }
        self.project = Some(project);
        self.issues.clear();
        self.activity.clear();
        self.reload(cx);
    }

    /// Loads the activity feed of `key` if it is not loaded.
    pub fn ensure_activity(&mut self, key: &IssueKey, cx: &mut Context<Self>) {
        if self.activity.get(key).is_some_and(Loadable::started) {
            return;
        }
        let slot_key = key.clone();
        let query_key = key.clone();
        let api = self.api.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.activity.entry(slot_key.clone()).or_default(),
            move |api| api.issue_events(&query_key),
        );
    }

    /// Drops the entry `seq` from a loaded feed (a deleted comment).
    pub(crate) fn forget_event(&mut self, key: &IssueKey, seq: u64, cx: &mut Context<Self>) {
        if let Some(feed) = self.activity.get_mut(key)
            && feed.edit(|events| events.retain(|event| event.seq != seq))
        {
            cx.notify();
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.started = true;
        self.state = LoadState::Loading;
        self.generation += 1;
        let generation = self.generation;
        let wanted = if self.follow_settings { None } else { self.project.clone() };
        query(
            cx,
            &self.api,
            move |api| {
                let project = match wanted {
                    Some(project) => project,
                    None => current_project(api)?,
                };
                let issues = api.issues(&project)?;
                Ok((project, issues))
            },
            move |this: &mut Self, result, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok((project, issues)) => {
                        if this.project.as_ref() != Some(&project) {
                            this.activity.clear();
                        }
                        this.project = Some(project);
                        this.issues = issues.into_iter().map(|issue| (issue.key.clone(), Arc::new(issue))).collect();
                        this.state = LoadState::Ready;
                        let buffered = std::mem::take(&mut this.buffer);
                        this.apply_inputs(buffered, cx);
                    }
                    Err(error) => this.state = failed(&error),
                }
                cx.notify();
            },
        );
    }

    pub(crate) fn apply(&mut self, inputs: Vec<IssueInput>, cx: &mut Context<Self>) {
        if !self.started {
            return;
        }
        if self.state == LoadState::Loading {
            self.buffer.extend(inputs);
            return;
        }
        if self.apply_inputs(inputs, cx) {
            cx.notify();
        }
    }

    fn apply_inputs(&mut self, inputs: Vec<IssueInput>, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        for input in inputs {
            match input {
                IssueInput::Changed(issue) => {
                    if self.project.as_ref() == Some(&issue.project) {
                        self.issues.insert(issue.key.clone(), Arc::new(issue));
                    } else {
                        self.issues.remove(&issue.key);
                    }
                    changed = true;
                }
                IssueInput::Removed(key) => {
                    changed |= self.issues.remove(&key).is_some();
                    self.activity.remove(&key);
                }
                IssueInput::Activity(key, event) => {
                    if let Some(feed) = self.activity.get_mut(&key) {
                        changed |= feed.edit(|events| {
                            if events.last().is_none_or(|last| last.seq < event.seq) {
                                events.push(event);
                            }
                        });
                    }
                }
                IssueInput::Settings => {
                    if self.follow_settings {
                        self.recheck_project(cx);
                    }
                }
            }
        }
        changed
    }

    /// Reloads when the settings now name another current project.
    fn recheck_project(&mut self, cx: &mut Context<Self>) {
        let shown = self.project.clone();
        query(cx, &self.api, current_project, move |this: &mut Self, result, cx| {
            if let Ok(project) = result
                && Some(&project) != shown.as_ref()
                && this.follow_settings
            {
                this.issues.clear();
                this.activity.clear();
                this.reload(cx);
            }
        });
    }
}

/// The project the settings name, else the first one.
pub(crate) fn current_project(api: &dyn Api) -> Result<ProjectId> {
    if let Some(project) = api.settings()?.current_project {
        return Ok(project);
    }
    api.projects()?.into_iter().next().map(|project| project.id).ok_or_else(|| anyhow!("There is no project yet"))
}
