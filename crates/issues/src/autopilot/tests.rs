use model::{AutopilotId, ProjectId, RunnableRef};

use super::*;

const H: i64 = 3_600_000;
const MIN: i64 = 60_000;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    time::from_civil(y, mo, d, h, mi, 0, 0)
}

fn pilot(trigger: AutopilotTrigger) -> Autopilot {
    Autopilot {
        id: AutopilotId::new("a"),
        project: ProjectId::new("pay"),
        name: "n".into(),
        trigger,
        target: RunnableRef::pipeline("triage"),
        prompt: String::new(),
        enabled: true,
        paused_reason: None,
        next_run: None,
        last_run: None,
        last_result: String::new(),
        budget_usd: None,
    }
}

fn daily(hour: u32, minute: u32) -> AutopilotTrigger {
    AutopilotTrigger::Schedule { schedule: Schedule::Day { hour, minute } }
}

#[test]
fn daily_fires_today_or_tomorrow() {
    let s = Schedule::Day { hour: 2, minute: 0 };
    assert_eq!(next_run(&s, utc(2024, 3, 10, 1, 0), 0), utc(2024, 3, 10, 2, 0));
    assert_eq!(next_run(&s, utc(2024, 3, 10, 3, 0), 0), utc(2024, 3, 11, 2, 0));
    // Strictly after: the exact fire time gives the next one.
    assert_eq!(next_run(&s, utc(2024, 3, 10, 2, 0), 0), utc(2024, 3, 11, 2, 0));
    assert_eq!(next_run(&s, utc(2024, 3, 10, 2, 0) - 1, 0), utc(2024, 3, 10, 2, 0));
}

#[test]
fn daily_crosses_month_year_and_leap_boundaries() {
    let s = Schedule::Day { hour: 2, minute: 30 };
    assert_eq!(next_run(&s, utc(2024, 1, 31, 12, 0), 0), utc(2024, 2, 1, 2, 30));
    assert_eq!(next_run(&s, utc(2024, 2, 28, 12, 0), 0), utc(2024, 2, 29, 2, 30));
    assert_eq!(next_run(&s, utc(2024, 2, 29, 12, 0), 0), utc(2024, 3, 1, 2, 30));
    assert_eq!(next_run(&s, utc(2023, 2, 28, 12, 0), 0), utc(2023, 3, 1, 2, 30));
    assert_eq!(next_run(&s, utc(2024, 12, 31, 12, 0), 0), utc(2025, 1, 1, 2, 30));
    // Midnight schedule just before midnight.
    let midnight = Schedule::Day { hour: 0, minute: 0 };
    assert_eq!(next_run(&midnight, utc(2024, 12, 31, 23, 59) + 59_999, 0), utc(2025, 1, 1, 0, 0));
}

#[test]
fn daily_in_positive_and_negative_offsets() {
    let s = Schedule::Day { hour: 2, minute: 0 };
    // UTC+5:30: 20:00 UTC on Jan 1 is 01:30 local on Jan 2; 02:00 local is 20:30 UTC.
    let ist = 19_800;
    assert_eq!(next_run(&s, utc(2024, 1, 1, 20, 0), ist), utc(2024, 1, 1, 20, 30));
    assert_eq!(next_run(&s, utc(2024, 1, 1, 20, 30), ist), utc(2024, 1, 2, 20, 30));
    // UTC-8: 05:00 UTC on Jan 1 is 21:00 local on Dec 31; 02:00 local is 10:00 UTC on Jan 1.
    let pst = -28_800;
    assert_eq!(next_run(&s, utc(2024, 1, 1, 5, 0), pst), utc(2024, 1, 1, 10, 0));
    assert_eq!(next_run(&s, utc(2024, 1, 1, 10, 0), pst), utc(2024, 1, 2, 10, 0));
    // The local day differs from the UTC day: 23:00 UTC Feb 28 is Mar 1 04:00 local at +5.
    assert_eq!(next_run(&s, utc(2023, 2, 28, 23, 0), 5 * 3600), utc(2023, 3, 1, 21, 0));
    // UTC+14 and UTC-12, the extremes.
    assert_eq!(next_run(&s, utc(2024, 6, 1, 0, 0), 14 * 3600), utc(2024, 6, 1, 12, 0));
    assert_eq!(next_run(&s, utc(2024, 6, 1, 0, 0), -12 * 3600), utc(2024, 6, 1, 14, 0));
}

#[test]
fn weekly_finds_the_next_weekday() {
    // 2024-01-01 is a Monday.
    let mon9 = Schedule::Week { weekday: 0, hour: 9, minute: 0 };
    assert_eq!(next_run(&mon9, utc(2024, 1, 1, 8, 0), 0), utc(2024, 1, 1, 9, 0));
    assert_eq!(next_run(&mon9, utc(2024, 1, 1, 9, 0), 0), utc(2024, 1, 8, 9, 0));
    assert_eq!(next_run(&mon9, utc(2024, 1, 3, 12, 0), 0), utc(2024, 1, 8, 9, 0));
    assert_eq!(next_run(&mon9, utc(2024, 1, 7, 23, 0), 0), utc(2024, 1, 8, 9, 0));
    let sun = Schedule::Week { weekday: 6, hour: 23, minute: 59 };
    assert_eq!(next_run(&sun, utc(2024, 1, 1, 0, 0), 0), utc(2024, 1, 7, 23, 59));
    let wed = Schedule::Week { weekday: 2, hour: 12, minute: 0 };
    assert_eq!(next_run(&wed, utc(2024, 1, 3, 12, 0), 0), utc(2024, 1, 10, 12, 0));
}

