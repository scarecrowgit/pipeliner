//! Catalog state that does not live in files: deployed pipeline versions, agents created by
//! other agents, permission rules and autopilots.

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use model::{
    Autopilot, AutopilotId, Millis, Origin, PermissionRule, PipelineId, PipelineVersion, ProjectId, RuleScope, RunId,
    Runnable, RunnableRef, SessionId, VersionState,
};
use rusqlite::{OptionalExtension as _, params};

use crate::Store;
use crate::codec::{Decoder, check_path, json, query_all, query_one, tag};

const VERSION_COLUMNS: &str = "pipeline, version, state, def_text, hash, note, deployed_at";
const AUTOPILOT_COLUMNS: &str = "id, project, name, trigger, target, prompt, enabled, paused_reason, next_run, \
     last_run, last_result, budget_usd";

/// An agent another agent created, and when it ended (`None` while it is alive).
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnedRunnable {
    pub runnable: Runnable,
    pub ended: Option<Millis>,
}

fn decode_version(d: &Decoder) -> Result<PipelineVersion> {
    Ok(PipelineVersion {
        pipeline: PipelineId(d.string("pipeline")?),
        version: d.u32("version")?,
        state: d.tag("state")?,
        def_text: d.string("def_text")?,
        hash: d.string("hash")?,
        note: d.string("note")?,
        deployed_at: d.get("deployed_at")?,
    })
}

fn decode_autopilot(d: &Decoder) -> Result<Autopilot> {
    Ok(Autopilot {
        id: AutopilotId(d.string("id")?),
        project: ProjectId(d.string("project")?),
        name: d.string("name")?,
        trigger: d.json("trigger")?,
        target: RunnableRef(d.string("target")?),
        prompt: d.string("prompt")?,
        enabled: d.get("enabled")?,
        paused_reason: d.get("paused_reason")?,
        next_run: d.get("next_run")?,
        last_run: d.get("last_run")?,
        last_result: d.string("last_result")?,
        budget_usd: d.get("budget_usd")?,
    })
}

/// A rule scope as two text columns.
fn scope_key(scope: &RuleScope) -> Result<(&'static str, String)> {
    Ok(match scope {
        RuleScope::Global => ("global", String::new()),
        RuleScope::Repo { path } => {
            check_path(path, "rule repo")?;
            let text = path.to_str().with_context(|| format!("rule repo {} is not UTF-8", path.display()))?;
            ("repo", text.to_string())
        }
        RuleScope::Session { session } => ("session", session.as_str().to_string()),
    })
}

fn decode_rule(d: &Decoder) -> Result<PermissionRule> {
    let key = d.string("scope_key")?;
    let scope = match d.string("scope")?.as_str() {
        "global" => RuleScope::Global,
        "repo" => RuleScope::Repo { path: PathBuf::from(key) },
        "session" => RuleScope::Session { session: SessionId(key) },
        other => bail!("decode permission_rules row {}: unknown scope {other:?}", d.string("id")?),
    };
    Ok(PermissionRule { tool: d.string("tool")?, pattern: d.string("pattern")?, decision: d.tag("decision")?, scope })
}

impl Store {
    /// Deploys a pipeline: stores `def_text` as the next version and makes it live, retiring
    /// the version that was live, all in one transaction. Returns the new version.
    pub fn deploy_pipeline(
        &self,
        pipeline: &PipelineId,
        def_text: &str,
        hash: &str,
        note: &str,
        deployed_at: Millis,
    ) -> Result<PipelineVersion> {
        let (live, retired) = (tag(&VersionState::Live)?, tag(&VersionState::Retired)?);
        self.write(|tx| {
            tx.execute(
                "UPDATE pipeline_versions SET state = ?2 WHERE pipeline = ?1 AND state = ?3",
                params![pipeline.as_str(), retired, live],
            )?;
            let version: u32 = tx.query_row(
                "SELECT coalesce(max(version), 0) + 1 FROM pipeline_versions WHERE pipeline = ?1",
                [pipeline.as_str()],
                |row| row.get(0),
            )?;
            tx.execute(
                &format!("INSERT INTO pipeline_versions ({VERSION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"),
                params![pipeline.as_str(), version, live, def_text, hash, note, deployed_at],
            )?;
            Ok(PipelineVersion {
                pipeline: pipeline.clone(),
                version,
                state: VersionState::Live,
                def_text: def_text.to_string(),
                hash: hash.to_string(),
                note: note.to_string(),
                deployed_at,
            })
        })
    }

