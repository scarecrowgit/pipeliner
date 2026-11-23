//! Pipeline runs with their node instances, logs and renders.

use anyhow::{Result, bail};
use model::{IssueKey, NodeId, PipelineId, Render, Run, RunId, RunLogLine};
use rusqlite::{OptionalExtension as _, Transaction, params};

use crate::Store;
use crate::codec::{Decoder, check_path, int, json, query_all, query_one, tag};

const RUN_COLUMNS: &str = "id, pipeline, version, issue, number, title, input, state, test, started, finished, \
     cost_usd, usage, error, pr_url, nodes";

const LOG_COLUMNS: &str = "run, seq, at, node, variant, level, text";

fn decode_run(d: &Decoder) -> Result<Run> {
    Ok(Run {
        id: RunId(d.string("id")?),
        pipeline: PipelineId(d.string("pipeline")?),
        version: d.u32("version")?,
        issue: d.opt::<IssueKey>("issue")?,
        number: d.u32("number")?,
        title: d.string("title")?,
        input: d.string("input")?,
        state: d.tag("state")?,
        test: d.get("test")?,
        started: d.get("started")?,
        finished: d.get("finished")?,
        cost_usd: d.get("cost_usd")?,
        usage: d.json("usage")?,
        error: d.get("error")?,
        pr_url: d.get("pr_url")?,
        nodes: d.json("nodes")?,
    })
}

fn decode_log(d: &Decoder) -> Result<RunLogLine> {
    Ok(RunLogLine {
        seq: d.u64("seq")?,
        at: d.get("at")?,
        node: d.opt::<NodeId>("node")?,
        variant: d.u32("variant")?,
        level: d.tag("level")?,
        text: d.string("text")?,
    })
}

fn require_run(tx: &Transaction, run: &RunId) -> Result<()> {
    if tx.query_row("SELECT 1 FROM runs WHERE id = ?1", [run.as_str()], |_| Ok(())).optional()?.is_none() {
        bail!("no run {run}");
    }
    Ok(())
}

impl Store {
    /// Creates a run or replaces it with its node instances. Logs and renders are kept.
    pub fn upsert_run(&self, run: &Run) -> Result<()> {
        for node in &run.nodes {
            if let Some(worktree) = &node.worktree {
                check_path(worktree, "node worktree")?;
            }
        }
        let (nodes, usage, state) = (json(&run.nodes)?, json(&run.usage)?, tag(&run.state)?);
        self.write(|tx| {
            tx.prepare_cached(&format!(
                "INSERT INTO runs ({RUN_COLUMNS}, open)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT (id) DO UPDATE SET pipeline = excluded.pipeline, version = excluded.version,
                    issue = excluded.issue, number = excluded.number, title = excluded.title,
                    input = excluded.input, state = excluded.state, test = excluded.test,
                    started = excluded.started, finished = excluded.finished, cost_usd = excluded.cost_usd,
                    usage = excluded.usage, error = excluded.error, pr_url = excluded.pr_url,
                    nodes = excluded.nodes, open = excluded.open"
            ))?
            .execute(params![
                run.id.as_str(),
                run.pipeline.as_str(),
                run.version,
                run.issue.as_ref().map(IssueKey::as_str),
                run.number,
                run.title,
                run.input,
                state,
                run.test,
                run.started,
                run.finished,
                run.cost_usd,
                usage,
                run.error,
                run.pr_url,
                nodes,
                run.state.is_open(),
            ])?;
            Ok(())
        })
    }

