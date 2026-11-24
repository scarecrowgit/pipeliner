//! Home and the app frame: stats per period, needs-you and working-now rows, spend, runtimes,
//! settings, projects and autopilots.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Task};

use crate::api::{
    Api, Autopilot, HomeStats, Need, Period, Project, Runtime, Settings, SpendBucket, SpendByRunnable, SpendToday,
    Working,
};
use crate::load::{Loadable, fetch, query};
use crate::models::issues::current_project;

/// Rows of "Spend by agent" before the "Others" row.
pub const TOP_SPENDERS: usize = 6;
/// How often time-relative numbers (ages, today's totals) are read again.
const CLOCK: Duration = Duration::from_secs(60);

/// Everything Home, the sidebar foot and the settings read. Each part loads on first use and is
/// re-read when an event says it changed.
pub struct OpsModel {
    api: Arc<dyn Api>,
    stats: HashMap<Period, Loadable<HomeStats>>,
    needs: Loadable<Vec<Need>>,
    working: Loadable<Vec<Working>>,
    spend_today: Loadable<SpendToday>,
    buckets: HashMap<Period, Loadable<Vec<SpendBucket>>>,
    by_runnable: HashMap<Period, Loadable<Vec<SpendByRunnable>>>,
    runtimes: Loadable<Vec<Runtime>>,
    settings: Loadable<Settings>,
    projects: Loadable<Vec<Project>>,
    autopilots: Loadable<Vec<Autopilot>>,
    clock: Option<Task<()>>,
}

