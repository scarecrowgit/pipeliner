//! The fixture's catalog, Home numbers and settings.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::api::{
    CatalogApi, Event, GraphChange, GraphDef, HomeStats, LoadProblem, Need, OpsApi, Period, PermissionRule, PipelineId,
    PipelineVersion, Place, Problem, Project, ProjectId, Runnable, RunnableActivity, RunnableKind, RunnableRef,
    RunnableState, Runtime, Settings, Severity, Skill, SpendBucket, SpendByRunnable, SpendToday, Template, UsedBy,
    VersionState, Working,
};

use super::FixtureApi;

impl FixtureApi {
    fn catalog_changed(&self) {
        self.emit(Event::CatalogChanged);
    }
}

impl CatalogApi for FixtureApi {
    fn runnables(&self) -> Vec<Runnable> {
        self.world().runnables.clone()
    }

    fn runnable_activity(&self) -> Result<Vec<(RunnableRef, RunnableActivity)>> {
        Ok(self.world().activity.clone())
    }

    fn skills(&self) -> Vec<Skill> {
        self.world().skills.clone()
    }

    fn catalog_problems(&self) -> Vec<LoadProblem> {
        Vec::new()
    }

    fn used_by(&self, reference: &RunnableRef) -> Result<UsedBy> {
        let world = self.world();
        Ok(UsedBy {
            issues: world.issues.iter().filter(|issue| issue.runner.as_ref() == Some(reference)).map(|issue| issue.key.clone()).collect(),
            pipelines: world
                .pipelines
                .iter()
                .filter(|graph| {
                    graph.nodes.iter().any(|node| match &node.kind {
                        crate::api::NodeKind::Agent(agent) | crate::api::NodeKind::Orchestrator(agent) => {
                            agent.runnable.as_ref() == Some(reference)
                        }
                        _ => false,
                    })
                })
                .map(|graph| graph.id.clone())
                .collect(),
            created: world
                .runnables
                .iter()
                .filter(|runnable| matches!(&runnable.origin, crate::api::Origin::Spawned { by, .. } if by == reference))
                .map(|runnable| runnable.reference.clone())
                .collect(),
        })
    }

    fn save_runnable(&self, _place: &Place, runnable: Runnable) -> Result<()> {
        self.record("save_runnable", runnable.reference.to_string())?;
        {
            let mut world = self.world();
            match world.runnables.iter_mut().find(|known| known.reference == runnable.reference) {
                Some(known) => *known = runnable,
                None => world.runnables.push(runnable),
            }
        }
        self.catalog_changed();
        Ok(())
    }

    fn duplicate_runnable(&self, reference: &RunnableRef) -> Result<RunnableRef> {
        self.record("duplicate_runnable", reference.to_string())?;
        let copy = {
            let mut world = self.world();
            let mut copy = world.runnable_mut(reference)?.clone();
            copy.reference = RunnableRef(format!("{}-copy", reference.as_str()));
            copy.name = format!("{} copy", copy.name);
            world.runnables.push(copy.clone());
            copy.reference
        };
        self.catalog_changed();
        Ok(copy)
    }

    fn rename_runnable(&self, reference: &RunnableRef, name: &str) -> Result<RunnableRef> {
        self.record("rename_runnable", format!("{reference} {name}"))?;
        self.world().runnable_mut(reference)?.name = name.to_string();
        self.catalog_changed();
        Ok(reference.clone())
    }

    fn set_runnable_paused(&self, reference: &RunnableRef, paused: bool) -> Result<()> {
        self.record("set_runnable_paused", format!("{reference} {paused}"))?;
        {
            let mut world = self.world();
            world.runnable_mut(reference)?.paused = paused;
            if let Some((_, activity)) = world.activity.iter_mut().find(|(known, _)| known == reference) {
                activity.state = if paused { RunnableState::Paused } else { RunnableState::Idle };
            }
        }
        self.catalog_changed();
        Ok(())
    }

    fn set_runnable_archived(&self, reference: &RunnableRef, archived: bool) -> Result<()> {
        self.record("set_runnable_archived", format!("{reference} {archived}"))?;
        self.world().runnable_mut(reference)?.archived = archived;
        self.catalog_changed();
        Ok(())
    }

