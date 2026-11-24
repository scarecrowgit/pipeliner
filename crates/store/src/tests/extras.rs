//! Deleted comments, kept tool displays, stored queues and run state, with the migration that
//! moves run markers into their table.

use base::paths::Paths;
use model::{
    Actor, Attachment, BlobId, DiffHunk, DiffLine, DiffLineKind, FileDiff, IssueEvent, IssueEventKind, IssueKey, RunId,
    SessionId, ToolDisplay,
};

use super::issues::issue;
use super::{session, store};
use crate::schema::MIGRATIONS;
use crate::{QueuedMessage, Store};

fn comment(text: &str) -> IssueEvent {
    IssueEvent { seq: 0, at: 5, actor: Actor::Me, kind: IssueEventKind::Comment { text: text.into(), images: Vec::new(), mentions: Vec::new() } }
}

#[test]
fn a_deleted_comment_leaves_the_feed_and_its_seq_is_not_reused() {
    let store = store();
    store.upsert_issue(&issue("HAR-1", "p", 1)).unwrap();
    let key = IssueKey::new("HAR-1");
    store.append_issue_event(&key, comment("first")).unwrap();
    let second = store.append_issue_event(&key, comment("second")).unwrap();
    assert!(store.delete_issue_event(&key, second.seq).unwrap());
    assert!(!store.delete_issue_event(&key, second.seq).unwrap(), "already gone");
    assert!(!store.delete_issue_event(&key, 99).unwrap());
    let seqs: Vec<u64> = store.issue_events(&key).unwrap().iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1]);
    let third = store.append_issue_event(&key, comment("third")).unwrap();
    assert_eq!(third.seq, 3, "the deleted entry's seq stays taken");
}

fn diff_display() -> ToolDisplay {
    let line = |kind, old_no, new_no, text: &str| DiffLine { kind, old_no, new_no, text: text.into(), no_newline: false };
    ToolDisplay::Diff(FileDiff {
        path: "src/a.rs".into(),
        added: 1,
        removed: 1,
        hunks: vec![DiffHunk {
            lines: vec![
                line(DiffLineKind::Context, Some(4), Some(4), "fn a() {"),
                line(DiffLineKind::Del, Some(5), None, "    1"),
                line(DiffLineKind::Add, None, Some(5), "    2"),
            ],
        }],
        ..FileDiff::default()
    })
}

#[test]
fn tool_displays_are_kept_per_call_copied_on_fork_and_dropped_with_the_session() {
    let store = store();
    store.upsert_session(&session("s1", "One", 1_000)).unwrap();
    store.upsert_session(&session("s2", "Fork", 2_000)).unwrap();
    let (s1, s2) = (SessionId::new("s1"), SessionId::new("s2"));
    let text = ToolDisplay::Text { title: "x".into(), body: "first".into() };
    store.set_tool_display(&s1, "call-1", &text).unwrap();
    store.set_tool_display(&s1, "call-1", &diff_display()).unwrap();
    let calls = vec!["call-1".to_string(), "call-2".to_string()];
    let found = store.tool_displays(&s1, &calls).unwrap();
    assert_eq!(found.len(), 1, "call-2 has none");
    assert_eq!(found["call-1"], diff_display(), "the second display replaced the first");
    assert!(store.set_tool_display(&SessionId::new("nope"), "c", &text).is_err(), "only for stored sessions");

    store.copy_tool_displays(&s1, &s2).unwrap();
    assert_eq!(store.tool_displays(&s2, &calls).unwrap()["call-1"], diff_display());
    store.delete_session(&s1).unwrap();
    assert!(store.tool_displays(&s1, &calls).unwrap().is_empty());
    assert_eq!(store.tool_displays(&s2, &calls).unwrap().len(), 1);
}

#[test]
fn queues_are_replaced_whole_and_read_back_in_order() {
    let store = store();
    store.upsert_session(&session("a", "A", 1_000)).unwrap();
    store.upsert_session(&session("b", "B", 1_000)).unwrap();
    let image = Attachment { blob: BlobId::new("b1"), name: "shot.png".into(), media_type: "image/png".into(), bytes: 3 };
    let one = QueuedMessage { text: "one".into(), images: vec![image] };
    let two = QueuedMessage { text: "two".into(), images: Vec::new() };
    store.set_queued_messages(&SessionId::new("a"), &[one.clone(), two.clone()]).unwrap();
    store.set_queued_messages(&SessionId::new("b"), std::slice::from_ref(&two)).unwrap();
    let all = store.queued_messages().unwrap();
    assert_eq!(all, vec![(SessionId::new("a"), vec![one, two.clone()]), (SessionId::new("b"), vec![two.clone()])]);

    store.set_queued_messages(&SessionId::new("a"), &[]).unwrap();
    store.delete_session(&SessionId::new("b")).unwrap();
    assert!(store.queued_messages().unwrap().is_empty());
}

#[test]
fn run_state_keeps_graph_and_snapshot_apart() {
    let store = store();
    let run = RunId::new("r1");
    assert_eq!(store.run_graph(&run).unwrap(), None);
    store.set_run_snapshot(&run, "{\"s\":1}").unwrap();
    assert_eq!(store.run_graph(&run).unwrap(), None, "a snapshot alone leaves the graph empty");
    store.set_run_graph(&run, "{\"g\":1}").unwrap();
    store.set_run_snapshot(&run, "{\"s\":2}").unwrap();
    assert_eq!(store.run_graph(&run).unwrap().as_deref(), Some("{\"g\":1}"));
    assert_eq!(store.run_snapshot(&run).unwrap().as_deref(), Some("{\"s\":2}"));
}

#[test]
fn the_migration_moves_run_markers_into_run_state() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    std::fs::create_dir_all(paths.v2_dir()).unwrap();
    {
        let conn = rusqlite::Connection::open(paths.db_file()).unwrap();
        for sql in &MIGRATIONS[..4] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute_batch(
            "INSERT INTO meta (key, value) VALUES
                ('run-graph:r1', 'graph one'), ('run-snapshot:r1', 'snap one'),
                ('run-snapshot:r2', 'snap two'), ('summary:s1', 'kept');",
        )
        .unwrap();
    }
    let store = Store::open(&paths).unwrap();
    assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
    let (r1, r2) = (RunId::new("r1"), RunId::new("r2"));
    assert_eq!(store.run_graph(&r1).unwrap().as_deref(), Some("graph one"));
    assert_eq!(store.run_snapshot(&r1).unwrap().as_deref(), Some("snap one"));
    assert_eq!((store.run_graph(&r2).unwrap(), store.run_snapshot(&r2).unwrap().as_deref()), (None, Some("snap two")));
    assert_eq!(store.marker("run-graph:r1").unwrap(), None, "the markers are gone");
    assert_eq!(store.marker("run-snapshot:r2").unwrap(), None);
    assert_eq!(store.marker("summary:s1").unwrap().as_deref(), Some("kept"), "other markers stay");
}
