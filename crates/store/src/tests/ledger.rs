use base::time::{day_start, from_civil, week_start};
use model::{ModelRef, NodeId, RunId, RunnableRef, SessionId, Usage, UsageEntry};

use super::store;

const OFFSET: i32 = 3 * 3600;
const DAY: i64 = 86_400_000;

fn entry(at: i64, runnable: &str, cost: f64) -> UsageEntry {
    UsageEntry {
        at,
        session: None,
        run: None,
        node: None,
        runnable: RunnableRef(runnable.into()),
        name: runnable.split(':').nth(1).unwrap_or(runnable).to_string(),
        created_by: None,
        model: ModelRef::parse("nr/cc/claude-opus-5-5"),
        usage: Usage { input: 10, output: 5, cache_read: 100, cache_write: 1 },
        cost_usd: cost,
        imported: false,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn totals_cover_half_open_ranges() {
    let store = store();
    store.record_usage(&entry(1_000, "agent:build", 1.0)).unwrap();
    store.record_usage(&entry(2_000, "agent:build", 2.0)).unwrap();
    store.record_usage(&entry(3_000, "agent:fixer", 4.0)).unwrap();
    let total = store.spend_between(1_000, 3_000).unwrap();
    assert!(close(total.cost_usd, 3.0));
    assert_eq!(total.calls, 2);
    assert_eq!(total.usage, Usage { input: 20, output: 10, cache_read: 200, cache_write: 2 });
    assert_eq!(store.spend_between(0, 10_000).unwrap().calls, 3);
    assert_eq!(store.spend_between(5_000, 10_000).unwrap(), crate::SpendTotal::default());
}

#[test]
fn day_buckets_follow_local_midnight() {
    let store = store();
    // Local midnight of 23 November 2026 at UTC+3 is 21:00 UTC on the 22nd.
    let midnight = from_civil(2026, 11, 23, 0, 0, 0, OFFSET);
    assert_eq!(day_start(midnight + 1, OFFSET), midnight);
    store.record_usage(&entry(midnight - 1, "agent:a", 1.0)).unwrap();
    store.record_usage(&entry(midnight, "agent:a", 2.0)).unwrap();
    store.record_usage(&entry(midnight + DAY - 1, "agent:a", 4.0)).unwrap();
    store.record_usage(&entry(midnight + 2 * DAY + 5, "agent:a", 8.0)).unwrap();

    let buckets = store.spend_by_day(midnight - DAY + 7, midnight + 3 * DAY, OFFSET).unwrap();
    let starts: Vec<i64> = buckets.iter().map(|b| b.start).collect();
    assert_eq!(starts, vec![midnight - DAY, midnight, midnight + DAY, midnight + 2 * DAY]);
    let costs: Vec<f64> = buckets.iter().map(|b| b.cost_usd).collect();
    assert_eq!(costs, vec![1.0, 6.0, 0.0, 8.0]);
    for bucket in &buckets {
        assert_eq!(day_start(bucket.start, OFFSET), bucket.start);
    }

    // In UTC the same calls fall on other days: 21:00 UTC splits nothing.
    let utc = store.spend_by_day(midnight - DAY, midnight + 3 * DAY, 0).unwrap();
    assert_eq!(utc[0].start, day_start(midnight - DAY, 0));
    assert!(close(utc.iter().map(|b| b.cost_usd).sum::<f64>(), 15.0));
    assert!(store.spend_by_day(midnight, midnight, OFFSET).unwrap().is_empty());
}

#[test]
fn week_buckets_start_on_local_monday() {
    let store = store();
    let monday = week_start(from_civil(2026, 11, 25, 12, 0, 0, OFFSET), OFFSET);
    assert_eq!(base::time::civil(monday, OFFSET).weekday, 0);
    assert_eq!(base::time::civil(monday, OFFSET).hour, 0);
    store.record_usage(&entry(monday - 1, "agent:a", 1.0)).unwrap();
    store.record_usage(&entry(monday, "agent:a", 2.0)).unwrap();
    store.record_usage(&entry(monday + 7 * DAY - 1, "agent:a", 4.0)).unwrap();
    store.record_usage(&entry(monday + 14 * DAY, "agent:a", 8.0)).unwrap();
    let weeks = store.spend_by_week(monday - 3 * DAY, monday + 15 * DAY, OFFSET).unwrap();
    let got: Vec<(i64, f64)> = weeks.iter().map(|w| (w.start, w.cost_usd)).collect();
    assert_eq!(got, vec![(monday - 7 * DAY, 1.0), (monday, 6.0), (monday + 7 * DAY, 0.0), (monday + 14 * DAY, 8.0)]);
}

#[test]
fn spend_by_runnable_keeps_the_top_and_sums_the_rest() {
    let store = store();
    store.record_usage(&entry(10, "agent:build", 5.0)).unwrap();
    let mut renamed = entry(20, "agent:build", 1.0);
    renamed.name = "Builder".into();
    store.record_usage(&renamed).unwrap();
    let mut spawned = entry(10, "spawned:x1", 3.0);
    spawned.created_by = Some(RunnableRef::agent("lead"));
    store.record_usage(&spawned).unwrap();
    store.record_usage(&entry(10, "agent:fixer", 2.0)).unwrap();
    store.record_usage(&entry(10, "agent:docs", 0.5)).unwrap();
    store.record_usage(&entry(99_999, "agent:late", 100.0)).unwrap();

    let rows = store.spend_by_runnable(0, 1_000, 2).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!((rows[0].runnable.as_str(), rows[0].name.as_str(), rows[0].spawned), ("agent:build", "Builder", false));
    assert!(close(rows[0].cost_usd, 6.0));
    assert_eq!((rows[1].runnable.as_str(), rows[1].spawned), ("spawned:x1", true));
    assert_eq!((rows[2].runnable.as_str(), rows[2].name.as_str()), ("", "Others"));
    assert!(close(rows[2].cost_usd, 2.5));

    let all = store.spend_by_runnable(0, 1_000, 10).unwrap();
    assert_eq!(all.len(), 4, "no Others row without a rest");
}

#[test]
fn spend_by_session_run_and_node() {
    let store = store();
    let run = RunId::new("r1");
    let mut a = entry(10, "agent:fixer", 1.0);
    a.run = Some(run.clone());
    a.node = Some(NodeId::new("fixer"));
    a.session = Some(SessionId::new("s-a"));
    let mut b = a.clone();
    b.cost_usd = 2.0;
    let mut c = a.clone();
    c.node = Some(NodeId::new("review"));
    c.cost_usd = 4.0;
    let mut d = entry(10, "agent:build", 8.0);
    d.session = Some(SessionId::new("chat"));
    for e in [&a, &b, &c, &d] {
        store.record_usage(e).unwrap();
    }
    assert!(close(store.spend_for_run(&run).unwrap().cost_usd, 7.0));
    assert!(close(store.spend_for_session(&SessionId::new("chat")).unwrap().cost_usd, 8.0));
    assert_eq!(store.spend_for_session(&SessionId::new("s-a")).unwrap().calls, 3);
    let nodes = store.spend_by_node(&run).unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!((nodes[0].0.as_str(), nodes[0].1.calls), ("fixer", 2));
    assert!(close(nodes[0].1.cost_usd, 3.0));
    assert_eq!(nodes[1].0.as_str(), "review");
}

#[test]
fn runs_are_counted_per_runnable() {
    let store = store();
    let at = |at: i64, runnable: &str, run: Option<&str>, session: Option<&str>| {
        let mut e = entry(at, runnable, 0.1);
        e.run = run.map(RunId::new);
        e.session = session.map(SessionId::new);
        store.record_usage(&e).unwrap();
    };
    at(100, "pipeline:triage", Some("r1"), Some("s1"));
    at(101, "pipeline:triage", Some("r1"), Some("s2"));
    at(102, "pipeline:triage", Some("r2"), None);
    at(103, "agent:build", None, Some("chat1"));
    at(104, "agent:build", None, Some("chat1"));
    at(5, "agent:build", None, Some("chat0"));
    let counts = store.runs_by_runnable(50).unwrap();
    assert_eq!(counts, vec![(RunnableRef::pipeline("triage"), 2), (RunnableRef::agent("build"), 1)]);
}
