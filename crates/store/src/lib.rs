//! SQLite storage: the single owner of the app's mutable state.
//!
//! One [`Store`] is opened at start-up and shared behind an `Arc` by the engine's worker
//! threads and the UI's background reads. Every change that touches more than one row is one
//! transaction, so two threads never lose each other's updates. Large payloads live in a
//! content-addressed blob directory next to the database.
//!
//! | Module | What it keeps |
//! | --- | --- |
//! | `sessions` | Chat sessions, their messages, todo lists and changed files |
//! | `search` | The chat palette's full-text search with facet counts |
//! | `ledger` | One row per model call, and the spend sums of Home |
//! | `issues` | Issues, their keys and activity feed |
//! | `runs` | Pipeline runs, their logs and renders |
//! | `reviews`, `inbox` | Variants waiting for a person; the Inbox |
//! | `catalog` | Deployed pipeline versions, spawned agents, permission rules, autopilots |
//! | `settings` | Projects, app settings, one-off markers |
//! | `blobs` | Content-addressed files |

mod blobs;
mod catalog;
mod codec;
mod db;
mod inbox;
mod issues;
mod ledger;
mod reviews;
mod runs;
mod schema;
mod search;
mod sessions;
mod settings;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use base::paths::Paths;
use rusqlite::{Connection, Transaction, TransactionBehavior};

pub use catalog::SpawnedRunnable;
pub use db::Db;
pub use ledger::SpendTotal;
pub use search::{FacetCounts, IndexStats, MessageSnippet, SearchResult, SessionHit, SessionQuery, StartedBy};
pub use sessions::{ParentFilter, SessionFilter};

/// The app's database and blob directory.
pub struct Store {
    db: Db,
    blobs: blobs::Blobs,
    /// The blob directory of an in-memory store, removed when the store is dropped.
    _scratch: Option<ScratchDir>,
}

impl Store {
    /// Opens the app's store under `paths`: creates the v2 directory, the database (or
    /// upgrades it) and the blob directory.
    pub fn open(paths: &Paths) -> Result<Self> {
        let v2 = paths.v2_dir();
        std::fs::create_dir_all(&v2).with_context(|| format!("create {}", v2.display()))?;
        let db = Db::open(&paths.db_file())?;
        let blobs = blobs::Blobs::new(paths.blobs_dir())?;
        Ok(Self { db, blobs, _scratch: None })
    }

    /// A private store for tests and previews: an in-memory database and a blob directory in
    /// the system temp directory that goes away with the store.
    pub fn in_memory() -> Result<Self> {
        let scratch = ScratchDir::new()?;
        let blobs = blobs::Blobs::new(scratch.0.join("blobs"))?;
        Ok(Self { db: Db::in_memory()?, blobs, _scratch: Some(scratch) })
    }

    /// How many schema steps the database has applied.
    pub fn schema_version(&self) -> Result<usize> {
        self.db.schema_version()
    }

    /// Runs `change` in one immediate transaction on the writer connection; nothing of it is
    /// kept unless it returns `Ok`.
    fn write<T>(&self, change: impl FnOnce(&Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.db.write();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = change(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Runs `query` on a read connection. Several statements see one snapshot.
    fn read<T>(&self, query: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.db.read();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let out = query(&tx)?;
        tx.finish()?;
        Ok(out)
    }
}

/// A temp directory removed on drop. `tempfile` is a test-only dependency, so the store keeps
/// its own.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("harness-store-{}", base::id::new_id()));
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
