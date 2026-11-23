use model::{
    Actor, IssueEventKind, IssueKey, IssueStatus, IssueType, Priority, ProjectId, RunId, RunnableRef, StartTriggers,
};
use serde_json::json;

use super::{Fixture, old_issue, old_run};

fn project() -> ProjectId {
    ProjectId::new("pipeliner")
}

fn write_issues(fx: &Fixture, next: u64, items: Vec<serde_json::Value>) {
    fx.write_json("issues.json", &json!({"next": next, "items": items}));
}

#[test]
fn every_old_status_and_kind_maps_to_the_new_ones() {
    let fx = Fixture::new();
    let statuses = [
        ("backlog", IssueStatus::Backlog),
        ("triage", IssueStatus::InProgress),
        ("design", IssueStatus::InProgress),
        ("approval", IssueStatus::InProgress),
        ("build", IssueStatus::InProgress),
        ("failed", IssueStatus::InProgress),
        ("review", IssueStatus::InReview),
        ("done", IssueStatus::Done),
    ];
    let mut items = Vec::new();
    for (index, (status, _)) in statuses.iter().enumerate() {
        items.push(old_issue(&format!("HAR-{}", index + 1), status, None));
    }
    for (offset, kind) in ["Feature", "Bug", "Chore", "Docs", "Whatever"].into_iter().enumerate() {
        let mut issue = old_issue(&format!("HAR-{}", 20 + offset), "backlog", None);
        issue["kind"] = json!(kind);
        items.push(issue);
    }
    write_issues(&fx, 30, items);
    let report = fx.import();
    assert_eq!(report.issues.imported, 13, "{report}");

    for (index, (_, expected)) in statuses.iter().enumerate() {
        let issue = fx
            .store
            .issue(&IssueKey::new(format!("HAR-{}", index + 1)))
            .unwrap()
            .unwrap();
        assert_eq!(issue.status, *expected, "{}", issue.key);
    }
    let kinds: Vec<IssueType> = (20..25)
        .map(|n| {
            fx.store
                .issue(&IssueKey::new(format!("HAR-{n}")))
                .unwrap()
                .unwrap()
                .kind
        })
        .collect();
    assert_eq!(
        kinds,
        [
            IssueType::Story,
            IssueType::Bug,
            IssueType::Chore,
            IssueType::Docs,
            IssueType::Story
        ]
    );
}

#[test]
fn an_issue_keeps_its_key_text_and_gets_the_project() {
    let fx = Fixture::new();
    let mut issue = old_issue("HAR-12", "review", None);
    issue["pr_url"] = json!("https://github.com/o/r/pull/3");
    issue["body"] = json!("  The body\n");
    write_issues(&fx, 12, vec![issue]);
    fx.import();

    let project = fx.store.project(&project()).unwrap().unwrap();
    assert_eq!(
        (project.name.as_str(), project.key_prefix.as_str()),
        ("Pipeliner", "HAR")
    );
    let issue = fx.store.issue(&IssueKey::new("HAR-12")).unwrap().unwrap();
    assert_eq!(issue.project, ProjectId::new("pipeliner"));
    assert_eq!(
        (issue.title.as_str(), issue.body.as_str()),
        ("Title of HAR-12", "The body")
    );
    assert_eq!((issue.created, issue.updated), (1_700_000_000_000, 1_700_000_000_000));
    assert_eq!(issue.priority, Priority::Medium);
    assert_eq!(issue.runner, Some(RunnableRef::pipeline("mockup-gpui-build")));
    assert_eq!(issue.triggers, StartTriggers::default());
    assert_eq!(issue.pr_url.as_deref(), Some("https://github.com/o/r/pull/3"));
    assert_eq!((issue.run, issue.run_count, issue.filed_by), (None, 0, None));
    assert_eq!(fx.store.issues(&ProjectId::new("pipeliner")).unwrap().len(), 1);
}

