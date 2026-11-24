//! A failure part-way through importing one thing leaves nothing of it behind, so the next
//! import starts clean. Failures are provoked with SQLite triggers on the in-memory store.

use model::{IssueKey, RunId, SessionId};
use serde_json::json;

use super::{Fixture, assistant_text, old_issue, old_run, old_session, png, user_text};

fn execute(fx: &Fixture, sql: &str) {
    fx.store.db.write().execute_batch(sql).unwrap();
}

#[test]
fn a_session_that_fails_after_its_usage_leaves_no_ledger_row() {
    let fx = Fixture::new();
    let legacy = "1763560000123-4821";
    let mut session = old_session(legacy, json!([user_text("hi"), assistant_text("hello")]));
    session["cost_usd"] = json!(0.5);
    session["usage"] = json!({"input_tokens": 10, "output_tokens": 5, "cache_read_tokens": 0, "cache_write_tokens": 0});
    fx.write_json(&format!("sessions/{legacy}.json"), &session);
    // The legacy id is the last step of writing a session.
    execute(
        &fx,
        "CREATE TRIGGER fail_legacy BEFORE UPDATE OF legacy_id ON sessions
         BEGIN SELECT RAISE(ABORT, 'boom'); END;",
    );

    let report = fx.import();
    assert_eq!((report.sessions.imported, report.sessions.failed), (0, 1), "{report}");
    assert!(fx.store.session(&SessionId::new(format!("v1-{legacy}"))).unwrap().is_none());
    assert_eq!(fx.store.spend_between(0, i64::MAX).unwrap().calls, 0);

    execute(&fx, "DROP TRIGGER fail_legacy");
    let report = fx.import();
    assert_eq!(report.sessions.imported, 1, "{report}");
    let spend = fx.store.spend_between(0, i64::MAX).unwrap();
    assert_eq!(spend.calls, 1);
    assert!((spend.cost_usd - 0.5).abs() < 1e-9);
}

#[test]
fn a_run_that_fails_midway_is_not_taken_for_imported_by_the_retry() {
    let fx = Fixture::new();
    let mut run = old_run("run1", "HAR-1", "done");
    run["log"] = json!([{"at": 1_700_000_060, "who": "pipeline", "kind": "step", "text": "ready"}]);
    run["usage"] = json!([{"who": "Build", "model": "nr/cc/claude-opus-5-5", "tokens": 100, "cost_usd": 1.5}]);
    run["renders"] = json!([{"step": "render", "screen": "home", "before": null,
        "after": fx.data().join("runs/run1/renders/home.png")}]);
    fx.write_json(
        "issues.json",
        &json!({"next": 1, "items": [old_issue("HAR-1", "review", Some("run1"))]}),
    );
    fx.write_json("runs/run1.json", &run);
    fx.write("runs/run1/renders/home.png", png(10, 10, 1));
    // Renders are written before the usage rows.
    execute(
        &fx,
        "CREATE TRIGGER fail_renders BEFORE UPDATE OF renders ON runs
         BEGIN SELECT RAISE(ABORT, 'boom'); END;",
    );

    let report = fx.import();
    assert_eq!((report.runs.imported, report.runs.failed), (0, 1), "{report}");
    let id = RunId::new("run1");
    assert!(fx.store.run(&id).unwrap().is_none());
    assert!(fx.store.run_log_tail(&id, 10).unwrap().is_empty());

    execute(&fx, "DROP TRIGGER fail_renders");
    let report = fx.import();
    assert_eq!(report.runs.imported, 1, "{report}");
    assert_eq!(fx.store.renders(&id).unwrap().len(), 1);
    assert_eq!(fx.store.run_log_tail(&id, 10).unwrap().len(), 1);
    assert_eq!(fx.store.spend_for_run(&id).unwrap().calls, 1);
    assert_eq!(fx.import().runs.skipped, 1);
}

#[test]
fn an_issue_whose_feed_fails_is_written_whole_by_the_retry() {
    let fx = Fixture::new();
    let mut issue = old_issue("HAR-1", "backlog", None);
    issue["comments"] = json!([
        {"id": 1, "author": "me", "created": 1_700_000_400, "text": "one", "mentions": []},
        {"id": 2, "author": "me", "created": 1_700_000_500, "text": "two", "mentions": []},
    ]);
    fx.write_json("issues.json", &json!({"next": 1, "items": [issue]}));
    execute(
        &fx,
        "CREATE TRIGGER fail_feed BEFORE INSERT ON issue_events WHEN NEW.seq = 3
         BEGIN SELECT RAISE(ABORT, 'boom'); END;",
    );

    let report = fx.import();
    assert_eq!((report.issues.imported, report.issues.failed), (0, 1), "{report}");
    let key = IssueKey::new("HAR-1");
    assert!(fx.store.issue(&key).unwrap().is_none());

    execute(&fx, "DROP TRIGGER fail_feed");
    let report = fx.import();
    assert_eq!(report.issues.imported, 1, "{report}");
    // Created and two comments.
    assert_eq!(fx.store.issue_events(&key).unwrap().len(), 3);
}

#[test]
fn a_huge_old_counter_is_applied_at_once() {
    let fx = Fixture::new();
    fx.write_json(
        "issues.json",
        &json!({"next": 1_000_000_000_000u64, "items": [old_issue("HAR-1", "done", None)]}),
    );
    let report = fx.import();
    assert!(report.problems.is_empty(), "{report}");
    assert_eq!(fx.store.next_issue_key("HAR").unwrap().as_str(), "HAR-1000000000001");
}
