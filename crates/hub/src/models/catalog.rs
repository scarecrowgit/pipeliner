//! The library: runnables with their activity, skills, load problems, and pipeline drafts and
//! versions on demand. Everything reloads on `CatalogChanged`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::Context;

use crate::api::{
    Api, GraphDef, LoadProblem, PipelineId, PipelineVersion, Runnable, RunnableActivity, RunnableKind, RunnableRef,
    Skill,
};
use crate::load::{LoadState, Loadable, fetch};

/// Runnables, skills and pipelines as the agents screen, pickers and the editor show them.
pub struct CatalogModel {
    api: Arc<dyn Api>,
    runnables: Loadable<Vec<Runnable>>,
    activity: Loadable<HashMap<RunnableRef, RunnableActivity>>,
    skills: Loadable<Vec<Skill>>,
    problems: Loadable<Vec<LoadProblem>>,
    pipelines: HashMap<PipelineId, Loadable<Option<GraphDef>>>,
    versions: HashMap<PipelineId, Loadable<Vec<PipelineVersion>>>,
}

impl CatalogModel {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self {
            api,
            runnables: Loadable::default(),
            activity: Loadable::default(),
            skills: Loadable::default(),
            problems: Loadable::default(),
            pipelines: HashMap::new(),
            versions: HashMap::new(),
        }
    }

    /// The load state of the runnable list, which every view of this model needs.
    pub fn state(&self) -> &LoadState {
        self.runnables.state()
    }

    /// Every runnable of the library and the project, archived ones included.
    pub fn runnables(&self) -> &[Runnable] {
        self.runnables.value().map_or(&[], |list| list.as_slice())
    }

    /// One runnable by reference.
    pub fn runnable(&self, reference: &RunnableRef) -> Option<&Runnable> {
        self.runnables().iter().find(|runnable| &runnable.reference == reference)
    }

    /// Runnables of one kind, archived ones left out.
    pub fn of_kind(&self, kind: RunnableKind) -> impl Iterator<Item = &Runnable> {
        self.runnables().iter().filter(move |runnable| runnable.kind == kind && !runnable.archived)
    }

    /// State and activity numbers of a runnable.
    pub fn activity(&self, reference: &RunnableRef) -> Option<&RunnableActivity> {
        self.activity.value().and_then(|map| map.get(reference))
    }

    /// The skills of the library.
    pub fn skills(&self) -> &Loadable<Vec<Skill>> {
        &self.skills
    }

    /// Files of the library that could not be read.
    pub fn problems(&self) -> &Loadable<Vec<LoadProblem>> {
        &self.problems
    }

    /// The draft of a pipeline, once [`Self::ensure_pipeline`] asked for it. `Some(None)` inside
    /// means the pipeline does not exist.
    pub fn pipeline(&self, id: &PipelineId) -> Option<&Loadable<Option<GraphDef>>> {
        self.pipelines.get(id)
    }

    /// Deployed versions of a pipeline, once [`Self::ensure_versions`] asked for them.
    pub fn versions(&self, id: &PipelineId) -> Option<&Loadable<Vec<PipelineVersion>>> {
        self.versions.get(id)
    }

    /// Reads runnables, activity, skills and problems, once.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.runnables.started() {
            self.reload_lists(cx);
        }
    }

    /// Reads a pipeline's draft unless it is loaded or loading.
    pub fn ensure_pipeline(&mut self, id: &PipelineId, cx: &mut Context<Self>) {
        if !self.pipelines.get(id).is_some_and(Loadable::started) {
            self.load_pipeline(id.clone(), cx);
        }
    }

    /// Reads a pipeline's versions unless they are loaded or loading.
    pub fn ensure_versions(&mut self, id: &PipelineId, cx: &mut Context<Self>) {
        if !self.versions.get(id).is_some_and(Loadable::started) {
            self.load_versions(id.clone(), cx);
        }
    }

    fn reload_lists(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.runnables, |api| Ok(api.runnables()));
        self.reload_activity(cx);
        fetch(self, cx, &api, |this: &mut Self| &mut this.skills, |api| Ok(api.skills()));
        fetch(self, cx, &api, |this: &mut Self| &mut this.problems, |api| Ok(api.catalog_problems()));
    }

    fn reload_activity(&mut self, cx: &mut Context<Self>) {
        let api = self.api.clone();
        fetch(self, cx, &api, |this: &mut Self| &mut this.activity, |api| {
            Ok(api.runnable_activity()?.into_iter().collect())
        });
    }

    fn load_pipeline(&mut self, id: PipelineId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let slot = id.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.pipelines.entry(slot.clone()).or_default(),
            move |api| Ok(api.pipeline(&id)),
        );
    }

    fn load_versions(&mut self, id: PipelineId, cx: &mut Context<Self>) {
        let api = self.api.clone();
        let slot = id.clone();
        fetch(
            self,
            cx,
            &api,
            move |this: &mut Self| this.versions.entry(slot.clone()).or_default(),
            move |api| api.pipeline_versions(&id),
        );
    }

    /// The library changed: re-read everything that was read before.
    pub(crate) fn catalog_changed(&mut self, cx: &mut Context<Self>) {
        if self.runnables.started() {
            self.reload_lists(cx);
        }
        let pipelines: Vec<PipelineId> = self.pipelines.keys().cloned().collect();
        for id in pipelines {
            self.load_pipeline(id, cx);
        }
        let versions: Vec<PipelineId> = self.versions.keys().cloned().collect();
        for id in versions {
            self.load_versions(id, cx);
        }
    }

    /// Spend changed: activity numbers include today's cost.
    pub(crate) fn spend_changed(&mut self, cx: &mut Context<Self>) {
        if self.activity.started() {
            self.reload_activity(cx);
        }
    }
}
