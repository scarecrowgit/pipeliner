//! SQLite-backed state with an append-only event log and change notifications.
//!
//! All entities are stored as JSON documents keyed by (kind, id) with an optional
//! `parent` (thread id for messages, run id for tasks, gate id for variants) and a
//! timestamp for ordering. Writers call `put_*`; every write notifies subscribers
//! with a [`Change`]. Writes from other processes are picked up by polling
//! `PRAGMA data_version` (see [`Store::watch_external`]).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use crate::model::*;

/// What changed. UI views re-query what they show when they see a relevant change.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Agent(Id),
    Pipeline(Id),
    Run(Id),
    Task {
        id: Id,
        run_id: Option<Id>,
    },
    Chat(Id),
    Message {
        thread_id: Id,
        id: Id,
    },
    Variant {
        gate_task_id: Id,
    },
    Decision {
        task_id: Id,
    },
    Artifact,
    Event,
    /// Another process wrote to the database: re-query everything.
    External,
    Config,
}

pub struct Store {
    conn: Mutex<Connection>,
    subscribers: Mutex<Vec<async_channel::Sender<Change>>>,
}

trait Doc: Serialize + DeserializeOwned {
    const KIND: &'static str;
    fn doc_id(&self) -> &str;
    fn parent(&self) -> Option<&str> {
        None
    }
    fn ts(&self) -> i64;
    fn change(&self) -> Change;
}

macro_rules! ts {
    ($e:expr) => {
        $e.timestamp_millis()
    };
}

impl Doc for AgentDef {
    const KIND: &'static str = "agent";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Agent(self.id.clone())
    }
}
impl Doc for Pipeline {
    const KIND: &'static str = "pipeline";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Pipeline(self.id.clone())
    }
}
impl Doc for Run {
    const KIND: &'static str = "run";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        Some(&self.pipeline_id)
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Run(self.id.clone())
    }
}
impl Doc for Task {
    const KIND: &'static str = "task";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Task {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
        }
    }
}
impl Doc for Chat {
    const KIND: &'static str = "chat";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn ts(&self) -> i64 {
        ts!(self.updated_at)
    }
    fn change(&self) -> Change {
        Change::Chat(self.id.clone())
    }
}
impl Doc for Message {
    const KIND: &'static str = "message";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        Some(&self.thread_id)
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Message {
            thread_id: self.thread_id.clone(),
            id: self.id.clone(),
        }
    }
}
impl Doc for Variant {
    const KIND: &'static str = "variant";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        Some(&self.gate_task_id)
    }
    fn ts(&self) -> i64 {
        self.index as i64
    }
    fn change(&self) -> Change {
        Change::Variant {
            gate_task_id: self.gate_task_id.clone(),
        }
    }
}
impl Doc for Decision {
    const KIND: &'static str = "decision";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        Some(&self.task_id)
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Decision {
            task_id: self.task_id.clone(),
        }
    }
}
impl Doc for Artifact {
    const KIND: &'static str = "artifact";
    fn doc_id(&self) -> &str {
        &self.id
    }
    fn parent(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
    fn ts(&self) -> i64 {
        ts!(self.created_at)
    }
    fn change(&self) -> Change {
        Change::Artifact
    }
}

impl Store {
    /// Opens (or creates) the database at the default location.
    pub fn open_default() -> Result<Arc<Self>> {
        let dir = crate::paths::data_dir();
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir.join("pipeliner.db"))
    }