#[test]
fn the_project_prefix_is_the_one_most_keys_carry() {
    let fx = Fixture::new();
    write_issues(
        &fx,
        0,
        vec![
            old_issue("ABC-1", "backlog", None),
            old_issue("XYZ-1", "backlog", None),
            old_issue("XYZ-2", "backlog", None),
        ],
    );
    fx.import();
    assert_eq!(fx.store.project(&project()).unwrap().unwrap().key_prefix, "XYZ");
}

#[test]
fn new_keys_start_past_the_highest_imported_and_past_the_old_counter() {
    // The highest key decides when it is the counter.
    let fx = Fixture::new();
    write_issues(
        &fx,
        7,
        vec![old_issue("HAR-3", "backlog", None), old_issue("HAR-7", "done", None)],
    );
    fx.import();
    fx.import();
    assert_eq!(fx.store.next_issue_key("HAR").unwrap().as_str(), "HAR-8");

    // The old counter decides when issues were deleted since: HAR-9 and HAR-10 are gone but
    // were used, so they are not handed out again.
    let fx = Fixture::new();
    write_issues(
        &fx,
        10,
        vec![old_issue("HAR-3", "backlog", None), old_issue("HAR-8", "done", None)],
    );
    fx.import();
    assert_eq!(fx.store.next_issue_key("HAR").unwrap().as_str(), "HAR-11");
    // Importing again does not move the counter on.
    fx.import();
    fx.import();
    assert_eq!(fx.store.next_issue_key("HAR").unwrap().as_str(), "HAR-12");
}

#[test]
fn comments_become_the_activity_feed() {
    let fx = Fixture::new();
    let mut issue = old_issue("HAR-4", "review", None);
    issue["pr_url"] = json!("https://github.com/o/r/pull/5");
    issue["comments"] = json!([
        {"id": 2, "author": "me", "created": 1_700_000_500, "text": "second @Build", "mentions": ["Build", "Build"]},
        {"id": 1, "author": "me", "created": 1_700_000_400, "text": " first ", "mentions": []},
        {"id": 3, "author": "me", "created": 1_700_000_600, "text": "   ", "mentions": []},
    ]);
    write_issues(&fx, 4, vec![issue]);
    fx.import();

    let events = fx.store.issue_events(&IssueKey::new("HAR-4")).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        (events[0].seq, events[0].at, &events[0].kind),
        (1, 1_700_000_000_000, &IssueEventKind::Created)
    );
    assert_eq!(events[0].actor, Actor::Me);
    assert_eq!(
        events[1].kind,
        IssueEventKind::Comment {
            text: "first".into(),
            images: vec![],
            mentions: vec![]
        }
    );
    assert_eq!(events[1].at, 1_700_000_400_000);
    assert_eq!(
        events[2].kind,
        IssueEventKind::Comment {
            text: "second @Build".into(),
            images: vec![],
            mentions: vec![RunnableRef::agent("build")]
        }
    );
    assert_eq!(
        events[3].kind,
        IssueEventKind::PrOpened {
            url: "https://github.com/o/r/pull/5".into()
        }
    );
}

#[test]
fn an_issue_takes_its_latest_run_and_counts_them_all() {
    let fx = Fixture::new();
    write_issues(&fx, 1, vec![old_issue("HAR-1", "review", Some("run2"))]);
    let mut first = old_run("run1", "HAR-1", "failed");
    first["created"] = json!(1_700_000_050);
    let mut second = old_run("run2", "HAR-1", "done");
    second["created"] = json!(1_700_001_050);
    fx.write_json("runs/run1.json", &first);
    fx.write_json("runs/run2.json", &second);
    let report = fx.import();
    assert_eq!((report.issues.imported, report.runs.imported), (1, 2), "{report}");

    let issue = fx.store.issue(&IssueKey::new("HAR-1")).unwrap().unwrap();
    assert_eq!((issue.run, issue.run_count), (Some(RunId::new("run2")), 2));
    // The issue last changed with its latest run.
    assert_eq!(issue.updated, 1_700_001_050_000);
    let runs = fx.store.runs_for_issue(&IssueKey::new("HAR-1")).unwrap();
    assert_eq!(
        runs.iter().map(|r| (r.id.as_str(), r.number)).collect::<Vec<_>>(),
        [("run2", 2), ("run1", 1)]
    );
    assert_eq!(
        fx.store.latest_run(&IssueKey::new("HAR-1")).unwrap().unwrap().id,
        RunId::new("run2")
    );
}

