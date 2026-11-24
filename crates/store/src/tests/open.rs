//! Opening, reopening, settings, markers, projects and blobs.

use std::path::PathBuf;

use base::paths::Paths;
use model::{BlobId, ModelRef, Project, ProjectId, SessionId, Settings};

use super::{session, store, stored};
use crate::Store;
use crate::schema::MIGRATIONS;

#[test]
fn opening_creates_the_directories_and_reopening_keeps_the_data() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let blob;
    {
        let store = Store::open(&paths).unwrap();
        assert!(paths.db_file().is_file());
        assert!(paths.blobs_dir().is_dir());
        store.upsert_session(&session("s1", "Kept across restarts", 1_000)).unwrap();
        store.append_message(&SessionId::new("s1"), stored("persisted words", 2_000)).unwrap();
        store.next_issue_key("HAR").unwrap();
        store.set_marker("v1_import_done", "yes").unwrap();
        blob = store.put_blob(b"image bytes").unwrap();
    }
    let store = Store::open(&paths).unwrap();
    assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len());
    let s = store.session(&SessionId::new("s1")).unwrap().unwrap();
    assert_eq!((s.title.as_str(), s.message_count), ("Kept across restarts", 1));
    let hits = store.search_sessions(&crate::SessionQuery { text: "persisted".into(), ..Default::default() }).unwrap();
    assert_eq!(hits.total, 1);
    assert_eq!(store.next_issue_key("HAR").unwrap().as_str(), "HAR-2");
    assert_eq!(store.marker("v1_import_done").unwrap().as_deref(), Some("yes"));
    assert_eq!(store.read_blob(&blob).unwrap(), b"image bytes");
}

#[test]
fn an_in_memory_store_cleans_up_its_blob_directory() {
    let store = store();
    let id = store.put_blob(b"x").unwrap();
    let path = store.blob_path(&id).unwrap();
    assert!(path.is_file());
    drop(store);
    assert!(!path.exists());
}

#[test]
fn settings_start_from_defaults() {
    let store = store();
    assert_eq!(store.settings().unwrap(), Settings::default());
    let settings = Settings {
        slots: 6,
        daily_limit_usd: Some(20.0),
        default_model: Some(ModelRef::parse("nr/cc/claude-opus-5-5")),
        current_project: Some(ProjectId::new("harness")),
        sidebar_collapsed: true,
        compact_at: 0.8,
        worktree_retention_days: 3,
    };
    store.set_settings(&settings).unwrap();
    assert_eq!(store.settings().unwrap(), settings);
}

#[test]
fn markers_set_replace_and_remove() {
    let store = store();
    assert_eq!(store.marker("k").unwrap(), None);
    store.set_marker("k", "1").unwrap();
    store.set_marker("k", "2").unwrap();
    assert_eq!(store.marker("k").unwrap().as_deref(), Some("2"));
    assert!(store.remove_marker("k").unwrap());
    assert!(!store.remove_marker("k").unwrap());
}

#[test]
fn projects_round_trip() {
    let store = store();
    let harness = Project {
        id: ProjectId::new("harness"),
        name: "Harness".into(),
        key_prefix: "HAR".into(),
        repos: vec![PathBuf::from("/home/u/code/pipeliner")],
    };
    let pay = Project { id: ProjectId::new("pay"), name: "Payments".into(), key_prefix: "PAY".into(), repos: vec![] };
    store.upsert_project(&pay).unwrap();
    store.upsert_project(&harness).unwrap();
    assert_eq!(store.project(&harness.id).unwrap(), Some(harness.clone()));
    assert_eq!(store.projects().unwrap(), vec![harness.clone(), pay.clone()]);
    assert!(store.delete_project(&pay.id).unwrap());
    assert_eq!(store.projects().unwrap(), vec![harness.clone()]);

    let tilde = Project { repos: vec![PathBuf::from("~/code/x")], ..harness.clone() };
    assert!(store.upsert_project(&tilde).is_err());
    let no_prefix = Project { key_prefix: String::new(), ..harness };
    assert!(store.upsert_project(&no_prefix).is_err());
}

#[test]
fn blobs_are_content_addressed_and_idempotent() {
    let store = store();
    let a = store.put_blob(b"hello").unwrap();
    assert_eq!(a.as_str(), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    let again = store.put_blob(b"hello").unwrap();
    assert_eq!(a, again);
    let path = store.blob_path(&a).unwrap();
    assert!(path.ends_with(format!("2c/f2/{}", a.as_str())));
    assert!(store.blob_exists(&a).unwrap());
    assert_eq!(store.read_blob(&a).unwrap(), b"hello");

    let empty = store.put_blob(b"").unwrap();
    assert_eq!(store.read_blob(&empty).unwrap(), b"");

    // No temp files are left beside the blob.
    let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(siblings.len(), 1);

    let missing = BlobId::new("0".repeat(64));
    assert!(!store.blob_exists(&missing).unwrap());
    assert!(store.read_blob(&missing).is_err());
    for bad in ["../../etc/passwd", "ABC", "", &"g".repeat(64)] {
        assert!(store.blob_path(&BlobId::new(bad)).is_err(), "{bad:?}");
    }
}
