//! The dialog that creates or edits a schedule autopilot: its name, what it runs (an agent,
//! orchestrator or pipeline of the catalog), the schedule `Schedule` can hold (every day at a
//! time, one day a week at a time, every few minutes or hours), what the target is told, the
//! budget of each run and whether it is on. "Save" sends it with `save_autopilot`; the engine
//! works out the next run. Triggers on events are not in the engine yet and stay
//! "Coming soon".

use gpui::{AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use hub::api::{Autopilot, AutopilotId, AutopilotTrigger, ProjectId, RunnableKind, RunnableRef, Schedule};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{SubmitOn, TextArea, TextInput, TextInputEvent, text_area_field, text_field};

use super::card::runner_kind;

/// The dialog's id.
pub(crate) const DIALOG: &str = "autopilot-dialog";
/// Id, anchor and probe of the "Runs" select.
pub(crate) const TARGET: &str = "autopilot-target";
/// Id and probe of the button that saves.
pub(crate) const SAVE: &str = "autopilot-save";

/// The kinds of schedule the dialog offers, as `Schedule` holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Repeat {
    /// Every day at a time.
    #[default]
    Daily,
    /// One day a week at a time.
    Weekly,
    /// Every few minutes or hours.
    Interval,
}

impl Repeat {
    const ALL: [Repeat; 3] = [Repeat::Daily, Repeat::Weekly, Repeat::Interval];

    fn label(self) -> &'static str {
        match self {
            Repeat::Daily => "Every day",
            Repeat::Weekly => "Every week",
            Repeat::Interval => "Every few hours",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Repeat::Daily => "daily",
            Repeat::Weekly => "weekly",
            Repeat::Interval => "interval",
        }
    }
}

/// The unit of an interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Unit {
    #[default]
    Hours,
    Minutes,
}

/// "02:00", "9:30" or "9" as (hour, minute).
pub(crate) fn parse_time(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let (h, m) = match text.split_once(':') {
        Some((h, m)) => (h.trim(), m.trim()),
        None => (text, "0"),
    };
    let (hour, minute) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (hour < 24 && minute < 60).then_some((hour, minute))
}

/// A positive dollar amount, with or without `$`; `Ok(None)` when the field is empty.
pub(crate) fn parse_budget(text: &str) -> Result<Option<f64>, ()> {
    let text = text.trim().trim_start_matches('$').trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(usd) if usd.is_finite() && usd > 0. => Ok(Some(usd)),
        _ => Err(()),
    }
}

/// The schedule of the form's fields, or what is wrong with them.
pub(crate) fn schedule_of(repeat: Repeat, time: &str, weekday: u32, every: &str, unit: Unit) -> Result<Schedule, SharedString> {
    match repeat {
        Repeat::Daily | Repeat::Weekly => {
            let Some((hour, minute)) = parse_time(time) else {
                return Err("Give the time as HH:MM, like 02:00.".into());
            };
            Ok(match repeat {
                Repeat::Weekly => Schedule::Week { weekday: weekday % 7, hour, minute },
                _ => Schedule::Day { hour, minute },
            })
        }
        Repeat::Interval => {
            let n = every.trim().parse::<u32>().ok().filter(|n| *n > 0);
            let minutes = n.and_then(|n| match unit {
                Unit::Hours => n.checked_mul(60),
                Unit::Minutes => Some(n),
            });
            match minutes {
                Some(minutes) if minutes >= 5 => Ok(Schedule::Minutes { minutes }),
                Some(_) => Err("Run it at most every 5 minutes.".into()),
                None => Err("Say how often, as a whole number.".into()),
            }
        }
    }
}

/// The dialog's state.
pub(crate) struct AutopilotForm {
    project: Option<ProjectId>,
    project_name: String,
    /// The autopilot being edited; `None` for a new one.
    editing: Option<Autopilot>,
    pub(crate) name: Entity<TextInput>,
    pub(crate) prompt: Entity<TextArea>,
    pub(crate) time: Entity<TextInput>,
    pub(crate) every: Entity<TextInput>,
    pub(crate) budget: Entity<TextInput>,
    pub(crate) repeat: Repeat,
    pub(crate) weekday: u32,
    pub(crate) unit: Unit,
    pub(crate) target: Option<RunnableRef>,
    pub(crate) enabled: bool,
    pub(crate) error: Option<SharedString>,
    focus_pending: bool,
    _subs: Vec<Subscription>,
}

