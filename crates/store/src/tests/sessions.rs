use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use base::paths::Paths;
use model::{
    ContentBlock, FileChange, IssueKey, Message, ModelRef, Role, RunId, SessionId, SessionOrigin, SessionState,
    TodoItem, TodoStatus, Usage,
};

use super::{session, store, stored, workdir};
use crate::{ParentFilter, SessionFilter, Store};

#[test]
fn a_session_round_trips_every_field() {
    let store = store();
    let mut s = session("s1", "Fix double charge on retry", 5_000);
    s.parent = Some(SessionId::new("root"));
    s.depth = 1;
    s.mode = model::ChatMode::Plan;
    s.workdir = Some(workdir("payments-api"));
    s.state = SessionState::Waiting;
    s.origin = SessionOrigin::Run { run: RunId::new("r1"), node: "fixer".into(), variant: 2 };
    s.issue = Some(IssueKey::new("PAY-1423"));
    s.pinned = true;
    s.cost_usd = 0.21;
    s.usage = Usage { input: 10, output: 20, cache_read: 30, cache_write: 40 };
    s.context_tokens = 12_345;
    s.context_window = 200_000;
    s.forced_skills = vec!["rust".into(), "gpui".into()];
    s.pr_url = Some("https://github.com/acme/pay/pull/7".into());
    store.upsert_session(&s).unwrap();
    assert_eq!(store.session(&s.id).unwrap(), Some(s.clone()));
    assert_eq!(store.session(&SessionId::new("nope")).unwrap(), None);

    // A second upsert replaces the header.
    s.title = "Renamed".into();
    s.state = SessionState::Done;
    s.workdir = None;
    s.pr_url = None;
    store.upsert_session(&s).unwrap();
    assert_eq!(store.session(&s.id).unwrap(), Some(s));
}

#[test]
fn appending_messages_numbers_counts_and_touches_the_session() {
    let store = store();
    let s = session("s1", "Chat", 1_000);
    store.upsert_session(&s).unwrap();
    let mut reply = stored("ignored", 3_000);
    reply.message = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::text("Retries reuse one key."),
            ContentBlock::ToolUse { id: "t1".into(), name: "read_file".into(), input: serde_json::json!({"path": "a"}) },
        ],
    };
    reply.model = Some(ModelRef::parse("nr/cc/claude-opus-5-5"));
    reply.usage = Usage { input: 1, output: 2, cache_read: 3, cache_write: 4 };
    reply.cost_usd = 0.5;
    reply.seq = 99;

    let first = store.append_message(&s.id, stored("hello", 2_000)).unwrap();
    let second = store.append_message(&s.id, reply.clone()).unwrap();
    assert_eq!((first.seq, second.seq), (1, 2));
    assert_eq!(second, model::StoredMessage { seq: 2, ..reply });

    let header = store.session(&s.id).unwrap().unwrap();
    assert_eq!(header.message_count, 2);
    assert_eq!(header.updated, 3_000);

    // A stale copy of the header cannot undo the count or move `updated` back.
    store.upsert_session(&s).unwrap();
    let header = store.session(&s.id).unwrap().unwrap();
    assert_eq!((header.message_count, header.updated), (2, 3_000));

    assert_eq!(store.messages(&s.id, 1..3).unwrap(), vec![first.clone(), second.clone()]);
    assert_eq!(store.messages(&s.id, 2..10).unwrap(), vec![second]);
    assert!(store.messages(&s.id, 3..10).unwrap().is_empty());
    // `u64::MAX` means "to the end".
    assert_eq!(store.messages(&s.id, 1..u64::MAX).unwrap().len(), 2);
    assert_eq!(store.messages(&s.id, 2..u64::MAX).unwrap().len(), 1);

    assert!(store.append_message(&SessionId::new("missing"), stored("x", 1)).is_err());
}