    /// The version new runs of a pipeline use, if it was ever deployed.
    pub fn live_pipeline_version(&self, pipeline: &PipelineId) -> Result<Option<PipelineVersion>> {
        let live = tag(&VersionState::Live)?;
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {VERSION_COLUMNS} FROM pipeline_versions WHERE pipeline = ?1 AND state = ?2"),
                [pipeline.as_str(), live.as_str()],
                "pipeline_versions",
                &["pipeline", "version"],
                decode_version,
            )
        })
    }

    /// One deployed version, live or retired: what a run pinned to it executes.
    pub fn pipeline_version(&self, pipeline: &PipelineId, version: u32) -> Result<Option<PipelineVersion>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {VERSION_COLUMNS} FROM pipeline_versions WHERE pipeline = ?1 AND version = ?2"),
                params![pipeline.as_str(), version],
                "pipeline_versions",
                &["pipeline", "version"],
                decode_version,
            )
        })
    }

    /// Every deployed version of a pipeline, newest first.
    pub fn pipeline_versions(&self, pipeline: &PipelineId) -> Result<Vec<PipelineVersion>> {
        self.read(|conn| {
            query_all(
                conn,
                &format!("SELECT {VERSION_COLUMNS} FROM pipeline_versions WHERE pipeline = ?1 ORDER BY version DESC"),
                [pipeline.as_str()],
                "pipeline_versions",
                &["pipeline", "version"],
                decode_version,
            )
        })
    }

    /// Records an agent created by another agent, or updates it. Its end time is kept.
    /// Fails for any other origin: those live in files.
    pub fn upsert_spawned(&self, runnable: &Runnable) -> Result<()> {
        let Origin::Spawned { by, run, session } = &runnable.origin else {
            bail!("{} is not a spawned runnable", runnable.reference);
        };
        let data = json(runnable)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO spawned_runnables (reference, by_ref, run, session, data) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (reference) DO UPDATE SET by_ref = excluded.by_ref, run = excluded.run,
                    session = excluded.session, data = excluded.data",
                params![
                    runnable.reference.as_str(),
                    by.as_str(),
                    run.as_ref().map(RunId::as_str),
                    session.as_ref().map(SessionId::as_str),
                    data
                ],
            )?;
            Ok(())
        })
    }

    /// Agents created by other agents, in creation order: all of them, or those of one run.
    pub fn spawned_runnables(&self, run: Option<&RunId>) -> Result<Vec<SpawnedRunnable>> {
        let decode = |d: &Decoder| Ok(SpawnedRunnable { runnable: d.json("data")?, ended: d.get("ended")? });
        self.read(|conn| match run {
            Some(run) => query_all(
                conn,
                "SELECT reference, data, ended FROM spawned_runnables WHERE run = ?1 ORDER BY rowid",
                [run.as_str()],
                "spawned_runnables",
                &["reference"],
                decode,
            ),
            None => query_all(
                conn,
                "SELECT reference, data, ended FROM spawned_runnables ORDER BY rowid",
                [],
                "spawned_runnables",
                &["reference"],
                decode,
            ),
        })
    }

    /// Marks a spawned agent as finished at `at`. Returns whether it exists.
    pub fn end_spawned(&self, reference: &RunnableRef, at: Millis) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE spawned_runnables SET ended = ?2 WHERE reference = ?1",
                params![reference.as_str(), at],
            )? > 0)
        })
    }

    /// Adds a permission rule. A rule for the same tool, pattern and scope is replaced, so
    /// the decision can change.
    pub fn add_permission_rule(&self, rule: &PermissionRule) -> Result<()> {
        let (scope, key) = scope_key(&rule.scope)?;
        let decision = tag(&rule.decision)?;
        self.write(|tx| {
            tx.execute(
                "INSERT INTO permission_rules (scope, scope_key, tool, pattern, decision) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (scope, scope_key, tool, pattern) DO UPDATE SET decision = excluded.decision",
                params![scope, key, rule.tool, rule.pattern, decision],
            )?;
            Ok(())
        })
    }

    /// Removes the rule with the same tool, pattern and scope, whatever its decision.
    /// Returns whether there was one.
    pub fn remove_permission_rule(&self, rule: &PermissionRule) -> Result<bool> {
        let (scope, key) = scope_key(&rule.scope)?;
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM permission_rules WHERE scope = ?1 AND scope_key = ?2 AND tool = ?3 AND pattern = ?4",
                params![scope, key, rule.tool, rule.pattern],
            )? > 0)
        })
    }

    /// The rules of one scope, in the order they were added.
    pub fn permission_rules(&self, scope: &RuleScope) -> Result<Vec<PermissionRule>> {
        let (scope, key) = scope_key(scope)?;
        self.read(|conn| {
            query_all(
                conn,
                "SELECT id, scope, scope_key, tool, pattern, decision FROM permission_rules
                 WHERE scope = ?1 AND scope_key = ?2 ORDER BY id",
                [scope, key.as_str()],
                "permission_rules",
                &["id"],
                decode_rule,
            )
        })
    }

    /// Every permission rule, in the order they were added.
    pub fn all_permission_rules(&self) -> Result<Vec<PermissionRule>> {
        self.read(|conn| {
            query_all(
                conn,
                "SELECT id, scope, scope_key, tool, pattern, decision FROM permission_rules ORDER BY id",
                [],
                "permission_rules",
                &["id"],
                decode_rule,
            )
        })
    }

    /// Creates an autopilot or replaces it.
    pub fn upsert_autopilot(&self, autopilot: &Autopilot) -> Result<()> {
        let trigger = json(&autopilot.trigger)?;
        self.write(|tx| {
            tx.execute(
                &format!(
                    "INSERT INTO autopilots ({AUTOPILOT_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT (id) DO UPDATE SET project = excluded.project, name = excluded.name,
                        trigger = excluded.trigger, target = excluded.target, prompt = excluded.prompt,
                        enabled = excluded.enabled, paused_reason = excluded.paused_reason,
                        next_run = excluded.next_run, last_run = excluded.last_run,
                        last_result = excluded.last_result, budget_usd = excluded.budget_usd"
                ),
                params![
                    autopilot.id.as_str(),
                    autopilot.project.as_str(),
                    autopilot.name,
                    trigger,
                    autopilot.target.as_str(),
                    autopilot.prompt,
                    autopilot.enabled,
                    autopilot.paused_reason,
                    autopilot.next_run,
                    autopilot.last_run,
                    autopilot.last_result,
                    autopilot.budget_usd,
                ],
            )?;
            Ok(())
        })
    }

    /// One autopilot.
    pub fn autopilot(&self, id: &AutopilotId) -> Result<Option<Autopilot>> {
        self.read(|conn| {
            query_one(
                conn,
                &format!("SELECT {AUTOPILOT_COLUMNS} FROM autopilots WHERE id = ?1"),
                [id.as_str()],
                "autopilots",
                &["id"],
                decode_autopilot,
            )
        })
    }

    /// Autopilots of one project, or of all projects, by name.
    pub fn autopilots(&self, project: Option<&ProjectId>) -> Result<Vec<Autopilot>> {
        self.read(|conn| match project {
            Some(project) => query_all(
                conn,
                &format!("SELECT {AUTOPILOT_COLUMNS} FROM autopilots WHERE project = ?1 ORDER BY name, id"),
                [project.as_str()],
                "autopilots",
                &["id"],
                decode_autopilot,
            ),
            None => query_all(
                conn,
                &format!("SELECT {AUTOPILOT_COLUMNS} FROM autopilots ORDER BY name, id"),
                [],
                "autopilots",
                &["id"],
                decode_autopilot,
            ),
        })
    }

    /// Deletes an autopilot. Returns whether it existed.
    pub fn delete_autopilot(&self, id: &AutopilotId) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM autopilots WHERE id = ?1", [id.as_str()])? > 0))
    }

    /// Records that an autopilot ticked at `at` with `result` ("filed 2 issues") and when it
    /// ticks next. Returns whether it exists.
    pub fn record_autopilot_tick(
        &self,
        id: &AutopilotId,
        at: Millis,
        result: &str,
        next_run: Option<Millis>,
    ) -> Result<bool> {
        self.write(|tx| {
            let found = tx
                .query_row(
                    "UPDATE autopilots SET last_run = ?2, last_result = ?3, next_run = ?4 WHERE id = ?1 RETURNING 1",
                    params![id.as_str(), at, result, next_run],
                    |_| Ok(()),
                )
                .optional()?;
            Ok(found.is_some())
        })
    }
}