#[test]
fn weekly_crosses_month_and_year_boundaries() {
    let mon9 = Schedule::Week { weekday: 0, hour: 9, minute: 0 };
    // Wed 2024-01-31 -> Mon Feb 5.
    assert_eq!(next_run(&mon9, utc(2024, 1, 31, 12, 0), 0), utc(2024, 2, 5, 9, 0));
    // Tue 2024-12-31 -> Mon 2025-01-06.
    assert_eq!(next_run(&mon9, utc(2024, 12, 31, 12, 0), 0), utc(2025, 1, 6, 9, 0));
    // Thu 2024-02-29 -> Mon Mar 4.
    assert_eq!(next_run(&mon9, utc(2024, 2, 29, 12, 0), 0), utc(2024, 3, 4, 9, 0));
}

#[test]
fn weekly_in_offsets_uses_the_local_weekday() {
    let mon9 = Schedule::Week { weekday: 0, hour: 9, minute: 0 };
    // Sunday 2024-01-07 20:00 UTC is already Monday 01:30 local at +5:30: fires at 09:00 local = 03:30 UTC.
    assert_eq!(next_run(&mon9, utc(2024, 1, 7, 20, 0), 19_800), utc(2024, 1, 8, 3, 30));
    // Monday 2024-01-01 05:00 UTC is still Sunday 21:00 local at -8: fires Monday 09:00 local = 17:00 UTC.
    assert_eq!(next_run(&mon9, utc(2024, 1, 1, 5, 0), -28_800), utc(2024, 1, 1, 17, 0));
    // Monday 10:00 local at -8 is past 09:00: next Monday.
    assert_eq!(next_run(&mon9, utc(2024, 1, 1, 18, 0), -28_800), utc(2024, 1, 8, 17, 0));
}

#[test]
fn minutes_count_from_the_given_time() {
    let s = Schedule::Minutes { minutes: 15 };
    let t = utc(2024, 1, 1, 0, 7);
    assert_eq!(next_run(&s, t, 0), t + 15 * MIN);
    assert_eq!(next_run(&s, t, 19_800), t + 15 * MIN);
    assert_eq!(next_run(&Schedule::Minutes { minutes: 0 }, t, 0), t + MIN);
    assert!(next_run(&Schedule::Minutes { minutes: u32::MAX }, i64::MAX - 1, 0) >= i64::MAX - 1);
}

#[test]
fn out_of_range_fields_are_clamped() {
    let s = Schedule::Day { hour: 99, minute: 99 };
    assert_eq!(next_run(&s, utc(2024, 1, 1, 0, 0), 0), utc(2024, 1, 1, 23, 59));
    let w = Schedule::Week { weekday: 9, hour: 0, minute: 0 };
    // Weekday 9 counts as 2 (Wednesday).
    assert_eq!(next_run(&w, utc(2024, 1, 1, 0, 0), 0), utc(2024, 1, 3, 0, 0));
}

#[test]
fn next_run_is_always_strictly_after_and_within_a_period() {
    let schedules = [Schedule::Day { hour: 2, minute: 0 }, Schedule::Week { weekday: 4, hour: 17, minute: 45 }];
    for offset in [-28_800, 0, 19_800, 50_400] {
        let mut t = utc(2024, 2, 20, 0, 0);
        for _ in 0..800 {
            for s in &schedules {
                let n = next_run(s, t, offset);
                let period = if matches!(s, Schedule::Day { .. }) { 24 * H } else { 168 * H };
                assert!(n > t && n - t <= period, "{s:?} {offset} {t}");
            }
            t += 7 * H + 13 * MIN;
        }
    }
}

#[test]
fn describes_triggers_like_the_mockup() {
    assert_eq!(describe(&daily(2, 0), "PAY triage"), "Every day at 02:00 → PAY triage");
    let monday = AutopilotTrigger::Schedule { schedule: Schedule::Week { weekday: 0, hour: 9, minute: 0 } };
    assert_eq!(describe(&monday, "Codex"), "Mondays at 09:00 → Codex");
    let sunday = AutopilotTrigger::Schedule { schedule: Schedule::Week { weekday: 6, hour: 7, minute: 5 } };
    assert_eq!(describe(&sunday, "X"), "Sundays at 07:05 → X");
    let event = AutopilotTrigger::Event { source: "payments-api".into() };
    assert_eq!(describe(&event, "Reproducer"), "On a new error in payments-api → Reproducer");
    let tagged = AutopilotTrigger::Event { source: "sentry:payments-api".into() };
    assert_eq!(describe(&tagged, "Reproducer"), "On a new error in payments-api → Reproducer");
}

