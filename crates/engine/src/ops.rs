//! [`OpsApi`]: Home's numbers from the ledger and the live registry, runtimes, settings,
//! projects and permission rules.

use anyhow::{Result, anyhow, bail};
use model::{
    Event, InboxKind, PermissionRule, Project, ProjectId, Runtime, RuntimeKind, Settings, SessionState, SpendBucket,
    SpendByRunnable, SpendToday, ToolDisplay,
};
use store::SessionFilter;

use crate::api::{HomeStats, Need, NeedKind, NewProject, OpsApi, Period, SweptWorktree, Target, Working, WorkingState};
use crate::bus::lock;
use crate::catalog::{days, period_start};
use crate::engine::{Engine, Inner};

impl Inner {
    fn host_name() -> String {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "This machine".to_string())
    }

    fn runnable_name(&self, reference: &model::RunnableRef, fallback: &str) -> String {
        self.catalog().get(reference).map_or_else(|| fallback.to_string(), |r| r.name.clone())
    }
}

/// A permission request in a few words: "asks to run bun test".
fn ask_text(display: &ToolDisplay, tool: &str) -> String {
    let what = match display {
        ToolDisplay::Command { command, .. } => format!("run {}", base::fmt::truncate_chars(command, 60)),
        ToolDisplay::Line { verb, target, .. } => format!("{verb} {target}"),
        ToolDisplay::Diff(diff) => format!("edit {}", diff.path),
        ToolDisplay::Text { title, .. } => title.clone(),
    };
    let what = if what.trim().is_empty() { format!("use {tool}") } else { what };
    format!("asks to {what}")
}

impl Engine {
    /// Everything that waits for a person's decision, oldest first: the one rule behind
    /// Home's "Waiting for you" tile, its "Needs you" list and the Inbox badge (which counts
    /// the entries that target the Inbox).
    ///
    /// - a tool call waiting for permission, in a chat;
    /// - a chat of the person's whose agent asked something (sessions pipeline nodes or
    ///   sub-agents run are covered by their Inbox items);
    /// - every Inbox item that [`InboxItem::needs_person`](model::InboxItem::needs_person):
    ///   open reviews, agents' questions in runs, budget stops and failed runs not yet
    ///   archived, and reports whose outcome asks for a decision. Reading an item changes
    ///   none of this.
    pub(crate) fn waiting_for_person(&self) -> Result<Vec<Need>> {
        let inner = &self.inner;
        let mut out = Vec::new();
        for request in inner.asks.all() {
            let source = inner.store.session(&request.session)?.map(|s| s.title).unwrap_or_default();
            out.push(Need {
                kind: NeedKind::Permission,
                title: format!("{} {}", request.agent, ask_text(&request.display, &request.tool)),
                source,
                since: request.at,
                target: Target::Session(request.session.clone()),
            });
        }
        let waiting = inner.store.sessions(&SessionFilter {
            state: Some(SessionState::Waiting),
            archived: Some(false),
            started_by: Some(store::StartedBy::Me),
            ..Default::default()
        })?;
        for session in waiting.into_iter().filter(|s| !inner.live.is_running(&s.id)) {
            out.push(Need {
                kind: NeedKind::Question,
                title: format!("{} asks you something", inner.runnable_name(&session.runnable, "The agent")),
                source: session.title.clone(),
                since: session.updated,
                target: Target::Session(session.id),
            });
        }
        for item in inner.store.inbox(false)?.into_iter().filter(|i| i.needs_person()) {
            let kind = match item.kind {
                InboxKind::Review { .. } | InboxKind::Report => NeedKind::Review,
                InboxKind::Question { .. } => NeedKind::Question,
                InboxKind::Failure => NeedKind::Failure,
            };
            out.push(Need { kind, title: item.title, source: item.source, since: item.created, target: Target::Inbox(item.id) });
        }
        out.sort_by_key(|n| n.since);
        Ok(out)
    }

    /// Stores the settings and tells everyone: waiting turns see a new slot count at once.
    fn save_settings(&self, settings: Settings) -> Result<()> {
        if settings.slots == 0 {
            bail!("At least one agent must be able to work");
        }
        self.inner.store.set_settings(&settings)?;
        self.inner.slots.notify();
        self.inner.publish(Event::SettingsChanged);
        if let Some(runtime) = self.runtimes().into_iter().next() {
            self.inner.publish(Event::RuntimeChanged { runtime });
        }
        Ok(())
    }

    /// Reads, changes and stores the settings as one step.
    fn change_settings(&self, change: impl FnOnce(&mut Settings) -> Result<()>) -> Result<()> {
        let _guard = lock(&self.inner.settings_lock);
        let mut settings = self.inner.store.settings()?;
        change(&mut settings)?;
        self.save_settings(settings)
    }
}