    fn delete_runnable(&self, reference: &RunnableRef) -> Result<()> {
        self.record("delete_runnable", reference.to_string())?;
        {
            let mut world = self.world();
            world.runnable_mut(reference)?;
            world.runnables.retain(|known| &known.reference != reference);
        }
        self.catalog_changed();
        Ok(())
    }

    fn templates(&self) -> Vec<Template> {
        let template = |id: &str, name: &str, description: &str, kind| Template {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            kind,
        };
        vec![
            template("bug-fixer", "Bug fixer", "Reproduces, writes a failing test, fixes", RunnableKind::Agent),
            template("reviewer", "Code reviewer", "Reads a diff and comments, never edits", RunnableKind::Agent),
            template("triage", "Issue triage", "Router › specialists › Review", RunnableKind::Pipeline),
        ]
    }

    fn create_from_template(&self, template: &str) -> Result<RunnableRef> {
        self.record("create_from_template", template.to_string())?;
        let reference = RunnableRef::agent(&format!("{template}-{}", self.world().fresh_id("t")));
        self.catalog_changed();
        Ok(reference)
    }

    fn import_runnable(&self, file: &Path) -> Result<RunnableRef> {
        self.record("import_runnable", file.display().to_string())?;
        bail!("importing files is not available in previews")
    }

    fn export_runnable(&self, reference: &RunnableRef, to: &Path) -> Result<PathBuf> {
        self.record("export_runnable", format!("{reference} {}", to.display()))?;
        Ok(to.join(format!("{}.md", reference.id())))
    }

    fn pipeline(&self, id: &PipelineId) -> Option<GraphDef> {
        self.world().pipelines.iter().find(|graph| &graph.id == id).cloned()
    }

    fn save_pipeline(&self, _place: &Place, graph: GraphDef) -> Result<()> {
        self.record("save_pipeline", graph.id.to_string())?;
        {
            let mut world = self.world();
            world.pipelines.retain(|known| known.id != graph.id);
            world.pipelines.push(graph);
        }
        self.catalog_changed();
        Ok(())
    }

    fn pipeline_problems(&self, graph: &GraphDef) -> Vec<Problem> {
        graph
            .nodes
            .iter()
            .filter(|node| matches!(&node.kind, crate::api::NodeKind::Source(source) if source.source == crate::api::SourceKind::Jira))
            .map(|node| Problem {
                severity: Severity::Error,
                node: Some(node.id.clone()),
                message: "Jira sources are not available yet".to_string(),
            })
            .collect()
    }

    fn pipeline_versions(&self, id: &PipelineId) -> Result<Vec<PipelineVersion>> {
        Ok(self.world().versions.get(id).cloned().unwrap_or_default())
    }

    fn pipeline_changes(&self, id: &PipelineId) -> Result<Vec<GraphChange>> {
        if id.as_str() != super::data_runs::PAY_TRIAGE {
            return Ok(Vec::new());
        }
        Ok(vec![
            GraphChange::Changed { node: "fixer".into(), name: "Fixer".into(), what: "variants 2 → 3".into() },
            GraphChange::Added { node: "testw".into(), name: "Test writer".into(), what: "sub-agent of Fixer".into() },
        ])
    }

    fn deploy_pipeline(&self, id: &PipelineId, note: &str) -> Result<PipelineVersion> {
        self.record("deploy_pipeline", format!("{id} {note}"))?;
        let version = {
            let mut world = self.world();
            let now = world.now;
            let versions = world.versions.entry(id.clone()).or_default();
            for version in versions.iter_mut() {
                version.state = VersionState::Retired;
            }
            let number = versions.last().map_or(1, |last| last.version + 1);
            let version = PipelineVersion {
                pipeline: id.clone(),
                version: number,
                state: VersionState::Live,
                def_text: String::new(),
                hash: format!("{id}-{number:03}"),
                note: note.to_string(),
                deployed_at: now,
            };
            versions.push(version.clone());
            version
        };
        self.catalog_changed();
        Ok(version)
    }
}

