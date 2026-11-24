//! [`CatalogApi`] over the `catalog` crate's files and the store's deployed versions. Every
//! write reloads the catalog and publishes [`Event::CatalogChanged`]; nothing is watched.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use catalog::{Instance, Place, TemplateContext};
use model::{
    GraphChange, GraphDef, PipelineId, PipelineVersion, Problem, Runnable, RunnableActivity, RunnableKind,
    RunnableRef, RunnableState, Severity, Skill,
};

use crate::api::{CatalogApi, LoadProblem, Template, UsedBy};
use crate::engine::{Engine, NOT_WIRED};

const MS_PER_DAY: i64 = 86_400_000;

impl Engine {
    /// Runs a write against the catalog, then reloads it.
    fn catalog_write<T>(&self, write: impl FnOnce(&catalog::Catalog) -> Result<T>) -> Result<T> {
        let catalog = self.inner.catalog();
        let out = write(&catalog);
        self.inner.reload_catalog();
        out
    }

    /// The place a runnable's copies go: where it lives, else the library.
    fn place_of(&self, reference: &RunnableRef) -> Place {
        self.inner.catalog().get(reference).and_then(|r| Place::of(&r.origin)).unwrap_or(Place::Library)
    }
}

impl CatalogApi for Engine {
    fn runnables(&self) -> Vec<Runnable> {
        self.inner.catalog().runnables().to_vec()
    }

    fn runnable_activity(&self) -> Result<Vec<(RunnableRef, RunnableActivity)>> {
        let inner = &self.inner;
        let now = inner.now();
        let today = base::time::day_start(now, base::time::utc_offset(now));
        let runs = inner.store.runs_by_runnable(today)?;
        let spend = inner.store.spend_by_runnable(today, now + 1, usize::MAX)?;
        let running: Vec<(RunnableRef, Option<model::IssueKey>, bool)> = inner
            .live
            .running()
            .into_iter()
            .filter_map(|id| inner.store.session(&id).ok().flatten())
            .map(|s| {
                let waiting = !inner.asks.of(&s.id).is_empty();
                (s.runnable, s.issue, waiting)
            })
            .collect();
        let default_chat = catalog::Catalog::default_chat_agent();
        let catalog = inner.catalog();
        let mut out = Vec::new();
        for runnable in catalog.runnables() {
            let reference = &runnable.reference;
            let active = running.iter().find(|(r, _, _)| r == reference);
            let state = if runnable.paused {
                RunnableState::Paused
            } else if let Some((_, issue, waiting)) = active {
                if *waiting { RunnableState::Waiting { issue: issue.clone() } } else { RunnableState::Running { issue: issue.clone() } }
            } else if runnable.kind == RunnableKind::Pipeline {
                let id = PipelineId::new(reference.id());
                match inner.store.live_pipeline_version(&id)? {
                    Some(version) => RunnableState::Live { version: version.version },
                    None => RunnableState::Draft { version: 0 },
                }
            } else if *reference == default_chat {
                RunnableState::Ready
            } else {
                RunnableState::Idle
            };
            let activity = RunnableActivity {
                state,
                runs_today: runs.iter().find(|(r, _)| r == reference).map_or(0, |(_, n)| *n),
                cost_today_usd: spend.iter().find(|row| &row.runnable == reference).map_or(0.0, |row| row.cost_usd),
                queued: 0,
            };
            out.push((reference.clone(), activity));
        }
        Ok(out)
    }

    fn skills(&self) -> Vec<Skill> {
        self.inner.catalog().skills().to_vec()
    }

    fn catalog_problems(&self) -> Vec<LoadProblem> {
        self.inner.catalog().problems().to_vec()
    }

    fn used_by(&self, reference: &RunnableRef) -> Result<UsedBy> {
        let inner = &self.inner;
        let mut issues = Vec::new();
        for project in inner.store.projects()? {
            for issue in inner.store.issues(&project.id)? {
                if issue.runner.as_ref() == Some(reference) {
                    let open = issue.run.as_ref().and_then(|id| inner.store.run(id).ok().flatten()).is_some_and(|r| r.state.is_open());
                    issues.push((open, issue.key));
                }
            }
        }
        issues.sort_by_key(|(open, _)| !*open);
        let catalog = inner.catalog();
        let quoted = format!("\"{reference}\"");
        let pipelines = catalog
            .runnables()
            .iter()
            .filter(|r| r.kind == RunnableKind::Pipeline)
            .map(|r| PipelineId::new(r.reference.id()))
            .filter(|id| {
                catalog.pipeline(id).and_then(|g| serde_json::to_string(g).ok()).is_some_and(|text| text.contains(&quoted))
            })
            .collect();
        Ok(UsedBy { issues: issues.into_iter().map(|(_, key)| key).collect(), pipelines, created: Vec::new() })
    }

