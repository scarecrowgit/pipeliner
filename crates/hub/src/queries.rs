//! One-off reads that belong to no model: pickers, menus, estimates, statuses. Each runs on the
//! background executor; the caller gets the result (or the error to show in place).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use gpui::{App, AppContext as _, Task};

use crate::api::{
    Api, BlobId, ChildDir, Estimate, FolderInfo, GraphChange, GraphDef, InboxItem, Issue, IssueEvent, IssueKey, PermissionRule,
    PipelineId, Problem, ProviderModels, PullRequest, RepoFacts, ReviewId, Run, RunId, RunLogLine, RunStats,
    RunWait, RunnableRef, Session, SessionFilter, SweptWorktree, SessionId, SlashCommand, StartFrom, Template, UsedBy, WorkdirEntry,
};

/// A cheap, clonable handle for reads outside the models.
#[derive(Clone)]
pub struct Queries {
    api: Arc<dyn Api>,
}

impl Queries {
    pub(crate) fn new(api: Arc<dyn Api>) -> Self {
        Self { api }
    }

    fn ask<R: Send + 'static>(&self, cx: &App, f: impl FnOnce(&dyn Api) -> R + Send + 'static) -> Task<R> {
        let api = self.api.clone();
        cx.background_spawn(async move { f(&*api) })
    }

    /// Sessions matching a filter (the archive, a "View all" page). The sidebar lists only the
    /// person's own chats; set `started_by: Some(StartedBy::Agents)` to ask for the sessions
    /// pipeline nodes and sub-agents started.
    pub fn sessions(&self, filter: SessionFilter, cx: &App) -> Task<Result<Vec<Session>>> {
        self.ask(cx, move |api| api.sessions(&filter))
    }

    /// Providers and their live model lists. Reaches the network.
    pub fn models(&self, cx: &App) -> Task<Vec<ProviderModels>> {
        self.ask(cx, |api| api.models())
    }

    /// Files for the `@` menu.
    pub fn mention_files(&self, id: &SessionId, prefix: &str, limit: usize, cx: &App) -> Task<Vec<String>> {
        let (id, prefix) = (id.clone(), prefix.to_string());
        self.ask(cx, move |api| api.mention_files(&id, &prefix, limit))
    }

    /// Commands for the `/` menu.
    pub fn slash_commands(&self, id: &SessionId, cx: &App) -> Task<Vec<SlashCommand>> {
        let id = id.clone();
        self.ask(cx, move |api| api.slash_commands(&id))
    }

    /// Where a blob's bytes are, for showing an image. Cheap: no disk access.
    pub fn blob_path(&self, id: &BlobId) -> PathBuf {
        self.api.blob_path(id)
    }

    /// Folders used recently, most recent first.
    pub fn recent_workdirs(&self, cx: &App) -> Task<Vec<WorkdirEntry>> {
        self.ask(cx, |api| api.recent_workdirs())
    }

    /// Worktrees of a repo, with who works in each.
    pub fn worktrees(&self, repo: &Path, cx: &App) -> Task<Vec<WorkdirEntry>> {
        let repo = repo.to_path_buf();
        self.ask(cx, move |api| api.worktrees(&repo))
    }

    /// Git facts of a folder, when it is a repo.
    pub fn repo_facts(&self, path: &Path, cx: &App) -> Task<Option<RepoFacts>> {
        let path = path.to_path_buf();
        self.ask(cx, move |api| api.repo_facts(&path))
    }

    /// Branches of a repo, for the base-branch picker.
    pub fn branches(&self, repo: &Path, cx: &App) -> Task<Vec<String>> {
        let repo = repo.to_path_buf();
        self.ask(cx, move |api| api.branches(&repo))
    }

    /// What "Found in this folder" shows.
    pub fn folder_info(&self, path: &Path, cx: &App) -> Task<FolderInfo> {
        let path = path.to_path_buf();
        self.ask(cx, move |api| api.folder_info(&path))
    }

    /// Whether a path is an existing folder: a missing folder, as opposed to an empty or
    /// non-git one.
    pub fn folder_exists(&self, path: &Path, cx: &App) -> Task<bool> {
        let path = path.to_path_buf();
        self.ask(cx, move |api| api.folder_exists(&path))
    }

    /// Folders inside `dir` whose name starts with `prefix`, for path completion: visible ones
    /// by name, then hidden ones.
    pub fn list_dir(&self, dir: &Path, prefix: &str, limit: usize, cx: &App) -> Task<Result<Vec<ChildDir>>> {
        let (dir, prefix) = (dir.to_path_buf(), prefix.to_string());
        self.ask(cx, move |api| api.list_dir(&dir, &prefix, limit))
    }

    /// A free branch name for a new chat.
    pub fn suggest_branch(&self, repo: &Path, hint: &str, cx: &App) -> Task<String> {
        let (repo, hint) = (repo.to_path_buf(), hint.to_string());
        self.ask(cx, move |api| api.suggest_branch(&repo, &hint))
    }

    /// One issue, also of a project the board does not show.
    pub fn issue(&self, key: &IssueKey, cx: &App) -> Task<Result<Option<Issue>>> {
        let key = key.clone();
        self.ask(cx, move |api| api.issue(&key))
    }

    /// The activity feed of an issue outside the board's model.
    pub fn issue_events(&self, key: &IssueKey, cx: &App) -> Task<Result<Vec<IssueEvent>>> {
        let key = key.clone();
        self.ask(cx, move |api| api.issue_events(&key))
    }

    /// One run, without opening a [`crate::RunModel`].
    pub fn run(&self, id: &RunId, cx: &App) -> Task<Result<Option<Run>>> {
        let id = id.clone();
        self.ask(cx, move |api| api.run(&id))
    }

    /// Log lines after `after_seq`, oldest first: paging through a log past the model's tail.
    pub fn run_log(&self, id: &RunId, after_seq: u64, limit: usize, cx: &App) -> Task<Result<Vec<RunLogLine>>> {
        let id = id.clone();
        self.ask(cx, move |api| api.run_log(&id, after_seq, limit))
    }

    /// Every run of an issue, for its run history.
    pub fn runs_for_issue(&self, key: &IssueKey, cx: &App) -> Task<Result<Vec<Run>>> {
        let key = key.clone();
        self.ask(cx, move |api| api.runs_for_issue(&key))
    }

    /// The newest `limit` runs of a pipeline, test runs included, newest first (the editor's
    /// "Last test run #N" after a restart).
    pub fn runs_for_pipeline(&self, id: &PipelineId, limit: usize, cx: &App) -> Task<Result<Vec<Run>>> {
        let id = id.clone();
        self.ask(cx, move |api| api.runs_for_pipeline(&id, limit))
    }

    /// Why a run waits for a person, without opening a [`crate::RunModel`].
    pub fn run_waiting(&self, id: &RunId, cx: &App) -> Task<Result<Option<RunWait>>> {
        let id = id.clone();
        self.ask(cx, move |api| api.run_waiting(&id))
    }

    /// What the worktree sweep would remove now, for the settings.
    pub fn planned_sweep(&self, cx: &App) -> Task<Result<Vec<SweptWorktree>>> {
        self.ask(cx, |api| api.sweep_worktrees(true))
    }

    /// The live numbers of a run's header.
    pub fn run_stats(&self, id: &RunId, cx: &App) -> Task<Result<RunStats>> {
        let id = id.clone();
        self.ask(cx, move |api| api.run_stats(&id))
    }

    /// What sending a review back would take.
    pub fn send_back_estimate(&self, id: &ReviewId, start: StartFrom, variants: u32, cx: &App) -> Task<Result<Estimate>> {
        let id = id.clone();
        self.ask(cx, move |api| api.send_back_estimate(&id, start, variants))
    }

    /// The live status of a pull request. Reaches the network.
    pub fn pull_request(&self, url: &str, cx: &App) -> Task<Result<PullRequest>> {
        let url = url.to_string();
        self.ask(cx, move |api| api.pull_request(&url))
    }

    /// What deleting a runnable would break.
    pub fn used_by(&self, reference: &RunnableRef, cx: &App) -> Task<Result<UsedBy>> {
        let reference = reference.clone();
        self.ask(cx, move |api| api.used_by(&reference))
    }

    /// Templates for a new agent or pipeline.
    pub fn templates(&self, cx: &App) -> Task<Vec<Template>> {
        self.ask(cx, |api| api.templates())
    }

    /// What the Problems tab shows for a graph, saved or not.
    pub fn pipeline_problems(&self, graph: GraphDef, cx: &App) -> Task<Vec<Problem>> {
        self.ask(cx, move |api| api.pipeline_problems(&graph))
    }

    /// The draft against the live version.
    pub fn pipeline_changes(&self, id: &PipelineId, cx: &App) -> Task<Result<Vec<GraphChange>>> {
        let id = id.clone();
        self.ask(cx, move |api| api.pipeline_changes(&id))
    }

    /// The saved allow and deny rules, for settings.
    pub fn permission_rules(&self, cx: &App) -> Task<Result<Vec<PermissionRule>>> {
        self.ask(cx, |api| api.permission_rules())
    }

    /// Archived inbox items, newest first (the live ones are in [`crate::InboxModel`]).
    pub fn archived_inbox(&self, cx: &App) -> Task<Result<Vec<InboxItem>>> {
        self.ask(cx, |api| api.inbox(true))
    }
}