impl AutopilotForm {
    /// A form for a new autopilot in `project`, or for `editing` when given.
    pub(crate) fn new(
        project: Option<ProjectId>,
        project_name: String,
        editing: Option<Autopilot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mut repeat, mut weekday, mut unit) = (Repeat::Daily, 0, Unit::Hours);
        let (mut time, mut every) = ("09:00".to_string(), "2".to_string());
        if let Some(AutopilotTrigger::Schedule { schedule }) = editing.as_ref().map(|a| &a.trigger) {
            match *schedule {
                Schedule::Day { hour, minute } => time = format!("{hour:02}:{minute:02}"),
                Schedule::Week { weekday: day, hour, minute } => {
                    repeat = Repeat::Weekly;
                    weekday = day % 7;
                    time = format!("{hour:02}:{minute:02}");
                }
                Schedule::Minutes { minutes } => {
                    repeat = Repeat::Interval;
                    if minutes % 60 == 0 {
                        every = (minutes / 60).to_string();
                    } else {
                        unit = Unit::Minutes;
                        every = minutes.to_string();
                    }
                }
            }
        }
        let a = editing.as_ref();
        let name_text = a.map(|a| a.name.clone()).unwrap_or_default();
        let prompt_text = a.map(|a| a.prompt.clone()).unwrap_or_default();
        let budget_text = a.and_then(|a| a.budget_usd).map(|usd| format!("{usd:.2}")).unwrap_or_default();
        let name = cx.new(|cx| TextInput::new(cx).placeholder("Nightly e2e triage").with_text(&name_text));
        let prompt = cx.new(|cx| {
            TextArea::new(cx)
                .placeholder("Triage last night’s failed e2e runs and file an issue for each new failure.")
                .submit_on(SubmitOn::Never)
                .rows(3, 8)
                .with_text(&prompt_text)
        });
        let time = cx.new(|cx| TextInput::new(cx).placeholder("02:00").with_text(&time));
        let every = cx.new(|cx| TextInput::new(cx).placeholder("2").with_text(&every));
        let budget = cx.new(|cx| TextInput::new(cx).placeholder("No limit").with_text(&budget_text));
        let mut subs = Vec::new();
        for input in [&name, &time, &every, &budget] {
            subs.push(cx.subscribe_in(input, window, |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Changed => {
                    this.error = None;
                    cx.notify();
                }
                TextInputEvent::Submit => this.submit(window, cx),
                _ => {}
            }));
        }
        Self {
            project: editing.as_ref().map(|a| a.project.clone()).or(project),
            project_name,
            target: editing.as_ref().map(|a| a.target.clone()),
            enabled: editing.as_ref().is_none_or(|a| a.enabled),
            editing,
            name,
            prompt,
            time,
            every,
            budget,
            repeat,
            weekday,
            unit,
            error: None,
            focus_pending: true,
            _subs: subs,
        }
    }

    /// The autopilot "Save" would send, or what is missing.
    pub(crate) fn build(&self, cx: &App) -> Result<Autopilot, SharedString> {
        let name = self.name.read(cx).text().trim().to_string();
        if name.is_empty() {
            return Err("Give the autopilot a name.".into());
        }
        let Some(target) = self.target.clone() else {
            return Err("Pick what it runs.".into());
        };
        let schedule = schedule_of(self.repeat, self.time.read(cx).text(), self.weekday, self.every.read(cx).text(), self.unit)?;
        let Ok(budget_usd) = parse_budget(self.budget.read(cx).text()) else {
            return Err("Give the budget in dollars, like 0.50, or leave it empty.".into());
        };
        let Some(project) = self.project.clone() else {
            return Err("There is no project to add it to.".into());
        };
        let prompt = self.prompt.read(cx).text().trim().to_string();
        Ok(match &self.editing {
            Some(old) => Autopilot {
                name,
                trigger: AutopilotTrigger::Schedule { schedule },
                target,
                prompt,
                enabled: self.enabled,
                paused_reason: if self.enabled { None } else { old.paused_reason.clone() },
                budget_usd,
                ..old.clone()
            },
            None => Autopilot {
                id: AutopilotId::new(format!("{}-{}", base::id::slug(&name, 32), base::id::new_id())),
                project,
                name,
                trigger: AutopilotTrigger::Schedule { schedule },
                target,
                prompt,
                enabled: self.enabled,
                paused_reason: None,
                next_run: None,
                last_run: None,
                last_result: String::new(),
                budget_usd,
            },
        })
    }

    /// Saves the autopilot and closes the dialog, or shows what is missing.
    pub(crate) fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.build(cx) {
            Ok(autopilot) => {
                hub::commands(cx).save_autopilot(autopilot, cx);
                overlays(window, cx).close(DIALOG, window, cx);
            }
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn set<T>(&mut self, f: impl FnOnce(&mut Self, T), value: T, cx: &mut Context<Self>) {
        f(self, value);
        self.error = None;
        cx.notify();
    }

    fn open_targets(&self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.entity();
        let anchor = Anchor::of(TARGET, window, cx);
        overlays(window, cx).open_menu(anchor, Placement::below(), move |_, cx| target_menu(&form, cx), window, cx);
    }

    /// The "Runs" select: the chosen runnable with its kind, or a prompt to pick one.
    fn target_select(&self, cx: &mut Context<Self>) -> AnyElement {
        let catalog = hub::catalog(cx);
        let (label, glyph) = match &self.target {
            Some(target) => {
                let (glyph, kind) = runner_kind(target);
                let name = catalog.read(cx).runnable(target).map_or_else(|| target.id().to_string(), |r| r.name.clone());
                (format!("{name} · {}", kind.to_lowercase()), glyph)
            }
            None => ("Pick an agent, orchestrator or pipeline".to_string(), IconName::Agent),
        };
        div()
            .w_full()
            .child(
                SelectBox::new(TARGET, label)
                    .icon(glyph)
                    .placeholder(self.target.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.open_targets(window, cx)))
                    .popover_anchor(TARGET),
            )
            .probe(TARGET)
            .into_any_element()
    }

    /// The schedule's kind and its values.
    fn schedule_fields(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = cx.entity().downgrade();
        let selected = Repeat::ALL.iter().position(|r| *r == self.repeat).unwrap_or(0);
        let kinds = Repeat::ALL
            .iter()
            .fold(Segmented::new("autopilot-repeat").small(), |seg, r| {
                seg.option(r.label()).option_probe(format!("autopilot-repeat-{}", r.slug()))
            })
            .selected(selected)
            .on_select(move |ix, _, cx| {
                form.update(cx, |this, cx| this.set(|t, r| t.repeat = r, Repeat::ALL[ix.min(2)], cx)).ok();
            });
        let label = |text: &'static str| div().flex_none().type_style(LABEL).text_color(TEXT_SECONDARY).child(text);
        let time = || {
            div().w(px(88.)).flex_none().child(text_field("autopilot-time-field", &self.time).small().mono()).probe("autopilot-time")
        };
        let values = match self.repeat {
            Repeat::Daily => row().gap(px(SPACE_2)).child(label("at")).child(time()),
            Repeat::Weekly => {
                let form = cx.entity().downgrade();
                let days = (0..7u32)
                    .fold(Segmented::new("autopilot-weekday").small(), |seg, d| {
                        seg.option(base::time::weekday_short(d)).option_probe(format!("autopilot-weekday-{d}"))
                    })
                    .selected(self.weekday as usize)
                    .on_select(move |ix, _, cx| {
                        form.update(cx, |this, cx| this.set(|t, d| t.weekday = d, ix as u32 % 7, cx)).ok();
                    });
                row().gap(px(SPACE_2)).child(label("On")).child(days).child(label("at")).child(time())
            }
            Repeat::Interval => {
                let form = cx.entity().downgrade();
                let units = Segmented::new("autopilot-unit")
                    .small()
                    .option("hours")
                    .option_probe("autopilot-unit-hours")
                    .option("minutes")
                    .option_probe("autopilot-unit-minutes")
                    .selected(if self.unit == Unit::Hours { 0 } else { 1 })
                    .on_select(move |ix, _, cx| {
                        let unit = if ix == 0 { Unit::Hours } else { Unit::Minutes };
                        form.update(cx, |this, cx| this.set(|t, u| t.unit = u, unit, cx)).ok();
                    });
                row()
                    .gap(px(SPACE_2))
                    .child(label("Every"))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_none()
                            .child(text_field("autopilot-every-field", &self.every).small().mono())
                            .probe("autopilot-every"),
                    )
                    .child(units)
            }
        };
        col().w_full().gap(px(SPACE_2_5)).child(kinds).child(values).into_any_element()
    }

    /// The dialog; the form is a view of its own in the dialog layer.
    pub(crate) fn dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Dialog {
        if self.focus_pending {
            self.focus_pending = false;
            let name = self.name.clone();
            window.defer(cx, move |window, cx| name.read(cx).focus(window));
        }
        let title = if self.editing.is_some() { "Edit autopilot" } else { "New autopilot" };
        let subtitle = if self.project_name.is_empty() { "No project".to_string() } else { self.project_name.clone() };
        let when = col()
            .w_full()
            .gap(px(SPACE_1_5))
            .child(
                OptionCard::new("autopilot-on-schedule", "On a schedule")
                    .description("Runs at the times you set here")
                    .selected(true)
                    .probe_name("autopilot-on-schedule"),
            )
            .child(
                div()
                    .id("autopilot-on-event-wrap")
                    .opacity(0.6)
                    .child(
                        OptionCard::new("autopilot-on-event", "When something happens")
                            .description("A new Sentry error, a Jira issue, a webhook")
                            .right(Tag::new("Coming soon"))
                            .probe_name("autopilot-on-event"),
                    )
                    .tooltip(Tooltip::text("Event triggers are coming soon")),
            );
        let form = cx.entity().downgrade();
        let toggle = Toggle::new("autopilot-enabled", self.enabled).on_change(move |on, _, cx| {
            form.update(cx, |this, cx| this.set(|t, on| t.enabled = on, on, cx)).ok();
        });
        let note = row()
            .gap(px(SPACE_2))
            .child(div().child(toggle).probe("autopilot-enabled"))
            .child(div().type_style(LABEL).text_color(TEXT_SECONDARY).child("On"));
        let save_label = if self.editing.is_some() { "Save" } else { "Create autopilot" };
        let save = Button::new(SAVE, save_label)
            .primary()
            .kbd("⌘↵")
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        let confirm = cx.entity().downgrade();
        let error = self.error.clone();
        Dialog::new(DIALOG, title)
            .width(560.)
            .subtitle(SharedString::from(subtitle))
            .child(
                col()
                    .w_full()
                    .gap(px(SPACE_1_5))
                    .child(Field::new("Name").child(div().child(text_field("autopilot-name-field", &self.name)).probe("autopilot-name")))
                    .when_some(error, |d, error| {
                        d.child(
                            div()
                                .debug_selector(|| "autopilot-error".into())
                                .type_style(LABEL)
                                .text_color(DANGER)
                                .child(error)
                                .probe("autopilot-error"),
                        )
                    }),
            )
            .child(Field::new("Starts").child(when))
            .child(Field::new("Schedule").child(self.schedule_fields(cx)))
            .child(Field::new("Runs").child(self.target_select(cx)))
            .child(
                Field::new("What it’s told")
                    .hint("Each run starts on this text, as if you had typed it.")
                    .child(div().child(text_area_field("autopilot-prompt-field", &self.prompt)).probe("autopilot-prompt")),
            )
            .child(
                Field::new("Budget per run").child(
                    row()
                        .gap(px(SPACE_2))
                        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("$"))
                        .child(
                            div()
                                .w(px(112.))
                                .flex_none()
                                .child(text_field("autopilot-budget-field", &self.budget).small().mono())
                                .probe("autopilot-budget"),
                        )
                        .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("The run stops there and asks you in the Inbox.")),
                ),
            )
            .note(note)
            .cancel_button()
            .action(div().child(save).probe(SAVE))
            .on_confirm(move |window, cx| {
                confirm.update(cx, |this, cx| this.submit(window, cx)).ok();
            })
    }
}

