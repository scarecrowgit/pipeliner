//! What a pipeline run needs to continue after a restart: the graph it executes and the
//! executor's snapshot, both as JSON text the engine writes and reads.

use anyhow::Result;
use model::RunId;
use rusqlite::{OptionalExtension as _, params};

use crate::Store;

/// The two columns of `run_state`.
#[derive(Clone, Copy)]
enum Part {
    Graph,
    Snapshot,
}

impl Part {
    fn column(self) -> &'static str {
        match self {
            Part::Graph => "graph",
            Part::Snapshot => "snapshot",
        }
    }
}

impl Store {
    /// Keeps the graph a run executes, so it stays on its version when the draft changes.
    /// The run's row need not exist yet.
    pub fn set_run_graph(&self, run: &RunId, graph: &str) -> Result<()> {
        self.set_run_part(run, Part::Graph, graph)
    }

    /// The graph a run executes, as kept by [`Store::set_run_graph`].
    pub fn run_graph(&self, run: &RunId) -> Result<Option<String>> {
        self.run_part(run, Part::Graph)
    }

    /// Keeps the executor's latest snapshot of a run.
    pub fn set_run_snapshot(&self, run: &RunId, snapshot: &str) -> Result<()> {
        self.set_run_part(run, Part::Snapshot, snapshot)
    }

    /// The executor's latest snapshot of a run.
    pub fn run_snapshot(&self, run: &RunId) -> Result<Option<String>> {
        self.run_part(run, Part::Snapshot)
    }

    fn set_run_part(&self, run: &RunId, part: Part, text: &str) -> Result<()> {
        let column = part.column();
        let sql = format!(
            "INSERT INTO run_state (run, {column}) VALUES (?1, ?2)
             ON CONFLICT (run) DO UPDATE SET {column} = excluded.{column}"
        );
        self.write(|tx| {
            tx.prepare_cached(&sql)?.execute(params![run.as_str(), text])?;
            Ok(())
        })
    }

    fn run_part(&self, run: &RunId, part: Part) -> Result<Option<String>> {
        let sql = format!("SELECT {} FROM run_state WHERE run = ?1", part.column());
        self.read(|conn| {
            let found: Option<Option<String>> =
                conn.prepare_cached(&sql)?.query_row([run.as_str()], |row| row.get(0)).optional()?;
            Ok(found.flatten())
        })
    }
}