    /// One run.
    pub fn run(&self, id: &RunId) -> Result<Option<Run>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id = ?1"),
                [id.as_str()],
                "runs",
                &["id"],
                decode_run,
            )
        })
    }

    /// Every run on an issue, newest first.
    pub fn runs_for_issue(&self, issue: &IssueKey) -> Result<Vec<Run>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE issue = ?1 ORDER BY started DESC, number DESC, id"),
                [issue.as_str()],
                "runs",
                &["id"],
                decode_run,
            )
        })
    }

    /// The newest run on an issue.
    pub fn latest_run(&self, issue: &IssueKey) -> Result<Option<Run>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!(
                    "SELECT {RUN_COLUMNS} FROM runs WHERE issue = ?1 ORDER BY started DESC, number DESC, id LIMIT 1"
                ),
                [issue.as_str()],
                "runs",
                &["id"],
                decode_run,
            )
        })
    }

    /// The newest run of every issue that has one, for the board, ordered by issue key.
    pub fn latest_runs(&self) -> Result<Vec<Run>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!(
                    "SELECT {RUN_COLUMNS} FROM (
                        SELECT *, row_number() OVER (
                            PARTITION BY issue ORDER BY started DESC, number DESC, id) AS rank
                        FROM runs WHERE issue IS NOT NULL)
                     WHERE rank = 1 ORDER BY issue"
                ),
                [],
                "runs",
                &["id"],
                decode_run,
            )
        })
    }

    /// Runs that still owe work or an answer (`RunState::is_open`), oldest first: what the
    /// engine resumes or marks interrupted at start-up.
    pub fn open_runs(&self) -> Result<Vec<Run>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {RUN_COLUMNS} FROM runs WHERE open = 1 ORDER BY started, id"),
                [],
                "runs",
                &["id"],
                decode_run,
            )
        })
    }

    /// Appends lines to a run's log, numbering them after the last one (the `seq` of each
    /// line is ignored). Returns the lines as stored.
    pub fn append_run_log(&self, run: &RunId, lines: &[RunLogLine]) -> Result<Vec<RunLogLine>> {
        let levels = lines.iter().map(|line| tag(&line.level)).collect::<Result<Vec<_>>>()?;
        self.write(|tx| {
            require_run(tx, run)?;
            let mut seq: i64 =
                tx.query_row("SELECT coalesce(max(seq), 0) FROM run_logs WHERE run = ?1", [run.as_str()], |row| {
                    row.get(0)
                })?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO run_logs (run, seq, at, node, variant, level, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut stored = Vec::with_capacity(lines.len());
            for (line, level) in lines.iter().zip(&levels) {
                seq += 1;
                insert.execute(params![
                    run.as_str(),
                    seq,
                    line.at,
                    line.node.as_ref().map(NodeId::as_str),
                    line.variant,
                    level,
                    line.text
                ])?;
                stored.push(RunLogLine { seq: seq as u64, ..line.clone() });
            }
            Ok(stored)
        })
    }

    /// Up to `limit` log lines after `after_seq` (0 for the start), in order.
    pub fn run_log_after(&self, run: &RunId, after_seq: u64, limit: usize) -> Result<Vec<RunLogLine>> {
        let after = int(after_seq)?;
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {LOG_COLUMNS} FROM run_logs WHERE run = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3"),
                params![run.as_str(), after, limit as i64],
                "run_logs",
                &["run", "seq"],
                decode_log,
            )
        })
    }

    /// The last `count` log lines, in order.
    pub fn run_log_tail(&self, run: &RunId, count: usize) -> Result<Vec<RunLogLine>> {
        let mut lines = self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {LOG_COLUMNS} FROM run_logs WHERE run = ?1 ORDER BY seq DESC LIMIT ?2"),
                params![run.as_str(), count as i64],
                "run_logs",
                &["run", "seq"],
                decode_log,
            )
        })?;
        lines.reverse();
        Ok(lines)
    }

    /// Adds a rendered before/after picture to a run.
    pub fn add_render(&self, run: &RunId, render: &Render) -> Result<()> {
        let render = serde_json::to_value(render)?;
        self.write(|tx| {
            require_run(tx, run)?;
            let current: String = tx.query_row("SELECT renders FROM runs WHERE id = ?1", [run.as_str()], |row| row.get(0))?;
            let mut renders: Vec<serde_json::Value> = serde_json::from_str(&current)
                .map_err(|err| anyhow::anyhow!("decode runs row {run}: column renders: {err}"))?;
            renders.push(render);
            tx.execute("UPDATE runs SET renders = ?2 WHERE id = ?1", params![run.as_str(), json(&renders)?])?;
            Ok(())
        })
    }

    /// A run's renders in the order they were added; empty for an unknown run.
    pub fn renders(&self, run: &RunId) -> Result<Vec<Render>> {
        let found = self.read(|conn| {
            query_one(conn, "SELECT id, renders FROM runs WHERE id = ?1", [run.as_str()], "runs", &["id"], |d| {
                d.json::<Vec<Render>>("renders")
            })
        })?;
        Ok(found.unwrap_or_default())
    }
}