#[test]
fn compaction_hides_messages_from_the_model_only() {
    let store = store();
    let s = session("s1", "Chat", 1_000);
    store.upsert_session(&s).unwrap();
    for i in 0..5 {
        store.append_message(&s.id, stored(&format!("m{i}"), 1_000 + i)).unwrap();
    }
    assert_eq!(store.compact_messages(&s.id, 3).unwrap(), 3);
    assert_eq!(store.compact_messages(&s.id, 3).unwrap(), 0);
    let seqs: Vec<u64> = store.model_messages(&s.id).unwrap().iter().map(|m| m.seq).collect();
    assert_eq!(seqs, vec![4, 5]);
    let all = store.messages(&s.id, 1..100).unwrap();
    assert_eq!(all.len(), 5);
    assert!(all[..3].iter().all(|m| m.compacted) && !all[3].compacted);
}

#[test]
fn deleting_a_session_takes_its_messages_and_keeps_its_children() {
    let store = store();
    let parent = session("p", "Parent", 1_000);
    let mut child = session("c", "Child", 2_000);
    child.parent = Some(parent.id.clone());
    store.upsert_session(&parent).unwrap();
    store.upsert_session(&child).unwrap();
    store.append_message(&parent.id, stored("unique zebra words", 1_500)).unwrap();

    assert!(store.delete_session(&parent.id).unwrap());
    assert!(!store.delete_session(&parent.id).unwrap());
    assert!(store.messages(&parent.id, 0..100).unwrap().is_empty());
    assert!(store.session(&child.id).unwrap().is_some());
    let count: i64 = store.db.read().query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 0);
    // The full-text index forgot the messages too.
    let hits = store.search_sessions(&crate::SessionQuery { text: "zebra".into(), ..Default::default() }).unwrap();
    assert_eq!(hits.total, 0);
}

#[test]
fn listing_filters_orders_and_pages() {
    let store = store();
    let mut a = session("a", "A", 1_000);
    a.pinned = true;
    let mut b = session("b", "B", 3_000);
    b.state = SessionState::Running;
    let mut c = session("c", "C", 2_000);
    c.archived = true;
    let mut d = session("d", "D", 4_000);
    d.parent = Some(SessionId::new("b"));
    for s in [&a, &b, &c, &d] {
        store.upsert_session(s).unwrap();
    }
    let ids = |filter: SessionFilter| -> Vec<String> {
        store.sessions(&filter).unwrap().into_iter().map(|s| s.id.0).collect()
    };
    assert_eq!(ids(SessionFilter::default()), ["d", "b", "c", "a"]);
    assert_eq!(ids(SessionFilter { archived: Some(false), ..Default::default() }), ["d", "b", "a"]);
    assert_eq!(ids(SessionFilter { archived: Some(true), ..Default::default() }), ["c"]);
    assert_eq!(ids(SessionFilter { pinned: Some(true), ..Default::default() }), ["a"]);
    assert_eq!(ids(SessionFilter { state: Some(SessionState::Running), ..Default::default() }), ["b"]);
    assert_eq!(ids(SessionFilter { parent: ParentFilter::TopLevel, ..Default::default() }), ["b", "c", "a"]);
    assert_eq!(ids(SessionFilter { parent: ParentFilter::Of("b".into()), ..Default::default() }), ["d"]);
    assert_eq!(ids(SessionFilter { limit: Some(2), ..Default::default() }), ["d", "b"]);
    assert_eq!(ids(SessionFilter { limit: Some(2), offset: 2, ..Default::default() }), ["c", "a"]);
    assert_eq!(ids(SessionFilter { offset: 3, ..Default::default() }), ["a"]);
    let children: Vec<String> = store.child_sessions(&b.id).unwrap().into_iter().map(|s| s.id.0).collect();
    assert_eq!(children, ["d"]);
}

