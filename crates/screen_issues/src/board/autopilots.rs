//! The autopilots popover (`issue_autopilots()` of the mockups): the project's autopilots with
//! their schedule, target and state, switched on and off from here.
//!
//! Event triggers (Sentry, Jira, webhooks), creating an autopilot and the run history are not
//! in the engine yet: they are drawn as "Coming soon".

use gpui::{App, Context, Entity, IntoElement, Render, SharedString, Subscription, Window, div, prelude::*, px};
use hub::OpsModel;
use hub::api::{Autopilot, AutopilotTrigger, Millis, Schedule};
use ui::dev::Probe;
use ui::prelude::*;

/// Probe and anchor of the header button.
pub(crate) const BUTTON: &str = "issues-autopilots";

/// "Every day at 02:00", "Mondays at 09:00", "Every 15 min".
pub(crate) fn schedule_label(schedule: &Schedule) -> String {
    match *schedule {
        Schedule::Day { hour, minute } => format!("Every day at {hour:02}:{minute:02}"),
        Schedule::Week { weekday, hour, minute } => {
            format!("{}s at {hour:02}:{minute:02}", base::time::weekday_name(weekday))
        }
        Schedule::Minutes { minutes } => format!("Every {minutes} min"),
    }
}

/// When an autopilot fires, in words.
pub(crate) fn trigger_label(trigger: &AutopilotTrigger) -> String {
    match trigger {
        AutopilotTrigger::Schedule { schedule } => schedule_label(schedule),
        AutopilotTrigger::Event { source } => {
            let what = source.split_once(": ").map_or(source.as_str(), |(_, what)| what);
            format!("On {what}")
        }
    }
}

/// The third line of a row: why it is paused, or when it runs next and what it did last.
/// The bool says the line is a warning.
pub(crate) fn status_line(autopilot: &Autopilot, now: Millis) -> (String, bool) {
    if let Some(reason) = &autopilot.paused_reason {
        return (format!("Paused · {reason}"), true);
    }
    if !autopilot.enabled {
        return ("Paused".to_string(), false);
    }
    let mut parts = Vec::new();
    if let Some(next) = autopilot.next_run {
        let secs = u64::try_from((next - now).max(0) / 1000).unwrap_or(0);
        parts.push(format!("Next run in {}", base::fmt::duration_words(secs)));
    }
    if !autopilot.last_result.is_empty() {
        if parts.is_empty() {
            parts.push(autopilot.last_result.clone());
        } else {
            parts.push(format!("last run {}", autopilot.last_result));
        }
    }
    if let (AutopilotTrigger::Event { .. }, Some(budget)) = (&autopilot.trigger, autopilot.budget_usd) {
        parts.push(format!("≤{} each", base::fmt::cost(budget)));
    }
    (parts.join(" · "), false)
}

/// The popover's content; it follows the ops model while open.
pub(crate) struct AutopilotsPopover {
    ops: Entity<OpsModel>,
    view: gpui::WeakEntity<super::IssuesView>,
    _sub: Subscription,
}

impl AutopilotsPopover {
    /// The popover of the current project's autopilots.
    pub(crate) fn new(view: gpui::WeakEntity<super::IssuesView>, cx: &mut Context<Self>) -> Self {
        let ops = hub::ops(cx);
        ops.update(cx, |ops, cx| ops.ensure_autopilots(cx));
        let sub = cx.observe(&ops, |_, _, cx| cx.notify());
        Self { ops, view, _sub: sub }
    }

    fn row(&self, ix: usize, autopilot: &Autopilot, now: Millis, cx: &App) -> impl IntoElement + use<> {
        let event = matches!(autopilot.trigger, AutopilotTrigger::Event { .. });
        let target = match self.view.upgrade() {
            Some(view) => view.read(cx).runner_name(&autopilot.target, cx),
            None => SharedString::from(autopilot.target.id().to_string()),
        };
        let (line, warn) = status_line(autopilot, now);
        let tile = Tile::new(if event { IconName::Zap } else { IconName::Clock })
            .tone(if event { TileTone::Accent } else { TileTone::Neutral });
        let saved = autopilot.clone();
        let toggle = Toggle::new(("issues-autopilot-toggle", ix), autopilot.enabled)
            .disabled(event)
            .on_change(move |on, _, cx| {
                let mut autopilot = saved.clone();
                autopilot.enabled = on;
                if on {
                    autopilot.paused_reason = None;
                }
                hub::commands(cx).save_autopilot(autopilot, cx);
            });
        row()
            .id(("issues-autopilot", ix))
            .debug_selector(move || format!("issues-autopilot-{ix}"))
            .items_start()
            .gap(px(SPACE_2_5))
            .p(px(SPACE_2_5))
            .rounded(px(RADIUS_MD))
            .child(tile)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        row()
                            .gap(px(SPACE_2))
                            .child(truncate(autopilot.name.clone()).font_weight(gpui::FontWeight::MEDIUM))
                            .when(event, |d| d.child(Tag::new("Coming soon"))),
                    )
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_SECONDARY)
                            .child(format!("{} → {target}", trigger_label(&autopilot.trigger))),
                    )
                    .when(!line.is_empty(), |d| {
                        d.child(div().type_style(LABEL).text_color(if warn { WARNING } else { TEXT_MUTED }).child(line))
                    }),
            )
            .child(div().flex_none().mt(px(SPACE_0_5)).child(toggle))
    }
}

impl Render for AutopilotsPopover {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = base::time::now_ms();
        let loaded = self.ops.read(cx).autopilots();
        let list: Vec<Autopilot> = loaded.value().map(|list| list.as_ref().clone()).unwrap_or_default();
        let on = list.iter().filter(|a| a.enabled).count();
        let paused = list.len() - on;
        let mut pop = Popover::new()
            .width(420.)
            .title("Autopilots")
            .subtitle("Create or pick up issues without you: on a schedule or when something happens.");
        if !list.is_empty() {
            pop = pop.trailing(format!("{on} on · {paused} paused"));
        }
        let body: Vec<gpui::AnyElement> = match loaded.state() {
            hub::LoadState::Loading if list.is_empty() => vec![note("Loading autopilots…").into_any_element()],
            hub::LoadState::Failed(error) => vec![note(error.clone()).text_color(DANGER).into_any_element()],
            _ if list.is_empty() => vec![note("No autopilot in this project yet.").into_any_element()],
            _ => list.iter().enumerate().map(|(ix, a)| self.row(ix, a, now, cx).into_any_element()).collect(),
        };
        div()
            .child(
                pop.children(body)
                    .child(PopSeparator)
                    .child(MenuItem::new("issues-autopilot-new", "New autopilot…").icon(IconName::Plus).coming_soon())
                    .child(MenuItem::new("issues-autopilot-history", "Run history").icon(IconName::Clock).coming_soon()),
            )
            .probe("issues-autopilots-popover")
    }
}

fn note(text: impl Into<SharedString>) -> gpui::Div {
    div().px(px(SPACE_2_5)).py(px(SPACE_2)).type_style(LABEL).text_color(TEXT_MUTED).child(text.into())
}
