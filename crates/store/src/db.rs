//! The database handle: one writer connection behind a mutex, WAL so readers never block it,
//! and a few read connections so a long search does not hold up the engine's writes.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::schema::MIGRATIONS;

/// Read connections of a file database. Reads are short; a handful covers the UI's background
/// reads and the engine's workers.
const READERS: usize = 4;

/// How long a statement waits for a lock held by another connection before it fails.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The SQLite connections of one database.
pub struct Db {
    writer: Mutex<Connection>,
    /// Empty for an in-memory database, whose only connection is the writer.
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
}

impl Db {
    /// Opens (and creates or upgrades) the database file.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        // First, so that switching to WAL and migrating wait for another process opening the
        // same new database instead of failing with SQLITE_BUSY.
        conn.busy_timeout(BUSY_TIMEOUT)?;
        enable_wal(&conn)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let mut db = Self::prepare(conn)?;
        for _ in 0..READERS {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
            )
            .with_context(|| format!("open a reader of {}", path.display()))?;
            reader.busy_timeout(BUSY_TIMEOUT)?;
            reader.pragma_update(None, "query_only", "ON")?;
            db.readers.push(Mutex::new(reader));
        }
        Ok(db)
    }

    /// A private in-memory database, for tests and previews.
    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { writer: Mutex::new(conn), readers: Vec::new(), next_reader: AtomicUsize::new(0) })
    }

    /// The writer connection. A poisoned lock is still usable: SQLite rolled back whatever
    /// the panicking thread had open.
    pub fn write(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A connection for reading: a free reader when one is, otherwise the next one in turn.
    /// An in-memory database reads through its writer.
    pub fn read(&self) -> MutexGuard<'_, Connection> {
        if self.readers.is_empty() {
            return self.write();
        }
        let start = self.next_reader.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        for offset in 0..self.readers.len() {
            if let Ok(guard) = self.readers[(start + offset) % self.readers.len()].try_lock() {
                return guard;
            }
        }
        self.readers[start].lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many schema steps the database has applied.
    pub fn schema_version(&self) -> Result<usize> {
        Ok(self.write().pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? as usize)
    }
}

/// Switches the file to WAL. Two processes converting a new file at once can get SQLITE_BUSY
/// without the busy timeout applying (SQLite refuses to wait where waiting would deadlock), so
/// the switch is retried until the timeout is spent.
fn enable_wal(conn: &Connection) -> Result<()> {
    let deadline = Instant::now() + BUSY_TIMEOUT;
    loop {
        match conn.pragma_update(None, "journal_mode", "WAL") {
            Ok(()) => return Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::DatabaseBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err).context("switch to WAL"),
        }
    }
}

/// Applies the schema steps the database lacks, in one immediate transaction that also reads
/// `user_version`: a second process opening the same new file waits for the lock, then finds
/// the work done, instead of running the steps again.
fn migrate(conn: &mut Connection) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let applied = tx.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? as usize;
    anyhow::ensure!(
        applied <= MIGRATIONS.len(),
        "the database is at schema {applied}, newer than this build ({})",
        MIGRATIONS.len()
    );
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        tx.execute_batch(sql).with_context(|| format!("migration {}", index + 1))?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_database_is_at_the_latest_schema() {
        let db = Db::in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn reopening_a_file_applies_nothing_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reopen.db");
        {
            let db = Db::open(&path).unwrap();
            db.write().execute("INSERT INTO meta (key, value) VALUES ('k', 'v')", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let value: String =
            db.read().query_row("SELECT value FROM meta WHERE key = 'k'", [], |row| row.get(0)).unwrap();
        assert_eq!(value, "v");
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
    }

    /// Two processes starting at once both open a database that does not exist yet.
    #[test]
    fn two_threads_opening_a_new_file_both_succeed() {
        for round in 0..20 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("race-{round}.db"));
            let barrier = std::sync::Barrier::new(2);
            let results: Vec<Result<usize>> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..2)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            Db::open(&path).and_then(|db| db.schema_version())
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for result in results {
                assert_eq!(result.unwrap(), MIGRATIONS.len(), "round {round}");
            }
        }
    }

    #[test]
    fn readers_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("ro.db")).unwrap();
        assert!(db.read().execute("INSERT INTO meta (key, value) VALUES ('k', 'v')", []).is_err());
    }

    /// The chat palette needs full-text search; this fails loudly if the bundled SQLite
    /// was built without FTS5.
    #[test]
    fn the_bundled_sqlite_has_fts5() {
        let db = Db::in_memory().unwrap();
        let conn = db.write();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(body);
             INSERT INTO probe (body) VALUES ('fix double charge on retry'), ('bump gateway sdk');",
        )
        .unwrap();
        let hits: i64 = conn
            .query_row("SELECT count(*) FROM probe WHERE probe MATCH 'retry'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(hits, 1);
    }
}