impl OpsApi for FixtureApi {
    fn home_stats(&self, period: Period) -> Result<HomeStats> {
        self.fail_if_told("home_stats")?;
        Ok(self.world().ops.stats.get(&period).cloned().unwrap_or_default())
    }

    fn needs_you(&self) -> Result<Vec<Need>> {
        self.fail_if_told("needs_you")?;
        Ok(self.world().ops.needs.clone())
    }

    fn working_now(&self) -> Result<Vec<Working>> {
        self.fail_if_told("working_now")?;
        Ok(self.world().ops.working.clone())
    }

    fn spend_today(&self) -> Result<SpendToday> {
        self.fail_if_told("spend_today")?;
        Ok(self.world().ops.spend_today.clone())
    }

    fn spend_buckets(&self, period: Period) -> Result<Vec<SpendBucket>> {
        self.fail_if_told("spend_buckets")?;
        Ok(self.world().ops.buckets.get(&period).cloned().unwrap_or_default())
    }

    fn spend_by_runnable(&self, period: Period, top: usize) -> Result<Vec<SpendByRunnable>> {
        self.fail_if_told("spend_by_runnable")?;
        let rows = self.world().ops.by_runnable.get(&period).cloned().unwrap_or_default();
        let (named, others): (Vec<SpendByRunnable>, Vec<SpendByRunnable>) =
            rows.into_iter().partition(|row| row.runnable.as_str() != "others");
        if named.len() <= top && others.is_empty() {
            return Ok(named);
        }
        let mut out: Vec<SpendByRunnable> = named.iter().take(top).cloned().collect();
        let rest = &named[top.min(named.len())..];
        // The stored "Others · 6" row stands for runnables beyond the named ones.
        let hidden: usize = others
            .iter()
            .filter_map(|row| row.name.rsplit("· ").next().and_then(|count| count.parse::<usize>().ok()))
            .sum();
        let cost: f64 = rest.iter().chain(&others).map(|row| row.cost_usd).sum();
        out.push(SpendByRunnable {
            runnable: RunnableRef("others".into()),
            name: format!("Others · {}", rest.len() + hidden),
            cost_usd: cost,
            spawned: false,
        });
        Ok(out)
    }

    fn runtimes(&self) -> Vec<Runtime> {
        self.world().runtimes.clone()
    }

    fn settings(&self) -> Result<Settings> {
        self.fail_if_told("settings")?;
        Ok(self.world().settings.clone())
    }

    fn set_settings(&self, settings: Settings) -> Result<()> {
        self.record("set_settings", format!("{settings:?}"))?;
        self.world().settings = settings;
        self.emit(Event::SettingsChanged);
        Ok(())
    }

    fn projects(&self) -> Result<Vec<Project>> {
        self.fail_if_told("projects")?;
        Ok(self.world().projects.clone())
    }

    fn save_project(&self, project: Project) -> Result<()> {
        self.record("save_project", project.name.clone())?;
        {
            let mut world = self.world();
            world.projects.retain(|known| known.id != project.id);
            world.projects.push(project);
        }
        self.emit(Event::SettingsChanged);
        Ok(())
    }

    fn delete_project(&self, id: &ProjectId) -> Result<()> {
        self.record("delete_project", id.to_string())?;
        {
            let mut world = self.world();
            if !world.projects.iter().any(|known| &known.id == id) {
                return Err(anyhow!("no project {id}"));
            }
            world.projects.retain(|known| &known.id != id);
        }
        self.emit(Event::SettingsChanged);
        Ok(())
    }

    fn permission_rules(&self) -> Result<Vec<PermissionRule>> {
        Ok(self.world().rules.clone())
    }

    fn remove_permission_rule(&self, rule: &PermissionRule) -> Result<()> {
        self.record("remove_permission_rule", format!("{} {}", rule.tool, rule.pattern))?;
        self.world().rules.retain(|known| known != rule);
        Ok(())
    }

    fn shutdown(&self) {
        self.record("shutdown", String::new()).ok();
    }
}