impl Render for AutopilotForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.dialog(window, cx)
    }
}

/// Every agent, orchestrator and pipeline an autopilot can run, the chosen one checked.
fn target_menu(form: &Entity<AutopilotForm>, cx: &mut App) -> Menu {
    let runnables: Vec<hub::api::Runnable> = hub::catalog(cx).read(cx).runnables().to_vec();
    let current = form.read(cx).target.clone();
    let mut menu = Menu::new("autopilot-target-menu").width(380.).max_height(420.);
    let groups = [
        (RunnableKind::Pipeline, "Pipelines"),
        (RunnableKind::Agent, "Agents"),
        (RunnableKind::Orchestrator, "Orchestrators"),
    ];
    for (kind, label) in groups {
        let of_kind: Vec<&hub::api::Runnable> = runnables
            .iter()
            .filter(|r| r.kind == kind && !r.archived && !matches!(r.origin, hub::api::Origin::Spawned { .. }))
            .collect();
        if of_kind.is_empty() {
            continue;
        }
        menu = menu.label(label);
        for runnable in of_kind {
            let reference = runnable.reference.clone();
            let (glyph, _) = runner_kind(&reference);
            let f = form.clone();
            let chosen = reference.clone();
            menu = menu.item(
                MenuItem::new(SharedString::from(format!("autopilot-target-{reference}")), runnable.name.clone())
                    .tile(Tile::new(glyph).medium())
                    .subline(runnable.description.clone())
                    .check_column()
                    .checked(current.as_ref() == Some(&reference))
                    .on_click(move |_, _, cx| {
                        f.update(cx, |this, cx| this.set(|t, r| t.target = Some(r), chosen.clone(), cx));
                    }),
            );
        }
    }
    menu
}
