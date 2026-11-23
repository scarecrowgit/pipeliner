use model::{InboxId, InboxKind, Link, Outcome, SessionId};
use serde_json::{Value, json};

use super::{Fixture, old_session, user_text};

fn item(id: &str) -> Value {
    json!({
        "id": id, "created": 1_700_000_000, "chat_id": "", "chat_title": "A chat", "agent": "Build",
        "workdir": null, "wd_label": "pipeliner \u{b7} main", "title": "Did the thing", "summary": "## Done\n\n- one",
        "outcome": "needs_review",
        "links": [{"url": "https://github.com/o/r/pull/42", "label": "PR"}, {"url": "https://example.com/doc", "label": "Doc"}],
        "read": false, "archived": false, "auto": false,
    })
}

fn write_inbox(fx: &Fixture, items: Vec<Value>) {
    fx.write_json("inbox.json", &json!({"items": items}));
}

#[test]
fn an_item_becomes_a_report_with_its_flags_links_and_time() {
    let fx = Fixture::new();
    let mut read = item("r2");
    read["read"] = json!(true);
    read["archived"] = json!(true);
    read["outcome"] = json!("blocked");
    read["links"] = json!([{"url": "https://example.com/x"}, {"url": ""}]);
    let mut odd = item("r3");
    odd["outcome"] = json!("something new");
    write_inbox(&fx, vec![item("r1"), read, odd]);
    let report = fx.import();
    assert_eq!((report.inbox.imported, report.inbox.failed), (3, 0), "{report}");

    let first = fx.store.inbox_item(&InboxId::new("r1")).unwrap().unwrap();
    assert_eq!(first.kind, InboxKind::Report);
    assert_eq!(
        (first.title.as_str(), first.summary.as_str()),
        ("Did the thing", "## Done\n\n- one")
    );
    assert_eq!(
        (first.outcome, first.source.as_str()),
        (Some(Outcome::NeedsReview), "Build")
    );
    assert_eq!(first.created, 1_700_000_000_000);
    assert_eq!((first.read, first.archived), (false, false));
    assert_eq!(
        first.links,
        vec![
            Link {
                label: "o/r#42".into(),
                url: "https://github.com/o/r/pull/42".into()
            },
            Link {
                label: "Doc".into(),
                url: "https://example.com/doc".into()
            },
        ]
    );
    let second = fx.store.inbox_item(&InboxId::new("r2")).unwrap().unwrap();
    assert_eq!(
        (second.read, second.archived, second.outcome),
        (true, true, Some(Outcome::Blocked))
    );
    assert_eq!(
        second.links,
        vec![Link {
            label: "https://example.com/x".into(),
            url: "https://example.com/x".into()
        }]
    );
    assert_eq!(
        fx.store.inbox_item(&InboxId::new("r3")).unwrap().unwrap().outcome,
        Some(Outcome::Done)
    );
    assert_eq!(fx.store.inbox(true).unwrap().len(), 1);
}

#[test]
fn an_item_points_at_the_session_it_came_from_when_that_was_imported() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("hi")])),
    );
    let mut linked = item("r1");
    linked["chat_id"] = json!("1763560000001-0001");
    let mut orphan = item("r2");
    orphan["chat_id"] = json!("1763560000099-9999");
    write_inbox(&fx, vec![linked, orphan]);
    fx.import();

    assert_eq!(
        fx.store.inbox_item(&InboxId::new("r1")).unwrap().unwrap().session,
        Some(SessionId::new("v1-1763560000001-0001"))
    );
    assert_eq!(fx.store.inbox_item(&InboxId::new("r2")).unwrap().unwrap().session, None);

    // A later run still links the session: it is found through the store, not only this run.
    let fx2 = Fixture::new();
    fx2.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("hi")])),
    );
    fx2.import();
    let mut late = item("r9");
    late["chat_id"] = json!("1763560000001-0001");
    write_inbox(&fx2, vec![late]);
    fx2.import();
    assert_eq!(
        fx2.store.inbox_item(&InboxId::new("r9")).unwrap().unwrap().session,
        Some(SessionId::new("v1-1763560000001-0001"))
    );
}

#[test]
fn a_dry_run_links_sessions_it_would_import() {
    let fx = Fixture::new();
    fx.write_json(
        "sessions/1763560000001-0001.json",
        &old_session("1763560000001-0001", json!([user_text("hi")])),
    );
    let mut linked = item("r1");
    linked["chat_id"] = json!("1763560000001-0001");
    write_inbox(&fx, vec![linked]);
    let dry = fx.dry_import();
    assert_eq!((dry.sessions.imported, dry.inbox.imported), (1, 1));
    assert!(fx.store.inbox(false).unwrap().is_empty());
}

#[test]
fn importing_twice_imports_nothing_the_second_time_and_keeps_later_edits() {
    let fx = Fixture::new();
    write_inbox(&fx, vec![item("r1"), item("r2")]);
    assert_eq!(fx.import().inbox.imported, 2);
    // The person marked one read in the new app; a second import must not undo it.
    fx.store.set_inbox_read(&InboxId::new("r1"), true).unwrap();
    let second = fx.import();
    assert_eq!((second.inbox.imported, second.inbox.skipped), (0, 2));
    assert!(fx.store.inbox_item(&InboxId::new("r1")).unwrap().unwrap().read);
    assert_eq!(fx.store.inbox(false).unwrap().len(), 2);
}

#[test]
fn a_bad_item_or_file_is_reported_and_the_rest_import() {
    let fx = Fixture::new();
    write_inbox(
        &fx,
        vec![
            item("r1"),
            json!({"title": "no id"}),
            json!({"id": "  "}),
            json!("not an object"),
            item("r2"),
        ],
    );
    let report = fx.import();
    assert_eq!((report.inbox.imported, report.inbox.failed), (2, 3), "{report}");
    assert_eq!(report.problems[0].item, "inbox.json item 2");

    let fx = Fixture::new();
    fx.write("inbox.json", "{{");
    let report = fx.import();
    assert_eq!(
        (report.inbox.failed, report.problems[0].item.as_str()),
        (1, "inbox.json")
    );
}
