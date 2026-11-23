//! Import tests: hand-written old files in temp directories, an in-memory store.

mod inbox;
mod issues;
mod report;
mod runs;
mod sessions;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base::paths::Paths;
use serde_json::{Value, json};

use super::{ImportOptions, ImportReport, import_v1};
use crate::Store;

/// An old data directory, the paths pointing at it and an empty store.
pub(super) struct Fixture {
    _dir: tempfile::TempDir,
    pub(super) paths: Paths,
    pub(super) store: Store,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(paths.v1_data_dir()).unwrap();
        Self {
            _dir: dir,
            paths,
            store: Store::in_memory().unwrap(),
        }
    }

    pub(super) fn data(&self) -> PathBuf {
        self.paths.v1_data_dir()
    }

    /// Writes a file under the old data directory.
    pub(super) fn write(&self, rel: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.data().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    pub(super) fn write_json(&self, rel: &str, value: &Value) -> PathBuf {
        self.write(rel, serde_json::to_vec_pretty(value).unwrap())
    }

    /// Sets a file's modification time to `ms` after the epoch.
    pub(super) fn touch(&self, path: &Path, ms: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_millis(ms)).unwrap();
    }

    pub(super) fn import(&self) -> ImportReport {
        import_v1(&self.store, &self.paths, &ImportOptions::default()).unwrap()
    }

    pub(super) fn dry_import(&self) -> ImportReport {
        import_v1(&self.store, &self.paths, &ImportOptions { dry_run: true }).unwrap()
    }

    /// Every file under the old data directory with its size and modification time.
    pub(super) fn snapshot(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64, SystemTime)>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let meta = entry.metadata().unwrap();
                out.push((entry.path(), meta.len(), meta.modified().unwrap()));
                if meta.is_dir() {
                    walk(&entry.path(), out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.data(), &mut out);
        out
    }
}

/// A PNG header of the given size (enough for the importer; not a decodable image). `tag`
/// makes pictures differ.
pub(super) fn png(width: u32, height: u32, tag: u8) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, tag]);
    bytes
}

pub(super) fn zero_usage() -> Value {
    json!({"input_tokens": 0, "output_tokens": 0, "cache_read_tokens": 0, "cache_write_tokens": 0})
}

/// A saved session as the old app wrote it.
pub(super) fn old_session(id: &str, messages: Value) -> Value {
    json!({
        "id": id,
        "title": format!("Session {id}"),
        "model": "nr/cc/claude-opus-5-5",
        "cost_usd": 0.0,
        "last_usage": null,
        "usage": zero_usage(),
        "messages": messages,
        "loaded_tools": [],
        "mode": "build",
        "agent": null,
        "forced_skills": [],
        "workdir": null,
        "work_mode": "in_place",
    })
}

pub(super) fn user_text(text: &str) -> Value {
    json!({"role": "user", "content": [{"Text": text}]})
}

pub(super) fn assistant_text(text: &str) -> Value {
    json!({"role": "assistant", "content": [{"Text": text}]})
}

/// An issue as the old app wrote it.
pub(super) fn old_issue(key: &str, status: &str, run: Option<&str>) -> Value {
    json!({
        "key": key,
        "title": format!("Title of {key}"),
        "body": "Body text",
        "kind": "Feature",
        "created": 1_700_000_000,
        "status": status,
        "pipeline": "mockup-gpui-build",
        "run": run,
        "pr_url": null,
    })
}

pub(super) fn old_stage(id: &str, name: &str, kind: &str, status: &str) -> Value {
    json!({"id": id, "name": name, "kind": kind, "status": status, "started": 1_700_000_100, "finished": 1_700_000_200, "summary": "one line"})
}

/// A run as the old app wrote it, with two stages and a log.
pub(super) fn old_run(id: &str, issue: &str, status: &str) -> Value {
    json!({
        "id": id,
        "issue": issue,
        "created": 1_700_000_050,
        "status": status,
        "pipeline": "mockup-gpui-build",
        "pipeline_name": "Mockup to GPUI build",
        "stages": [old_stage("triage", "Triage", "decision", "done"), old_stage("build", "Build", "agent", "done")],
        "notes": {},
        "renders": [],
        "pr_url": null,
        "error": null,
        "tokens": 0,
        "cost_usd": 0.0,
        "usage": [],
        "log": [],
    })
}