    fn save_runnable(&self, place: &Place, runnable: Runnable) -> Result<()> {
        let paths = self.inner.paths.clone();
        self.catalog_write(|_| catalog::write::save_agent(&paths, place, &runnable).map(|_| ()))
    }

    fn duplicate_runnable(&self, reference: &RunnableRef) -> Result<RunnableRef> {
        let (paths, place) = (self.inner.paths.clone(), self.place_of(reference));
        self.catalog_write(|c| catalog::write::duplicate(c, &paths, &place, reference))
    }

    fn rename_runnable(&self, reference: &RunnableRef, name: &str) -> Result<RunnableRef> {
        self.catalog_write(|c| catalog::write::rename(c, reference, name))
    }

    fn set_runnable_paused(&self, reference: &RunnableRef, paused: bool) -> Result<()> {
        self.catalog_write(|c| catalog::write::set_paused(c, reference, paused))
    }

    fn set_runnable_archived(&self, reference: &RunnableRef, archived: bool) -> Result<()> {
        self.catalog_write(|c| catalog::write::set_archived(c, reference, archived))
    }

    fn delete_runnable(&self, reference: &RunnableRef) -> Result<()> {
        self.catalog_write(|c| catalog::write::delete(c, reference))
    }

    fn templates(&self) -> Vec<Template> {
        catalog::templates()
    }

    fn create_from_template(&self, template: &str) -> Result<RunnableRef> {
        let paths = self.inner.paths.clone();
        self.catalog_write(|c| match catalog::instantiate(c, template, &TemplateContext::default())? {
            Instance::Agent(runnable) => {
                catalog::write::save_agent(&paths, &Place::Library, &runnable)?;
                Ok(runnable.reference)
            }
            Instance::Pipeline(graph) => {
                catalog::write::save_pipeline(&paths, &Place::Library, &graph)?;
                Ok(RunnableRef::pipeline(graph.id.as_str()))
            }
        })
    }

    fn import_runnable(&self, file: &Path) -> Result<RunnableRef> {
        let paths = self.inner.paths.clone();
        self.catalog_write(|c| catalog::transfer::import(c, &paths, &Place::Library, file))
    }

    fn export_runnable(&self, reference: &RunnableRef, to: &Path) -> Result<PathBuf> {
        let catalog = self.inner.catalog();
        let runnable = catalog.get(reference).ok_or_else(|| anyhow!("{reference} does not exist"))?;
        let path = if to.is_dir() || to.extension().is_none() {
            let ext = if runnable.kind == RunnableKind::Pipeline { "toml" } else { "md" };
            to.join(format!("{}.{ext}", reference.id()))
        } else {
            to.to_path_buf()
        };
        catalog::transfer::export(&catalog, reference, &path)?;
        Ok(path)
    }

    fn pipeline(&self, id: &PipelineId) -> Option<GraphDef> {
        self.inner.catalog().pipeline(id).cloned()
    }

    fn save_pipeline(&self, place: &Place, graph: GraphDef) -> Result<()> {
        let paths = self.inner.paths.clone();
        self.catalog_write(|_| catalog::write::save_pipeline(&paths, place, &graph).map(|_| ()))
    }

    fn pipeline_problems(&self, _graph: &GraphDef) -> Vec<Problem> {
        vec![Problem { severity: Severity::Warning, node: None, message: format!("{NOT_WIRED}: the graph is not checked") }]
    }

    fn pipeline_versions(&self, id: &PipelineId) -> Result<Vec<PipelineVersion>> {
        self.inner.store.pipeline_versions(id)
    }

    fn pipeline_changes(&self, _id: &PipelineId) -> Result<Vec<GraphChange>> {
        bail!("{NOT_WIRED}")
    }

    fn deploy_pipeline(&self, _id: &PipelineId, _note: &str) -> Result<PipelineVersion> {
        bail!("{NOT_WIRED}")
    }
}

/// Days in a period, for limits scaled to it.
pub(crate) fn days(period: crate::api::Period) -> i64 {
    match period {
        crate::api::Period::Today => 1,
        crate::api::Period::Days7 => 7,
        crate::api::Period::Days30 => 30,
        crate::api::Period::Days90 => 90,
    }
}

/// The start of a period: local midnight of its first day.
pub(crate) fn period_start(now: i64, period: crate::api::Period) -> i64 {
    let offset = base::time::utc_offset(now);
    base::time::day_start(now - (days(period) - 1) * MS_PER_DAY, offset)
}
