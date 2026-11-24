//! The spend ledger: one row per model call, and the sums Home and the sidebar show.

use anyhow::Result;
use base::time::{day_start, week_start};
use model::{Millis, NodeId, RunId, RunnableRef, SessionId, SpendBucket, SpendByRunnable, Usage, UsageEntry};
use rusqlite::{Connection, Params, Transaction, params};

use crate::Store;
use crate::codec::int;

const MS_PER_DAY: i64 = 86_400_000;

/// A sum over ledger rows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpendTotal {
    pub cost_usd: f64,
    pub usage: Usage,
    /// Model calls summed.
    pub calls: u64,
}

const TOTAL_COLUMNS: &str = "total(cost_usd), total(input), total(output), total(cache_read), total(cache_write), count(*)";

fn read_total(row: &rusqlite::Row, first: usize) -> rusqlite::Result<SpendTotal> {
    let n = |i: usize| -> rusqlite::Result<u64> { Ok(row.get::<_, f64>(first + i)? as u64) };
    Ok(SpendTotal {
        cost_usd: row.get(first)?,
        usage: Usage { input: n(1)?, output: n(2)?, cache_read: n(3)?, cache_write: n(4)? },
        calls: row.get::<_, i64>(first + 5)? as u64,
    })
}

fn total_where(conn: &Connection, condition: &str, params: impl Params) -> Result<SpendTotal> {
    let sql = format!("SELECT {TOTAL_COLUMNS} FROM usage_ledger WHERE {condition}");
    Ok(conn.query_row(&sql, params, |row| read_total(row, 0))?)
}

/// Appends one model call to the ledger inside the caller's transaction.
pub(crate) fn record_usage_in(tx: &Transaction, entry: &UsageEntry) -> Result<()> {
    let usage = entry.usage;
    let (input, output, cache_read, cache_write) =
        (int(usage.input)?, int(usage.output)?, int(usage.cache_read)?, int(usage.cache_write)?);
    tx.prepare_cached(
        "INSERT INTO usage_ledger (at, session, run, node, runnable, name, created_by, model, input, output,
            cache_read, cache_write, cost_usd, imported)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        entry.at,
        entry.session.as_ref().map(SessionId::as_str),
        entry.run.as_ref().map(RunId::as_str),
        entry.node.as_ref().map(NodeId::as_str),
        entry.runnable.as_str(),
        entry.name,
        entry.created_by.as_ref().map(RunnableRef::as_str),
        entry.model.to_string(),
        input,
        output,
        cache_read,
        cache_write,
        entry.cost_usd,
        entry.imported,
    ])?;
    Ok(())
}

/// Whether the ledger has any row of a run.
pub(crate) fn run_has_usage_in(tx: &Transaction, run: &RunId) -> Result<bool> {
    Ok(tx.query_row("SELECT EXISTS (SELECT 1 FROM usage_ledger WHERE run = ?1)", [run.as_str()], |row| row.get(0))?)
}

/// Reference of the row that sums runnables created by agents beyond the top spenders.
pub const CREATED_BY_AGENTS: &str = "spawned:*";

impl Store {
    /// Appends one model call to the ledger.
    pub fn record_usage(&self, entry: &UsageEntry) -> Result<()> {
        self.write(|tx| record_usage_in(tx, entry))
    }

    /// Spend of calls made at or after `from` and before `to`.
    pub fn spend_between(&self, from: Millis, to: Millis) -> Result<SpendTotal> {
        self.read(|conn| total_where(conn, "at >= ?1 AND at < ?2", params![from, to]))
    }