impl OpsApi for Engine {
    fn home_stats(&self, period: Period) -> Result<HomeStats> {
        let inner = &self.inner;
        let now = inner.now();
        let from = period_start(now, period);
        let settings = inner.store.settings()?;
        let needs = self.waiting_for_person()?;
        let runs: u32 = inner.store.runs_by_runnable(from)?.iter().map(|(_, n)| n).sum();
        let sessions = inner.store.sessions(&SessionFilter { parent: store::ParentFilter::TopLevel, ..Default::default() })?;
        let recent: Vec<_> = sessions.iter().filter(|s| s.updated >= from && s.message_count > 0).collect();
        let finished = recent.iter().filter(|s| !matches!(s.state, SessionState::Failed)).count();
        let pipeline_runs: Vec<_> = inner.store.latest_runs()?.into_iter().filter(|r| r.started >= from).collect();
        let shipped = recent.iter().filter(|s| s.pr_url.is_some()).count()
            + pipeline_runs.iter().filter(|r| r.pr_url.is_some()).count();
        let total = recent.len() + pipeline_runs.len();
        let done = finished + pipeline_runs.iter().filter(|r| r.state == model::RunState::Done).count();
        Ok(HomeStats {
            working_now: inner.live.running().len() as u32,
            slots: settings.slots,
            waiting: needs.len() as u32,
            oldest_waiting: needs.iter().map(|n| n.since).min(),
            runs,
            finished_percent: if total == 0 { 0 } else { base::fmt::percent(done as f64, total as f64) },
            spend_usd: inner.store.spend_between(from, now + 1)?.cost_usd,
            limit_usd: settings.daily_limit_usd.map(|l| l * days(period) as f64),
            shipped: shipped as u32,
        })
    }

    fn needs_you(&self) -> Result<Vec<Need>> {
        self.waiting_for_person()
    }

    fn working_now(&self) -> Result<Vec<Working>> {
        let inner = &self.inner;
        let runtime = Inner::host_name();
        let mut out = Vec::new();
        for id in inner.live.running() {
            let Some(session) = inner.store.session(&id)? else { continue };
            let name = inner.runnable_name(&session.runnable, &session.title);
            let pending = inner.asks.of(&id);
            let state = match pending.first() {
                Some(request) => WorkingState::Waiting { text: ask_text(&request.display, &request.tool) },
                None => WorkingState::Running { percent: None },
            };
            let key = session
                .issue
                .as_ref()
                .map(ToString::to_string)
                .or_else(|| session.workdir.as_ref().and_then(|w| w.branch.clone()))
                .unwrap_or_default();
            out.push(Working {
                name,
                kind: "Agent · chat".to_string(),
                runnable_kind: model::RunnableKind::Agent,
                spawned: false,
                task: session.title.clone(),
                key,
                runtime: runtime.clone(),
                since: session.updated,
                cost_usd: session.cost_usd,
                state,
                target: Target::Session(session.id),
            });
        }
        out.sort_by_key(|w| w.since);
        Ok(out)
    }

    fn spend_today(&self) -> Result<SpendToday> {
        let inner = &self.inner;
        let now = inner.now();
        let from = period_start(now, Period::Today);
        Ok(SpendToday {
            cost_usd: inner.store.spend_between(from, now + 1)?.cost_usd,
            limit_usd: inner.store.settings()?.daily_limit_usd,
        })
    }

    fn spend_buckets(&self, period: Period) -> Result<Vec<SpendBucket>> {
        let inner = &self.inner;
        let now = inner.now();
        let from = period_start(now, period);
        let offset = base::time::utc_offset(now);
        match period {
            Period::Days90 => inner.store.spend_by_week(from, now + 1, offset),
            _ => inner.store.spend_by_day(from, now + 1, offset),
        }
    }

    fn spend_by_runnable(&self, period: Period, top: usize) -> Result<Vec<SpendByRunnable>> {
        let now = self.inner.now();
        self.inner.store.spend_by_runnable(period_start(now, period), now + 1, top)
    }

    fn runtimes(&self) -> Vec<Runtime> {
        let inner = &self.inner;
        let slots = inner.store.settings().map_or(Settings::default().slots, |s| s.slots);
        let busy = inner.slots.busy();
        let soon = |name: &str, kind| Runtime {
            name: name.to_string(),
            kind,
            available: false,
            online: false,
            slots_total: 0,
            slots_busy: 0,
            note: "Coming soon".to_string(),
        };
        vec![
            Runtime {
                name: Inner::host_name(),
                kind: RuntimeKind::Local,
                available: true,
                online: true,
                slots_total: slots,
                slots_busy: busy,
                note: if busy == 0 { "idle".to_string() } else { format!("{busy} working") },
            },
            soon("Cloud", RuntimeKind::Cloud),
            soon("Remote", RuntimeKind::Remote),
        ]
    }

    fn settings(&self) -> Result<Settings> {
        self.inner.store.settings()
    }

    fn set_settings(&self, settings: Settings) -> Result<()> {
        let _guard = lock(&self.inner.settings_lock);
        self.save_settings(settings)
    }

    fn projects(&self) -> Result<Vec<Project>> {
        self.inner.store.projects()
    }

