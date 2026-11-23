//! Autopilot schedules in local time, and how they read in the popover.

use base::{fmt, time};
use model::{Autopilot, AutopilotTrigger, Schedule};

#[cfg(test)]
mod tests;

const MS_PER_MINUTE: i64 = 60_000;

/// The first moment strictly after `after_ms` at which `schedule` fires, in the time zone given
/// by `utc_offset_secs` (seconds east of UTC). Hours and minutes past their range are clamped,
/// an interval of 0 minutes counts as 1. Daylight saving is not modelled: the offset is fixed.
///
/// `Minutes` counts from `after_ms` itself, so pass the time of the last run (or "now" for a
/// new autopilot).
pub fn next_run(schedule: &Schedule, after_ms: i64, utc_offset_secs: i32) -> i64 {
    let local = time::civil(after_ms, utc_offset_secs);
    let at = |day: u32, hour: u32, minute: u32| {
        time::from_civil(local.year, local.month, day, hour.min(23), minute.min(59), 0, utc_offset_secs)
    };
    match *schedule {
        Schedule::Minutes { minutes } => after_ms.saturating_add(i64::from(minutes.max(1)) * MS_PER_MINUTE),
        Schedule::Day { hour, minute } => {
            let today = at(local.day, hour, minute);
            if today > after_ms { today } else { at(local.day + 1, hour, minute) }
        }
        Schedule::Week { weekday, hour, minute } => {
            let ahead = (weekday % 7 + 7 - local.weekday) % 7;
            let this_week = at(local.day + ahead, hour, minute);
            if this_week > after_ms { this_week } else { at(local.day + ahead + 7, hour, minute) }
        }
    }
}

/// A schedule in words: "Every day at 02:00", "Mondays at 09:00", "Every 15 minutes".
pub fn describe_schedule(schedule: &Schedule) -> String {
    match *schedule {
        Schedule::Day { hour, minute } => format!("Every day at {:02}:{:02}", hour.min(23), minute.min(59)),
        Schedule::Week { weekday, hour, minute } => {
            format!("{}s at {:02}:{:02}", time::weekday_name(weekday), hour.min(23), minute.min(59))
        }
        Schedule::Minutes { minutes } => match minutes.max(1) {
            1 => "Every minute".to_string(),
            60 => "Every hour".to_string(),
            m if m % 60 == 0 => format!("Every {} hours", m / 60),
            m => format!("Every {m} minutes"),
        },
    }
}

/// The trigger and its target in one line: "Every day at 02:00 → PAY triage",
/// "On a new error in payments-api → Reproducer". An event source written `kind:name` shows
/// only the name.
pub fn describe(trigger: &AutopilotTrigger, target_name: &str) -> String {
    let when = match trigger {
        AutopilotTrigger::Schedule { schedule } => describe_schedule(schedule),
        AutopilotTrigger::Event { source } => {
            format!("On a new error in {}", source.rsplit_once(':').map_or(source.as_str(), |(_, name)| name))
        }
    };
    format!("{when} → {target_name}")
}

/// The third line of an autopilot row: "Next run in 5h 40m · last run filed 2 issues",
/// "Paused · GitLab token expires in 3 days".
///
/// An autopilot is paused when the app switched it off (`paused_reason`) or a person did
/// (`enabled` false). An event autopilot shows what it did last and its spending cap.
pub fn status_line(autopilot: &Autopilot, now_ms: i64) -> String {
    if let Some(reason) = autopilot.paused_reason.as_deref().filter(|r| !r.is_empty()) {
        return format!("Paused · {reason}");
    }
    if !autopilot.enabled {
        return "Paused".to_string();
    }
    let last = (!autopilot.last_result.is_empty()).then(|| format!("last run {}", autopilot.last_result));
    match autopilot.trigger {
        AutopilotTrigger::Schedule { .. } => {
            let next = match autopilot.next_run {
                None => "Next run not scheduled".to_string(),
                Some(at) if at <= now_ms => "Next run due now".to_string(),
                Some(at) => format!("Next run in {}", fmt::duration_words(((at - now_ms) / 1000) as u64)),
            };
            join([Some(next), last])
        }
        AutopilotTrigger::Event { .. } => {
            let cap = autopilot.budget_usd.map(|usd| format!("≤{} each", fmt::cost(usd)));
            let line = join([Some(autopilot.last_result.clone()).filter(|r| !r.is_empty()), cap]);
            if line.is_empty() { "Waiting for events".to_string() } else { line }
        }
    }
}

fn join(parts: [Option<String>; 2]) -> String {
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// The autopilots to fire now: enabled, not paused by the app, schedule-triggered, with a
/// stored `next_run` that has been reached. Event triggers are never due (not available yet),
/// and neither is an autopilot without a `next_run`: the store sets it with `next_run` when
/// the autopilot is created, switched on or has fired.
pub fn due(autopilots: &[Autopilot], now_ms: i64) -> Vec<&Autopilot> {
    autopilots
        .iter()
        .filter(|a| a.enabled && a.paused_reason.is_none())
        .filter(|a| matches!(a.trigger, AutopilotTrigger::Schedule { .. }))
        .filter(|a| a.next_run.is_some_and(|at| at <= now_ms))
        .collect()
}

/// How many autopilots are on and how many are paused: "2 on · 1 paused".
pub fn header_count(autopilots: &[Autopilot]) -> (usize, usize) {
    let on = autopilots.iter().filter(|a| a.enabled && a.paused_reason.is_none()).count();
    (on, autopilots.len() - on)
}

/// The popover's header: "2 on · 1 paused", "2 on" when none is paused.
pub fn header_text(autopilots: &[Autopilot]) -> String {
    match header_count(autopilots) {
        (on, 0) => format!("{on} on"),
        (on, paused) => format!("{on} on · {paused} paused"),
    }
}

/// The button of the issues header: "Autopilots · 2" (the ones that are on).
pub fn button_label(autopilots: &[Autopilot]) -> String {
    format!("Autopilots · {}", header_count(autopilots).0)
}
