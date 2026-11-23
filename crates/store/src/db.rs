//! The database handle: one writer connection behind a mutex, WAL so readers never block it.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context as _, Result};
use rusqlite::Connection;

/// Schema steps in order. A database's `user_version` is the number of steps applied to it,
/// so a step is never edited after it ships: add a new one instead.
const MIGRATIONS: &[&str] = &[
    // 1: bookkeeping for the app itself (schema notes, one-time import markers).
    "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;",
];

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    /// Opens (and creates or upgrades) the database file.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::prepare(conn)
    }

    /// A private in-memory database, for tests and previews.
    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// The writer connection. A poisoned lock is still usable: SQLite rolled back whatever
    /// the panicking thread had open.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn schema_version(&self) -> Result<usize> {
        Ok(self.conn().pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? as usize)
    }
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let applied = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? as usize;
    anyhow::ensure!(
        applied <= MIGRATIONS.len(),
        "the database is at schema {applied}, newer than this build ({})",
        MIGRATIONS.len()
    );
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql).with_context(|| format!("migration {}", index + 1))?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
        tx.commit()?;
    }
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
        let dir = std::env::temp_dir().join(format!("harness-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reopen.db");
        let _ = std::fs::remove_file(&path);
        {
            let db = Db::open(&path).unwrap();
            db.conn().execute("INSERT INTO meta (key, value) VALUES ('k', 'v')", []).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let value: String =
            db.conn().query_row("SELECT value FROM meta WHERE key = 'k'", [], |row| row.get(0)).unwrap();
        assert_eq!(value, "v");
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The chat palette needs full-text search; this fails loudly if the bundled SQLite
    /// was built without FTS5.
    #[test]
    fn the_bundled_sqlite_has_fts5() {
        let db = Db::in_memory().unwrap();
        let conn = db.conn();
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