#[test]
fn sessions_are_found_by_their_legacy_id() {
    let store = store();
    let s = session("new", "Imported", 1_000);
    store.upsert_session(&s).unwrap();
    store.set_session_legacy_id(&s.id, "old-123").unwrap();
    assert_eq!(store.session_by_legacy_id("old-123").unwrap().map(|s| s.id), Some(s.id.clone()));
    assert_eq!(store.session_by_legacy_id("old-999").unwrap(), None);
    assert!(store.set_session_legacy_id(&SessionId::new("missing"), "x").is_err());
    // Two sessions cannot claim the same old session.
    store.upsert_session(&session("other", "Other", 1_000)).unwrap();
    assert!(store.set_session_legacy_id(&SessionId::new("other"), "old-123").is_err());
}

#[test]
fn todos_and_file_changes_are_kept_per_session() {
    let store = store();
    let s = session("s1", "Chat", 1_000);
    store.upsert_session(&s).unwrap();
    assert!(store.todos(&s.id).unwrap().is_empty());
    let todos = vec![
        TodoItem { text: "write test".into(), status: TodoStatus::Done },
        TodoItem { text: "fix".into(), status: TodoStatus::InProgress },
    ];
    let files = vec![FileChange { path: "src/a.rs".into(), added: 3, removed: 1 }];
    store.set_todos(&s.id, &todos).unwrap();
    store.set_file_changes(&s.id, &files).unwrap();
    // Re-saving the header keeps them.
    store.upsert_session(&s).unwrap();
    assert_eq!(store.todos(&s.id).unwrap(), todos);
    assert_eq!(store.file_changes(&s.id).unwrap(), files);
    assert!(store.set_todos(&SessionId::new("missing"), &todos).is_err());
    assert!(store.todos(&SessionId::new("missing")).unwrap().is_empty());
}

#[test]
fn tilde_paths_are_refused() {
    let store = store();
    let mut s = session("s1", "Chat", 1_000);
    let mut w = workdir("app");
    w.path = PathBuf::from("~/code/app");
    s.workdir = Some(w);
    assert!(store.upsert_session(&s).is_err());
    assert!(store.session(&s.id).unwrap().is_none());
}

#[test]
fn a_row_that_does_not_decode_names_its_table_and_id() {
    let store = store();
    store.upsert_session(&session("bad", "Chat", 1_000)).unwrap();
    store.db.write().execute("UPDATE sessions SET state = 'sleeping' WHERE id = 'bad'", []).unwrap();
    let err = format!("{:#}", store.session(&SessionId::new("bad")).unwrap_err());
    assert!(err.contains("sessions row bad") && err.contains("state"), "{err}");
    store.db.write().execute("UPDATE sessions SET state = 'idle', usage = '{' WHERE id = 'bad'", []).unwrap();
    let err = format!("{:#}", store.sessions(&SessionFilter::default()).unwrap_err());
    assert!(err.contains("sessions row bad") && err.contains("usage"), "{err}");
}

#[test]
fn eight_threads_append_without_duplicates_or_losses() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(&Paths::under(dir.path())).unwrap());
    let s = session("busy", "Busy", 0);
    store.upsert_session(&s).unwrap();
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let store = Arc::clone(&store);
            let id = s.id.clone();
            std::thread::spawn(move || {
                let mut seqs = Vec::new();
                for i in 0..50 {
                    seqs.push(store.append_message(&id, stored(&format!("t{t} m{i}"), i)).unwrap().seq);
                    // Readers run alongside the writers.
                    let _ = store.session(&id).unwrap();
                }
                seqs
            })
        })
        .collect();
    let seqs: Vec<u64> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    let unique: BTreeSet<u64> = seqs.iter().copied().collect();
    assert_eq!(unique.len(), 400);
    assert_eq!(unique.iter().copied().collect::<Vec<_>>(), (1..=400).collect::<Vec<_>>());
    assert_eq!(store.session(&s.id).unwrap().unwrap().message_count, 400);
    assert_eq!(store.messages(&s.id, 1..1000).unwrap().len(), 400);
}