    pub fn open(path: &Path) -> Result<Arc<Self>> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Arc<Self>> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Arc<Self>> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS docs (
                kind TEXT NOT NULL,
                id TEXT NOT NULL,
                parent TEXT,
                ts INTEGER NOT NULL,
                data TEXT NOT NULL,
                PRIMARY KEY (kind, id)
             );
             CREATE INDEX IF NOT EXISTS docs_parent ON docs(kind, parent, ts);
             CREATE INDEX IF NOT EXISTS docs_ts ON docs(kind, ts);
             CREATE TABLE IF NOT EXISTS events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                kind TEXT NOT NULL,
                run_id TEXT,
                task_id TEXT,
                text TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS counters (name TEXT PRIMARY KEY, value INTEGER NOT NULL);",
        )?;
        Ok(Arc::new(Store {
            conn: Mutex::new(conn),
            subscribers: Mutex::new(vec![]),
        }))
    }

    /// Folder of the database file (`None` for an in-memory store).
    pub fn dir(&self) -> Option<std::path::PathBuf> {
        let c = self.conn.lock();
        c.path()
            .filter(|p| !p.is_empty())
            .and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_path_buf()))
    }

    /// Receive every change. Dropping the receiver unsubscribes.
    pub fn subscribe(&self) -> async_channel::Receiver<Change> {
        let (tx, rx) = async_channel::unbounded();
        self.subscribers.lock().push(tx);
        rx
    }

    fn notify(&self, change: Change) {
        self.subscribers
            .lock()
            .retain(|s| s.try_send(change.clone()).is_ok());
    }

    /// Polls for writes from other processes once a second and emits [`Change::External`].
    pub fn watch_external(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let path = {
            let c = self.conn.lock();
            c.path().map(|p| p.to_string())
        };
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            return;
        };
        std::thread::Builder::new()
            .name("store-watch".into())
            .spawn(move || {
                // A separate connection sees other connections' (and processes') commits
                // through data_version; our own writes also bump it, so compare against the
                // count of local writes.
                let Ok(watch) = Connection::open(&path) else {
                    return;
                };
                let mut last: i64 = -1;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(1000));
                    let Some(store) = weak.upgrade() else { return };
                    let v: i64 = watch
                        .query_row("PRAGMA data_version", [], |r| r.get(0))
                        .unwrap_or(last);
                    if last >= 0 && v != last {
                        store.notify(Change::External);
                    }
                    last = v;
                }
            })
            .ok();
    }

    // ---- generic -------------------------------------------------------------

    fn put<T: Doc>(&self, doc: &T) -> Result<()> {
        let data = serde_json::to_string(doc)?;
        self.conn.lock().execute(
            "INSERT INTO docs(kind,id,parent,ts,data) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(kind,id) DO UPDATE SET parent=excluded.parent, ts=excluded.ts, data=excluded.data",
            params![T::KIND, doc.doc_id(), doc.parent(), doc.ts(), data],
        )?;
        self.notify(doc.change());
        Ok(())
    }

    fn get<T: Doc>(&self, id: &str) -> Option<T> {
        let c = self.conn.lock();
        let data: Option<String> = c
            .query_row(
                "SELECT data FROM docs WHERE kind=?1 AND id=?2",
                params![T::KIND, id],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        data.and_then(|d| serde_json::from_str(&d).ok())
    }

    fn all<T: Doc>(&self) -> Vec<T> {
        self.query::<T>(
            "SELECT data FROM docs WHERE kind=?1 ORDER BY ts",
            params![T::KIND],
        )
    }

    fn by_parent<T: Doc>(&self, parent: &str) -> Vec<T> {
        self.query::<T>(
            "SELECT data FROM docs WHERE kind=?1 AND parent=?2 ORDER BY ts",
            params![T::KIND, parent],
        )
    }

    fn query<T: Doc>(&self, sql: &str, p: impl rusqlite::Params) -> Vec<T> {
        let c = self.conn.lock();
        let Ok(mut stmt) = c.prepare_cached(sql) else {
            return vec![];
        };
        let rows = stmt.query_map(p, |r| r.get::<_, String>(0));
        match rows {
            Ok(rows) => rows
                .filter_map(|r| r.ok())
                .filter_map(|d| serde_json::from_str(&d).ok())
                .collect(),
            Err(_) => vec![],
        }
    }

    fn delete<T: Doc>(&self, id: &str, change: Change) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM docs WHERE kind=?1 AND id=?2",
            params![T::KIND, id],
        )?;
        self.notify(change);
        Ok(())
    }

    // ---- agents ----------------------------------------------------------------

    pub fn agents(&self) -> Vec<AgentDef> {
        self.all()
    }
    pub fn agent(&self, id: &str) -> Option<AgentDef> {
        self.get(id)
    }
    pub fn agent_by_name(&self, name: &str) -> Option<AgentDef> {
        self.agents()
            .into_iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
    }
    pub fn put_agent(&self, a: &AgentDef) -> Result<()> {
        self.put(a)
    }
    pub fn delete_agent(&self, id: &str) -> Result<()> {
        self.delete::<AgentDef>(id, Change::Agent(id.into()))
    }

    // ---- pipelines -------------------------------------------------------------

    pub fn pipelines(&self) -> Vec<Pipeline> {
        self.all::<Pipeline>()
            .into_iter()
            .filter(|p| !p.archived)
            .collect()
    }
    pub fn pipeline(&self, id: &str) -> Option<Pipeline> {
        self.get(id)
    }
    pub fn put_pipeline(&self, p: &Pipeline) -> Result<()> {
        self.put(p)
    }
    pub fn delete_pipeline(&self, id: &str) -> Result<()> {
        self.delete::<Pipeline>(id, Change::Pipeline(id.into()))
    }

    // ---- runs ------------------------------------------------------------------

    pub fn runs(&self) -> Vec<Run> {
        self.all()
    }
    pub fn runs_for_pipeline(&self, pipeline_id: &str) -> Vec<Run> {
        self.by_parent(pipeline_id)
    }
    pub fn run(&self, id: &str) -> Option<Run> {
        self.get(id)
    }
    pub fn put_run(&self, r: &Run) -> Result<()> {
        self.put(r)
    }
    /// Reads, modifies and writes a run atomically with respect to other `update_run` calls.
    pub fn update_run(&self, id: &str, f: impl FnOnce(&mut Run)) -> Result<Run> {
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock();
        let mut r: Run = self.get(id).context("run not found")?;
        f(&mut r);
        self.put(&r)?;
        Ok(r)
    }

    pub fn next_number(&self, counter: &str) -> u64 {
        let c = self.conn.lock();
        let _ = c.execute(
            "INSERT INTO counters(name,value) VALUES (?1,1) ON CONFLICT(name) DO UPDATE SET value=value+1",
            params![counter],
        );
        c.query_row(
            "SELECT value FROM counters WHERE name=?1",
            params![counter],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(1) as u64
    }

    // ---- tasks -----------------------------------------------------------------

    pub fn tasks(&self) -> Vec<Task> {
        self.all()
    }
    pub fn tasks_for_run(&self, run_id: &str) -> Vec<Task> {
        self.by_parent(run_id)
    }
    pub fn task(&self, id: &str) -> Option<Task> {
        self.get(id)
    }
    pub fn children(&self, parent_id: &str) -> Vec<Task> {
        self.tasks()
            .into_iter()
            .filter(|t| t.parent_id.as_deref() == Some(parent_id))
            .collect()
    }
    pub fn put_task(&self, t: &Task) -> Result<()> {
        self.put(t)
    }
    pub fn update_task(&self, id: &str, f: impl FnOnce(&mut Task)) -> Result<Task> {
        static LOCK: Mutex<()> = Mutex::new(());
        let _g = LOCK.lock();
        let mut t: Task = self.get(id).context("task not found")?;
        f(&mut t);
        self.put(&t)?;
        Ok(t)
    }
    /// Tasks waiting for you (review queue).
    pub fn waiting(&self) -> Vec<Task> {
        self.tasks()
            .into_iter()
            .filter(|t| t.status == Status::Waiting)
            .collect()
    }

    // ---- chats & messages ------------------------------------------------------

    pub fn chats(&self) -> Vec<Chat> {
        let mut v: Vec<Chat> = self
            .all::<Chat>()
            .into_iter()
            .filter(|c| !c.archived)
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        v
    }
    pub fn chat(&self, id: &str) -> Option<Chat> {
        self.get(id)
    }
    pub fn put_chat(&self, c: &Chat) -> Result<()> {
        self.put(c)
    }
    pub fn messages(&self, thread_id: &str) -> Vec<Message> {
        self.by_parent(thread_id)
    }
    pub fn message(&self, id: &str) -> Option<Message> {
        self.get(id)
    }
    pub fn put_message(&self, m: &Message) -> Result<()> {
        self.put(m)
    }
    /// User messages posted into a thread that the agent hasn't seen yet (steering).
    pub fn take_undelivered(&self, thread_id: &str) -> Vec<Message> {
        let mut out = vec![];
        for mut m in self.messages(thread_id) {
            if m.role == Role::User && !m.delivered {
                m.delivered = true;
                let _ = self.put(&m);
                out.push(m);
            }
        }
        out
    }

    // ---- variants, decisions, artifacts ----------------------------------------

    pub fn variants(&self, gate_task_id: &str) -> Vec<Variant> {
        self.by_parent(gate_task_id)
    }
    pub fn put_variant(&self, v: &Variant) -> Result<()> {
        self.put(v)
    }
    pub fn decisions(&self) -> Vec<Decision> {
        self.all()
    }
    pub fn decisions_for(&self, task_id: &str) -> Vec<Decision> {
        self.by_parent(task_id)
    }
    pub fn put_decision(&self, d: &Decision) -> Result<()> {
        self.put(d)
    }
    /// The oldest unconsumed decision for a task, marking it consumed.
    pub fn take_decision(&self, task_id: &str) -> Option<Decision> {
        let mut d = self
            .decisions_for(task_id)
            .into_iter()
            .find(|d| !d.consumed)?;
        d.consumed = true;
        self.put(&d).ok()?;
        Some(d)
    }
    pub fn artifacts(&self) -> Vec<Artifact> {
        self.all()
    }
    pub fn artifacts_for_run(&self, run_id: &str) -> Vec<Artifact> {
        self.by_parent(run_id)
    }
    pub fn put_artifact(&self, a: &Artifact) -> Result<()> {
        self.put(a)
    }

    // ---- event log -------------------------------------------------------------

    pub fn log(
        &self,
        kind: &str,
        run_id: Option<&str>,
        task_id: Option<&str>,
        text: impl Into<String>,
    ) {
        let text = text.into();
        let _ = self.conn.lock().execute(
            "INSERT INTO events(ts,kind,run_id,task_id,text) VALUES (?1,?2,?3,?4,?5)",
            params![now().to_rfc3339(), kind, run_id, task_id, text],
        );
        self.notify(Change::Event);
    }

    /// Most recent events first.
    pub fn events(&self, limit: usize) -> Vec<Event> {
        let c = self.conn.lock();
        let Ok(mut stmt) = c.prepare(
            "SELECT seq,ts,kind,run_id,task_id,text FROM events ORDER BY seq DESC LIMIT ?1",
        ) else {
            return vec![];
        };
        let rows = stmt.query_map(params![limit as i64], |r| {
            let ts: String = r.get(1)?;
            Ok(Event {
                seq: r.get(0)?,
                ts: chrono::DateTime::parse_from_rfc3339(&ts)
                    .map(|d| d.to_utc())
                    .unwrap_or_else(|_| now()),
                kind: r.get(2)?,
                run_id: r.get(3)?,
                task_id: r.get(4)?,
                text: r.get(5)?,
            })
        });
        rows.map(|r| r.filter_map(|e| e.ok()).collect())
            .unwrap_or_default()
    }

    pub fn emit(&self, change: Change) {
        self.notify(change);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_notify() {
        let s = Store::open_in_memory().unwrap();
        let rx = s.subscribe();
        let mut t = Task::new(TaskKind::Agent, "x");
        t.run_id = Some("r1".into());
        s.put_task(&t).unwrap();
        assert_eq!(s.tasks_for_run("r1").len(), 1);
        assert!(matches!(rx.try_recv().unwrap(), Change::Task { .. }));
        s.update_task(&t.id, |t| t.status = Status::Done).unwrap();
        assert_eq!(s.task(&t.id).unwrap().status, Status::Done);
        assert_eq!(s.next_number("run"), 1);
        assert_eq!(s.next_number("run"), 2);
        s.log("test", None, None, "hello");
        assert_eq!(s.events(10)[0].text, "hello");
    }
}