    fn save_project(&self, mut project: Project) -> Result<()> {
        if project.name.trim().is_empty() {
            bail!("A project needs a name");
        }
        if project.key_prefix.trim().is_empty() {
            project.key_prefix = issues::keys::prefix_for(&project.name);
        }
        project.key_prefix = project.key_prefix.trim().to_ascii_uppercase();
        if project.id.as_str().is_empty() {
            project.id = ProjectId::new(base::id::slug(&project.name, 40));
        }
        self.inner.store.upsert_project(&project)?;
        self.inner.publish(Event::SettingsChanged);
        Ok(())
    }

    fn delete_project(&self, id: &ProjectId) -> Result<()> {
        if !self.inner.store.delete_project(id)? {
            bail!("There is no project {id}");
        }
        // The current project falls back to the first one left.
        self.change_settings(|settings| {
            if settings.current_project.as_ref() == Some(id) {
                settings.current_project = None;
            }
            Ok(())
        })?;
        self.inner.publish(Event::SettingsChanged);
        Ok(())
    }

    fn create_project(&self, new: NewProject) -> Result<Project> {
        let inner = &self.inner;
        let name = new.name.trim();
        if name.is_empty() {
            bail!("A project needs a name");
        }
        let existing = inner.store.projects()?;
        let taken = || existing.iter().map(|p| p.key_prefix.as_str());
        let key_prefix = if new.key_prefix.trim().is_empty() {
            // A prefix made from the name gets a letter more when another project has it.
            let base = issues::keys::prefix_for(name);
            std::iter::once(base.clone())
                .chain(('A'..='Z').map(|c| format!("{base}{c}")))
                .find_map(|candidate| issues::keys::validate_prefix(&candidate, taken()).ok())
                .ok_or_else(|| anyhow!("Pick a key prefix for {name}: {base} is taken"))?
        } else {
            issues::keys::validate_prefix(&new.key_prefix, taken())?
        };
        let mut repos = Vec::new();
        for typed in &new.repos {
            let path = inner.paths.expand(&typed.to_string_lossy());
            if !path.is_dir() {
                bail!("The folder {} does not exist", inner.paths.display(&path));
            }
            if !repos.contains(&path) {
                repos.push(path);
            }
        }
        let base = base::id::slug(name, 40);
        let base = if base.is_empty() { key_prefix.to_ascii_lowercase() } else { base };
        let mut id = ProjectId::new(base.clone());
        let mut n = 2;
        while existing.iter().any(|p| p.id == id) {
            id = ProjectId::new(format!("{base}-{n}"));
            n += 1;
        }
        let project = Project { id, name: name.to_string(), key_prefix, repos };
        inner.store.upsert_project(&project)?;
        inner.publish(Event::SettingsChanged);
        Ok(project)
    }

    fn rename_project(&self, id: &ProjectId, name: &str) -> Result<Project> {
        let name = name.trim();
        if name.is_empty() {
            bail!("A project needs a name");
        }
        let mut project = self.inner.store.project(id)?.ok_or_else(|| anyhow!("There is no project {id}"))?;
        project.name = name.to_string();
        self.inner.store.upsert_project(&project)?;
        self.inner.publish(Event::SettingsChanged);
        Ok(project)
    }

    fn set_current_project(&self, id: &ProjectId) -> Result<()> {
        if self.inner.store.project(id)?.is_none() {
            bail!("There is no project {id}");
        }
        self.change_settings(|settings| {
            settings.current_project = Some(id.clone());
            Ok(())
        })
    }

    fn project_counts(&self) -> Result<Vec<crate::api::ProjectCounts>> {
        let store = &self.inner.store;
        let projects = store.projects()?;
        let mut issues = Vec::new();
        for project in &projects {
            issues.extend(store.issues(&project.id)?);
        }
        let mut runs = store.open_runs()?;
        runs.extend(store.latest_runs()?);
        Ok(crate::api::project_counts_of(&projects, &issues, &runs))
    }

    fn set_daily_limit(&self, usd: Option<f64>) -> Result<()> {
        if usd.is_some_and(|usd| !usd.is_finite() || usd < 0.0) {
            bail!("The daily limit cannot be below $0.00");
        }
        self.change_settings(|settings| {
            settings.daily_limit_usd = usd;
            Ok(())
        })
    }

    fn set_slots(&self, slots: u32) -> Result<()> {
        self.change_settings(|settings| {
            settings.slots = slots;
            Ok(())
        })
    }

    fn set_worktree_retention(&self, days: u32) -> Result<()> {
        self.change_settings(|settings| {
            settings.worktree_retention_days = days;
            Ok(())
        })
    }

    fn sweep_worktrees(&self, dry_run: bool) -> Result<Vec<SweptWorktree>> {
        crate::sweep::sweep(&self.inner, dry_run)
    }

    fn permission_rules(&self) -> Result<Vec<PermissionRule>> {
        self.inner.store.all_permission_rules()
    }

    fn remove_permission_rule(&self, rule: &PermissionRule) -> Result<()> {
        if !self.inner.store.remove_permission_rule(rule)? {
            bail!("That rule no longer exists");
        }
        self.inner.publish(Event::SettingsChanged);
        Ok(())
    }

    fn shutdown(&self) {
        self.inner.shutdown();
    }
}