    /// Spend per local day for the days that overlap `[from, to)`, oldest first, days without
    /// spend included. A bucket starts at `base::time::day_start` of its day; the whole day
    /// is summed even where it reaches outside the range.
    pub fn spend_by_day(&self, from: Millis, to: Millis, utc_offset_secs: i32) -> Result<Vec<SpendBucket>> {
        let first = day_start(from, utc_offset_secs);
        let sums: Vec<(i64, f64)> = self.read(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT (at - ?1) / ?3 AS day, total(cost_usd) FROM usage_ledger
                 WHERE at >= ?1 AND at < ?2 GROUP BY day ORDER BY day",
            )?;
            let end = day_start(to.saturating_sub(1), utc_offset_secs).saturating_add(MS_PER_DAY);
            let rows = stmt.query_map(params![first, end.max(first), MS_PER_DAY], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })?;
        let mut buckets = Vec::new();
        let mut start = first;
        while start < to {
            buckets.push(SpendBucket { start, cost_usd: 0.0 });
            // A day is 24 hours in a fixed offset.
            start += MS_PER_DAY;
        }
        for (day, cost) in sums {
            if let Some(bucket) = usize::try_from(day).ok().and_then(|day| buckets.get_mut(day)) {
                bucket.cost_usd += cost;
            }
        }
        Ok(buckets)
    }

    /// Spend per local week (Monday to Sunday) for the weeks that overlap `[from, to)`, oldest
    /// first, empty weeks included. A bucket starts at `base::time::week_start`.
    pub fn spend_by_week(&self, from: Millis, to: Millis, utc_offset_secs: i32) -> Result<Vec<SpendBucket>> {
        let first = week_start(from, utc_offset_secs);
        let days = self.spend_by_day(first, to, utc_offset_secs)?;
        let mut weeks: Vec<SpendBucket> = Vec::new();
        for day in days {
            let start = week_start(day.start, utc_offset_secs);
            match weeks.last_mut() {
                Some(week) if week.start == start => week.cost_usd += day.cost_usd,
                _ => weeks.push(SpendBucket { start, cost_usd: day.cost_usd }),
            }
        }
        Ok(weeks)
    }

    /// "Spend by agent" for `[from, to)`: the `top` biggest spenders, then one row named
    /// "Others" (with an empty reference) summing the rest of the person's runnables, and one
    /// named "Created by agents" ([`CREATED_BY_AGENTS`], `spawned`) summing the rest of the
    /// runnables agents created, each when there is such a rest. A runnable's
    /// name is the one of its latest call; `spawned` is set when another agent created it.
    pub fn spend_by_runnable(&self, from: Millis, to: Millis, top: usize) -> Result<Vec<SpendByRunnable>> {
        let rows: Vec<(String, String, f64, bool)> = self.read(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT runnable,
                        (SELECT name FROM usage_ledger l WHERE l.runnable = u.runnable AND l.at >= ?1 AND l.at < ?2
                         ORDER BY l.at DESC, l.id DESC LIMIT 1),
                        total(cost_usd) AS cost,
                        max(created_by IS NOT NULL OR runnable LIKE 'spawned:%')
                 FROM usage_ledger u WHERE at >= ?1 AND at < ?2
                 GROUP BY runnable ORDER BY cost DESC, runnable",
            )?;
            let rows = stmt.query_map(params![from, to], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })?;
        let mut out: Vec<SpendByRunnable> = rows
            .iter()
            .take(top)
            .map(|(runnable, name, cost_usd, spawned)| SpendByRunnable {
                runnable: RunnableRef(runnable.clone()),
                name: name.clone(),
                cost_usd: *cost_usd,
                spawned: *spawned,
            })
            .collect();
        // The rest: what the person's runnables spent as "Others", and what runnables created
        // by agents spent as one more row, so totals by who created them stay right.
        let rest = rows.get(top..).unwrap_or_default();
        let others: f64 = rest.iter().filter(|row| !row.3).map(|row| row.2).sum();
        let created: f64 = rest.iter().filter(|row| row.3).map(|row| row.2).sum();
        if rest.iter().any(|row| !row.3) {
            out.push(SpendByRunnable { runnable: RunnableRef(String::new()), name: "Others".to_string(), cost_usd: others, spawned: false });
        }
        if rest.iter().any(|row| row.3) {
            out.push(SpendByRunnable {
                runnable: RunnableRef(CREATED_BY_AGENTS.to_string()),
                name: "Created by agents".to_string(),
                cost_usd: created,
                spawned: true,
            });
        }
        Ok(out)
    }

    /// Everything one session spent.
    pub fn spend_for_session(&self, session: &SessionId) -> Result<SpendTotal> {
        self.read(|conn| total_where(conn, "session = ?1", [session.as_str()]))
    }

    /// Everything one run spent.
    pub fn spend_for_run(&self, run: &RunId) -> Result<SpendTotal> {
        self.read(|conn| total_where(conn, "run = ?1", [run.as_str()]))
    }

    /// What each node of a run spent, in node id order. Calls without a node are left out.
    pub fn spend_by_node(&self, run: &RunId) -> Result<Vec<(NodeId, SpendTotal)>> {
        self.read(|conn| {
            let sql = format!(
                "SELECT node, {TOTAL_COLUMNS} FROM usage_ledger WHERE run = ?1 AND node IS NOT NULL
                 GROUP BY node ORDER BY node"
            );
            let mut stmt = conn.prepare_cached(&sql)?;
            let rows = stmt.query_map([run.as_str()], |row| Ok((NodeId(row.get(0)?), read_total(row, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// How many runs each runnable worked in since `since`: distinct runs, and distinct
    /// sessions for calls outside a run (a chat counts as one). Most active first.
    pub fn runs_by_runnable(&self, since: Millis) -> Result<Vec<(RunnableRef, u32)>> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT runnable, count(DISTINCT coalesce('run:' || run, 'session:' || session)) AS n
                 FROM usage_ledger WHERE at >= ?1 AND (run IS NOT NULL OR session IS NOT NULL)
                 GROUP BY runnable ORDER BY n DESC, runnable",
            )?;
            let rows = stmt.query_map([since], |row| Ok((RunnableRef(row.get(0)?), row.get::<_, u32>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }
}