#[test]
fn importing_twice_imports_nothing_the_second_time() {
    let fx = Fixture::new();
    let mut issue = old_issue("HAR-1", "review", Some("run1"));
    issue["comments"] = json!([{"id": 1, "author": "me", "created": 1_700_000_400, "text": "hello", "mentions": []}]);
    write_issues(&fx, 1, vec![issue]);
    fx.write_json("runs/run1.json", &old_run("run1", "HAR-1", "done"));
    let first = fx.import();
    assert_eq!((first.issues.imported, first.runs.imported), (1, 1));

    let second = fx.import();
    assert_eq!((second.issues.imported, second.issues.skipped), (0, 1));
    assert_eq!((second.runs.imported, second.runs.skipped), (0, 1));
    assert_eq!(fx.store.issue_events(&IssueKey::new("HAR-1")).unwrap().len(), 2);
    assert_eq!(fx.store.spend_for_run(&RunId::new("run1")).unwrap().calls, 0);
    assert_eq!(fx.store.run_log_tail(&RunId::new("run1"), 100).unwrap().len(), 0);
}

#[test]
fn a_dry_run_writes_nothing_and_reports_what_a_real_run_does() {
    let fx = Fixture::new();
    write_issues(
        &fx,
        9,
        vec![
            old_issue("HAR-1", "review", Some("run1")),
            old_issue("HAR-2", "done", None),
        ],
    );
    let mut run = old_run("run1", "HAR-1", "done");
    run["renders"] = json!([{"step": "render", "screen": "home", "before": null, "after": fx.data().join("runs/run1/renders/home.png")}]);
    fx.write_json("runs/run1.json", &run);
    fx.write("runs/run1/renders/home.png", super::png(10, 20, 1));
    let before = fx.snapshot();

    let dry = fx.dry_import();
    assert!(fx.store.projects().unwrap().is_empty());
    assert!(fx.store.issue(&IssueKey::new("HAR-1")).unwrap().is_none());
    assert!(fx.store.run(&RunId::new("run1")).unwrap().is_none());
    assert_eq!(fx.store.next_issue_key("HAR").unwrap().as_str(), "HAR-1");

    let fresh = Fixture::new();
    fresh.write_json(
        "issues.json",
        &json!({"next": 9, "items": [old_issue("HAR-1", "review", Some("run1")), old_issue("HAR-2", "done", None)]}),
    );
    fresh.write_json("runs/run1.json", &run);
    fresh.write("runs/run1/renders/home.png", super::png(10, 20, 1));
    let real = fresh.import();
    assert_eq!(
        (dry.issues, dry.runs, dry.images),
        (real.issues, real.runs, real.images)
    );
    assert_eq!(real.images.imported, 1);
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn bad_issues_and_a_bad_issues_file_are_reported() {
    let fx = Fixture::new();
    write_issues(
        &fx,
        3,
        vec![
            old_issue("HAR-1", "backlog", None),
            json!({"key": "HAR-2", "status": "sideways"}),
            json!({"title": "no key", "status": "done"}),
            old_issue("nonsense", "backlog", None),
            old_issue("HAR-3", "done", None),
        ],
    );
    let report = fx.import();
    assert_eq!((report.issues.imported, report.issues.failed), (2, 3), "{report}");
    let items: Vec<&str> = report.problems.iter().map(|p| p.item.as_str()).collect();
    assert_eq!(items, ["HAR-2", "issues.json item 3", "nonsense"]);

    let fx = Fixture::new();
    fx.write("issues.json", "[1, 2");
    fx.write_json("runs/run1.json", &old_run("run1", "HAR-1", "done"));
    let report = fx.import();
    assert_eq!(report.problems[0].item, "issues.json");
    // The run is history of an issue that is not there; it still imports.
    assert_eq!(report.runs.imported, 1);
    assert_eq!(fx.store.run(&RunId::new("run1")).unwrap().unwrap().issue, None);
}
