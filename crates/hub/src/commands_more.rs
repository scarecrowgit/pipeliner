//! Commands for runs, the inbox, the catalog and settings.

use std::path::{Path, PathBuf};

use gpui::{App, Task};

use crate::api::{
    GraphDef, InboxId, PermissionRule, PipelineId, PipelineVersion, Place, Project, ProjectId, ReviewDecision,
    ReviewId, Run, RunId, RunInput, Runnable, RunnableRef, Settings,
};
use crate::commands::Commands;

impl Commands {
    // ------------------------------------------------------------ runs

    /// Starts the live version of a pipeline on a manual item.
    pub fn start_run(&self, pipeline: &PipelineId, input: RunInput, cx: &mut App) -> Task<Option<Run>> {
        let pipeline = pipeline.clone();
        self.call("start the run", cx, move |api| api.start_run(&pipeline, input))
    }

    /// Runs the draft in throwaway worktrees.
    pub fn test_run(&self, pipeline: &PipelineId, input: RunInput, cx: &mut App) -> Task<Option<Run>> {
        let pipeline = pipeline.clone();
        self.call("start the test run", cx, move |api| api.test_run(&pipeline, input))
    }

    /// Cancels a run.
    pub fn cancel_run(&self, id: &RunId, cx: &mut App) {
        let id = id.clone();
        self.fire("cancel the run", cx, move |api| api.cancel_run(&id));
    }

    /// Continues an interrupted or failed run.
    pub fn resume_run(&self, id: &RunId, cx: &mut App) {
        let id = id.clone();
        self.fire("resume the run", cx, move |api| api.resume_run(&id));
    }

    // ------------------------------------------------------------ inbox

    /// Marks an inbox item read or unread.
    pub fn set_inbox_read(&self, id: &InboxId, read: bool, cx: &mut App) {
        let id = id.clone();
        self.fire("mark the item", cx, move |api| api.set_inbox_read(&id, read));
    }

    /// Archives an inbox item, or restores it.
    pub fn archive_inbox(&self, id: &InboxId, archived: bool, cx: &mut App) {
        let id = id.clone();
        let action = if archived { "archive the item" } else { "restore the item" };
        self.fire(action, cx, move |api| api.archive_inbox(&id, archived));
    }

    /// Ships, skips or sends back.
    pub fn decide_review(&self, id: &ReviewId, decision: ReviewDecision, cx: &mut App) {
        let id = id.clone();
        self.fire("record the decision", cx, move |api| api.decide_review(&id, decision));
    }

    // ------------------------------------------------------------ catalog

    /// Writes an agent or orchestrator to the library or the project.
    pub fn save_runnable(&self, place: Place, runnable: Runnable, cx: &mut App) {
        self.fire("save the agent", cx, move |api| api.save_runnable(&place, runnable));
    }

    /// Copies a runnable; resolves to the copy.
    pub fn duplicate_runnable(&self, reference: &RunnableRef, cx: &mut App) -> Task<Option<RunnableRef>> {
        let reference = reference.clone();
        self.call("duplicate it", cx, move |api| api.duplicate_runnable(&reference))
    }

    /// Renames a runnable; resolves to its new reference.
    pub fn rename_runnable(&self, reference: &RunnableRef, name: &str, cx: &mut App) -> Task<Option<RunnableRef>> {
        let (reference, name) = (reference.clone(), name.to_string());
        self.call("rename it", cx, move |api| api.rename_runnable(&reference, &name))
    }

    /// Pauses or resumes a runnable.
    pub fn set_runnable_paused(&self, reference: &RunnableRef, paused: bool, cx: &mut App) {
        let reference = reference.clone();
        let action = if paused { "pause it" } else { "resume it" };
        self.fire(action, cx, move |api| api.set_runnable_paused(&reference, paused));
    }

    /// Archives a runnable, or restores it.
    pub fn set_runnable_archived(&self, reference: &RunnableRef, archived: bool, cx: &mut App) {
        let reference = reference.clone();
        let action = if archived { "archive it" } else { "restore it" };
        self.fire(action, cx, move |api| api.set_runnable_archived(&reference, archived));
    }

    /// Deletes a runnable.
    pub fn delete_runnable(&self, reference: &RunnableRef, cx: &mut App) {
        let reference = reference.clone();
        self.fire("delete it", cx, move |api| api.delete_runnable(&reference));
    }

    /// Creates a runnable from a template; resolves to it.
    pub fn create_from_template(&self, template: &str, cx: &mut App) -> Task<Option<RunnableRef>> {
        let template = template.to_string();
        self.call("create it from the template", cx, move |api| api.create_from_template(&template))
    }

    /// Imports a runnable from a file; resolves to it.
    pub fn import_runnable(&self, file: &Path, cx: &mut App) -> Task<Option<RunnableRef>> {
        let file = file.to_path_buf();
        self.call("import the file", cx, move |api| api.import_runnable(&file))
    }

    /// Writes a runnable to a file; resolves to its path.
    pub fn export_runnable(&self, reference: &RunnableRef, to: &Path, cx: &mut App) -> Task<Option<PathBuf>> {
        let (reference, to) = (reference.clone(), to.to_path_buf());
        self.call("export it", cx, move |api| api.export_runnable(&reference, &to))
    }

    /// Saves a pipeline draft.
    pub fn save_pipeline(&self, place: Place, graph: GraphDef, cx: &mut App) {
        self.fire("save the pipeline", cx, move |api| api.save_pipeline(&place, graph));
    }

    /// Deploys the draft as the next version.
    pub fn deploy_pipeline(&self, id: &PipelineId, note: &str, cx: &mut App) -> Task<Option<PipelineVersion>> {
        let (id, note) = (id.clone(), note.to_string());
        self.call("deploy the pipeline", cx, move |api| api.deploy_pipeline(&id, &note))
    }

    // ------------------------------------------------------------ settings

    /// Saves the settings.
    pub fn set_settings(&self, settings: Settings, cx: &mut App) {
        self.fire("save the settings", cx, move |api| api.set_settings(settings));
    }

    /// Creates or replaces a project.
    pub fn save_project(&self, project: Project, cx: &mut App) {
        self.fire("save the project", cx, move |api| api.save_project(project));
    }

    /// Deletes a project.
    pub fn delete_project(&self, id: &ProjectId, cx: &mut App) {
        let id = id.clone();
        self.fire("delete the project", cx, move |api| api.delete_project(&id));
    }

    /// Removes a saved allow or deny rule.
    pub fn remove_permission_rule(&self, rule: PermissionRule, cx: &mut App) {
        self.fire("remove the rule", cx, move |api| api.remove_permission_rule(&rule));
    }

    /// Stops workers and marks running work interrupted. Blocks: call once, on quit, when the
    /// UI no longer needs to respond.
    pub fn shutdown(&self) {
        self.api.shutdown();
    }
}