impl OpsModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            stats: HashMap::new(),
            needs: Loadable::default(),
            working: Loadable::default(),
            spend_today: Loadable::default(),
            buckets: HashMap::new(),
            by_runnable: HashMap::new(),
            runtimes: Loadable::default(),
            settings: Loadable::default(),
            projects: Loadable::default(),
            autopilots: Loadable::default(),
            clock: None,
        }
    }

    /// The stat strip for a period, once [`Self::ensure_period`] asked for it.
    pub fn stats(&self, period: Period) -> Option<&Loadable<HomeStats>> {
        self.stats.get(&period)
    }

    /// The "Needs you" rows of Home.
    pub fn needs(&self) -> &Loadable<Vec<Need>> {
        &self.needs
    }

    /// The "Working now" rows of Home.
    pub fn working(&self) -> &Loadable<Vec<Working>> {
        &self.working
    }

    /// Today's spend for the sidebar foot.
    pub fn spend_today(&self) -> &Loadable<SpendToday> {
        &self.spend_today
    }

    /// Bars of the spend chart for a period, once loaded.
    pub fn spend_buckets(&self, period: Period) -> Option<&Loadable<Vec<SpendBucket>>> {
        self.buckets.get(&period)
    }

    /// The biggest spenders of a period, once loaded.
    pub fn spend_by_runnable(&self, period: Period) -> Option<&Loadable<Vec<SpendByRunnable>>> {
        self.by_runnable.get(&period)
    }

    /// The runtimes, kept current by `RuntimeChanged`.
    pub fn runtimes(&self) -> &Loadable<Vec<Runtime>> {
        &self.runtimes
    }

    /// The app settings.
    pub fn settings(&self) -> &Loadable<Settings> {
        &self.settings
    }

    /// Every project.
    pub fn projects(&self) -> &Loadable<Vec<Project>> {
        &self.projects
    }

    /// The project the settings name, else the first one.
    pub fn current_project(&self) -> Option<&Project> {
        let projects = self.projects.value()?;
        let wanted = self.settings.value().and_then(|settings| settings.current_project.as_ref());
        wanted.and_then(|id| projects.iter().find(|project| &project.id == id)).or_else(|| projects.first())
    }

    /// Autopilots of the current project, once [`Self::ensure_autopilots`] asked for them.
    pub fn autopilots(&self) -> &Loadable<Vec<Autopilot>> {
        &self.autopilots
    }

    /// Loads what the app frame and Home need, and starts the clock.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.settings.started() {
            return;
        }
        self.load_settings(cx);
        self.load_live(false, cx);
        self.load_spend_today(cx);
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.runtimes, |api| Ok(api.runtimes()));
        self.ensure_period(Period::Today, cx);
        self.clock = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CLOCK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// Loads the stats and spend charts of a period.
    pub fn ensure_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if !self.stats.get(&period).is_some_and(Loadable::started) {
            self.load_period(period, cx);
        }
    }

    /// Reads again what Home shows, whatever state it is in (after a failed load, for a
    /// "Try again"): needs-you and working-now, today's spend, and the stats and charts of
    /// `period`. Settings and projects too, unless they are loaded. `ensure_*` never retry a
    /// failed load; this does.
    pub fn reload(&mut self, period: Period, cx: &mut Context<Self>) {
        if !self.settings.started() {
            self.ensure_loaded(cx);
            self.ensure_period(period, cx);
            return;
        }
        if !self.settings.is_ready() || !self.projects.is_ready() {
            self.load_settings(cx);
        }
        self.load_live(false, cx);
        self.load_spend_today(cx);
        self.load_period(period, cx);
    }

    /// Reads the current project's autopilots unless they are loaded or loading.
    pub fn ensure_autopilots(&mut self, cx: &mut Context<Self>) {
        if !self.autopilots.started() {
            self.load_autopilots(cx);
        }
    }

    fn load_settings(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.settings, |api| api.settings());
        fetch(self, cx, &api, |this: &mut Self| &mut this.projects, |api| api.projects());
    }

    /// Needs-you and working-now (they change with sessions, runs and the inbox), plus the
    /// stats of every loaded period when `with_stats`: read together, applied with one notify.
    fn load_live(&mut self, with_stats: bool, cx: &mut Context<Self>) {
        let needs = self.needs.begin();
        let working = self.working.begin();
        let periods: Vec<(Period, u64)> = if with_stats {
            let loaded = self.loaded_periods();
            loaded.into_iter().map(|period| (period, self.stats.entry(period).or_default().begin())).collect()
        } else {
            Vec::new()
        };
        query(
            cx,
            &self.api,
            move |api| {
                let stats: Vec<_> =
                    periods.into_iter().map(|(period, generation)| (period, generation, api.home_stats(period))).collect();
                Ok((api.needs_you(), api.working_now(), stats))
            },
            move |this: &mut Self, result, cx| {
                let Ok((needs_result, working_result, stats)) = result else { return };
                let mut changed = this.needs.finish(needs, needs_result);
                changed |= this.working.finish(working, working_result);
                for (period, generation, result) in stats {
                    changed |= this.stats.entry(period).or_default().finish(generation, result);
                }
                if changed {
                    cx.notify();
                }
            },
        );
    }

    fn load_spend_today(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.spend_today, |api| api.spend_today());
    }

    fn load_stats(&mut self, period: Period, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.stats.entry(period).or_default(),
            move |api| api.home_stats(period),
        );
    }

    fn load_period(&mut self, period: Period, cx: &mut Context<Self>) {
        self.load_stats(period, cx);
        let api = self.api.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.buckets.entry(period).or_default(),
            move |api| api.spend_buckets(period),
        );
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.by_runnable.entry(period).or_default(),
            move |api| api.spend_by_runnable(period, TOP_SPENDERS),
        );
    }

    fn load_autopilots(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.autopilots, |api| {
            let project = current_project(api)?;
            api.autopilots(&project)
        });
    }

    fn loaded_periods(&self) -> Vec<Period> {
        self.stats.iter().filter(|(_, slot)| slot.started()).map(|(period, _)| *period).collect()
    }

    /// Something a person may need to act on changed (a session, a run, the inbox).
    pub(crate) fn live_changed(&mut self, cx: &mut Context<Self>) {
        if !self.settings.started() {
            return;
        }
        self.load_live(true, cx);
    }

    pub(crate) fn spend_changed(&mut self, cx: &mut Context<Self>) {
        if !self.settings.started() {
            return;
        }
        self.load_spend_today(cx);
        for period in self.loaded_periods() {
            self.load_period(period, cx);
        }
    }

    pub(crate) fn settings_changed(&mut self, cx: &mut Context<Self>) {
        if !self.settings.started() {
            return;
        }
        self.load_settings(cx);
        self.load_spend_today(cx);
        for period in self.loaded_periods() {
            self.load_stats(period, cx);
        }
        if self.autopilots.started() {
            self.load_autopilots(cx);
        }
    }

    pub(crate) fn autopilots_changed(&mut self, cx: &mut Context<Self>) {
        if self.autopilots.started() {
            self.load_autopilots(cx);
        }
    }

    pub(crate) fn runtime_changed(&mut self, runtimes: Vec<Runtime>, cx: &mut Context<Self>) {
        let changed = self.runtimes.edit(|known| {
            for runtime in runtimes {
                match known.iter_mut().find(|r| r.name == runtime.name) {
                    Some(slot) => *slot = runtime,
                    None => known.push(runtime),
                }
            }
        });
        if changed {
            cx.notify();
        }
    }

    /// The slow clock: ages and today's totals move with time alone.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.load_live(true, cx);
        self.load_spend_today(cx);
    }
}