#[test]
fn describes_intervals() {
    let d = |m| describe_schedule(&Schedule::Minutes { minutes: m });
    assert_eq!((d(1).as_str(), d(15).as_str(), d(60).as_str(), d(120).as_str(), d(90).as_str()),
        ("Every minute", "Every 15 minutes", "Every hour", "Every 2 hours", "Every 90 minutes"));
}

#[test]
fn status_line_for_a_scheduled_autopilot() {
    let now = utc(2024, 1, 1, 12, 0);
    let mut a = pilot(daily(2, 0));
    a.next_run = Some(now + 5 * H + 40 * MIN);
    a.last_run = Some(now - H);
    a.last_result = "filed 2 issues".into();
    assert_eq!(status_line(&a, now), "Next run in 5h 40m · last run filed 2 issues");
    a.last_result.clear();
    assert_eq!(status_line(&a, now), "Next run in 5h 40m");
    a.next_run = Some(now + 30_000);
    assert_eq!(status_line(&a, now), "Next run in 30s");
    a.next_run = Some(now);
    assert_eq!(status_line(&a, now), "Next run due now");
    a.next_run = None;
    assert_eq!(status_line(&a, now), "Next run not scheduled");
}

#[test]
fn status_line_when_paused() {
    let mut a = pilot(daily(9, 0));
    a.enabled = false;
    a.paused_reason = Some("GitLab token expires in 3 days".into());
    assert_eq!(status_line(&a, 0), "Paused · GitLab token expires in 3 days");
    a.paused_reason = None;
    assert_eq!(status_line(&a, 0), "Paused");
    // A paused reason wins even when the flag still says enabled.
    a.enabled = true;
    a.paused_reason = Some("repo missing".into());
    assert_eq!(status_line(&a, 0), "Paused · repo missing");
}

#[test]
fn status_line_for_an_event_autopilot() {
    let mut a = pilot(AutopilotTrigger::Event { source: "payments-api".into() });
    assert_eq!(status_line(&a, 0), "Waiting for events");
    a.last_result = "3 issues today".into();
    a.budget_usd = Some(0.5);
    assert_eq!(status_line(&a, 0), "3 issues today · ≤$0.50 each");
    a.last_result.clear();
    assert_eq!(status_line(&a, 0), "≤$0.50 each");
}

#[test]
fn due_picks_enabled_scheduled_autopilots_that_have_reached_their_time() {
    let now = 1_000_000;
    let mk = |id: &str, trigger, next: Option<i64>, enabled: bool, reason: Option<&str>| {
        let mut a = pilot(trigger);
        a.id = AutopilotId::new(id);
        a.next_run = next;
        a.enabled = enabled;
        a.paused_reason = reason.map(String::from);
        a
    };
    let event = AutopilotTrigger::Event { source: "x".into() };
    let all = vec![
        mk("due", daily(2, 0), Some(now - 1), true, None),
        mk("exact", daily(2, 0), Some(now), true, None),
        mk("later", daily(2, 0), Some(now + 1), true, None),
        mk("off", daily(2, 0), Some(now - 1), false, None),
        mk("paused", daily(2, 0), Some(now - 1), true, Some("token")),
        mk("unscheduled", daily(2, 0), None, true, None),
        mk("event", event, Some(now - 1), true, None),
    ];
    let ids: Vec<_> = due(&all, now).iter().map(|a| a.id.to_string()).collect();
    assert_eq!(ids, ["due", "exact"]);
    assert!(due(&[], now).is_empty());
}

#[test]
fn header_counts_and_labels() {
    let on = pilot(daily(2, 0));
    let mut off = pilot(daily(9, 0));
    off.enabled = false;
    let mut broken = pilot(daily(9, 0));
    broken.paused_reason = Some("x".into());
    assert_eq!(header_count(&[]), (0, 0));
    assert_eq!(header_count(&[on.clone(), on.clone(), off.clone()]), (2, 1));
    assert_eq!(header_count(&[on.clone(), broken.clone()]), (1, 1));
    assert_eq!(header_text(&[on.clone(), on.clone(), off]), "2 on · 1 paused");
    assert_eq!(header_text(std::slice::from_ref(&on)), "1 on");
    assert_eq!(button_label(&[on.clone(), on, broken]), "Autopilots · 2");
}

#[test]
fn a_tick_missed_by_less_than_a_day_fires_and_an_older_one_is_skipped() {
    let now = utc(2026, 11, 24, 12, 0);
    let mut recent = pilot(daily(3, 0));
    recent.next_run = Some(utc(2026, 11, 24, 3, 0));
    assert_eq!(tick(&recent, now, 0), Tick::Fire);
    let mut old = pilot(daily(3, 0));
    old.next_run = Some(utc(2026, 11, 22, 3, 0));
    assert_eq!(tick(&old, now, 0), Tick::Skip { next_run: utc(2026, 11, 25, 3, 0) });
    let mut off = pilot(daily(1, 0));
    off.enabled = false;
    off.next_run = Some(1);
    assert_eq!(next_due(&[recent.clone(), old.clone(), off]), Some(utc(2026, 11, 22, 3, 0)));
    assert_eq!(next_due(&[]), None);
}
