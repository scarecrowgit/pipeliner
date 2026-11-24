//! Commands: what views call to make things happen. Every call returns at once; the work runs on
//! the background executor and a failure comes back as a [`HubNotice`] emitted by the hub.
//! Commands reach the engine one at a time, in the order they were called, so "pin" then
//! "unpin" can never land the other way round.
//!
//! Commands whose result a view needs (a new session to open, a renamed reference) return a
//! `Task<Option<T>>`: `Some` on success, `None` after a failure was already shown. Dropping that
//! task does not cancel the command.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use gpui::{App, BackgroundExecutor, Task, WeakEntity};

use crate::api::{
    Api, Attachment, Autopilot, AutopilotId, ChatMode, ExportFormat, ExportOptions, Issue, IssueKey, IssuePatch,
    ModelRef, NewIssue, NewSession, PermissionDecision, RequestId, RunnableRef, Session, SessionId, UserMessage,
    WorkdirChoice,
};
use crate::hub::Hub;
use crate::notice::HubNotice;

/// One queued engine call.
type Job = Box<dyn FnOnce(&dyn Api) + Send>;

/// A cheap, clonable handle to the engine's commands. One method per [`Api`] command.
#[derive(Clone)]
pub struct Commands {
    pub(crate) api: Arc<dyn Api>,
    pub(crate) hub: WeakEntity<Hub>,
    jobs: async_channel::Sender<Job>,
}

impl Commands {
    /// Starts the worker that runs commands in order. It ends when the last handle is dropped.
    pub(crate) fn new(api: Arc<dyn Api>, hub: WeakEntity<Hub>, executor: &BackgroundExecutor) -> Self {
        let (jobs, queue) = async_channel::unbounded::<Job>();
        let worker = api.clone();
        executor
            .spawn(async move {
                while let Ok(job) = queue.recv().await {
                    job(&*worker);
                }
            })
            .detach();
        Self { api, hub, jobs }
    }

    /// Queues `f` for the command worker; a failure becomes "Could not {action}".
    pub(crate) fn call<R: Send + 'static>(
        &self,
        action: &'static str,
        cx: &mut App,
        f: impl FnOnce(&dyn Api) -> Result<R> + Send + 'static,
    ) -> Task<Option<R>> {
        let hub = self.hub.clone();
        let (done, outcome) = async_channel::bounded::<Result<R>>(1);
        let job: Job = Box::new(move |api| {
            done.try_send(f(api)).ok();
        });
        let queued = self.jobs.try_send(job).is_ok();
        let (sender, receiver) = async_channel::bounded(1);
        cx.spawn(async move |cx| {
            let stopped = || Err(anyhow::anyhow!("the command worker stopped"));
            let result = if queued { outcome.recv().await.unwrap_or_else(|_| stopped()) } else { stopped() };
            match result {
                Ok(value) => {
                    sender.try_send(value).ok();
                }
                Err(error) => {
                    hub.update(cx, |hub, cx| hub.notify_user(HubNotice::failed(action, &error), cx)).ok();
                }
            }
        })
        .detach();
        cx.foreground_executor().spawn(async move { receiver.recv().await.ok() })
    }

    /// Like [`Self::call`] for commands with nothing to return.
    pub(crate) fn fire(
        &self,
        action: &'static str,
        cx: &mut App,
        f: impl FnOnce(&dyn Api) -> Result<()> + Send + 'static,
    ) {
        drop(self.call(action, cx, f));
    }

    // ------------------------------------------------------------ chat

    /// Creates a session (and its worktree); nothing runs until a message is sent.
    pub fn new_session(&self, new: NewSession, cx: &mut App) -> Task<Option<Session>> {
        self.call("start the chat", cx, move |api| api.new_session(new))
    }

    /// Sends a message; a busy session queues it for its next turn.
    pub fn send(&self, id: &SessionId, message: UserMessage, cx: &mut App) {
        let session = id.clone();
        let sent = self.call("send the message", cx, move |api| api.send(&session, message));
        // The queue is not part of the event stream: read it again.
        self.refresh_after(id, sent, cx);
    }

    /// Re-reads an open session's detail once `done` succeeded: for changes the event stream
    /// does not carry (the queue, MCP state, forced skills).
    fn refresh_after(&self, id: &SessionId, done: Task<Option<()>>, cx: &mut App) {
        let hub = self.hub.clone();
        let id = id.clone();
        cx.spawn(async move |cx| {
            if done.await.is_none() {
                return;
            }
            hub.update(cx, |hub, cx| {
                if let Some(model) = hub.sessions.get(&id).and_then(WeakEntity::upgrade) {
                    model.update(cx, |model, cx| model.refresh_detail(cx));
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stops the running turn.
    pub fn interrupt(&self, id: &SessionId, cx: &mut App) {
        let id = id.clone();
        self.fire("stop the agent", cx, move |api| {
            api.interrupt(&id);
            Ok(())
        });
    }

    /// Runs the last failed or interrupted turn again.
    pub fn retry(&self, id: &SessionId, cx: &mut App) {
        let id = id.clone();
        self.fire("retry the turn", cx, move |api| api.retry(&id));
    }

    /// Summarises older turns now.
    pub fn compact(&self, id: &SessionId, cx: &mut App) {
        let id = id.clone();
        self.fire("compact the chat", cx, move |api| api.compact(&id));
    }

    /// Answers a tool call that waits for a person.
    pub fn answer_permission(&self, request: &RequestId, decision: PermissionDecision, cx: &mut App) {
        let request = request.clone();
        self.fire("answer the request", cx, move |api| api.answer_permission(&request, decision));
    }

    /// Switches who answers from the next message on.
    pub fn set_session_runnable(&self, id: &SessionId, runnable: &RunnableRef, mode: ChatMode, cx: &mut App) {
        let (id, runnable) = (id.clone(), runnable.clone());
        self.fire("switch the agent", cx, move |api| api.set_session_runnable(&id, &runnable, mode));
    }

    /// Switches the model from the next message on.
    pub fn set_session_model(&self, id: &SessionId, model: &ModelRef, cx: &mut App) {
        let (id, model) = (id.clone(), model.clone());
        self.fire("switch the model", cx, move |api| api.set_session_model(&id, &model));
    }

    /// Moves the session to another folder; resolves to the updated session.
    pub fn set_session_workdir(&self, id: &SessionId, workdir: WorkdirChoice, cx: &mut App) -> Task<Option<Session>> {
        let id = id.clone();
        self.call("switch the folder", cx, move |api| api.set_session_workdir(&id, workdir))
    }

    /// Sets the skills sent with every request of the session.
    pub fn set_forced_skills(&self, id: &SessionId, skills: Vec<String>, cx: &mut App) {
        let session = id.clone();
        let done = self.call("change the skills", cx, move |api| api.set_forced_skills(&session, skills));
        self.refresh_after(id, done, cx);
    }

    /// Renames a chat.
    pub fn rename_session(&self, id: &SessionId, title: &str, cx: &mut App) {
        let (id, title) = (id.clone(), title.to_string());
        self.fire("rename the chat", cx, move |api| api.rename_session(&id, &title));
    }

    /// Pins or unpins a chat in the sidebar.
    pub fn pin_session(&self, id: &SessionId, pinned: bool, cx: &mut App) {
        let id = id.clone();
        self.fire(if pinned { "pin the chat" } else { "unpin the chat" }, cx, move |api| api.pin_session(&id, pinned));
    }

    /// Archives a chat, or restores it.
    pub fn archive_session(&self, id: &SessionId, archived: bool, cx: &mut App) {
        let id = id.clone();
        let action = if archived { "archive the chat" } else { "restore the chat" };
        self.fire(action, cx, move |api| api.archive_session(&id, archived));
    }

    /// Deletes a chat and its sub-agents.
    pub fn delete_session(&self, id: &SessionId, cx: &mut App) {
        let id = id.clone();
        self.fire("delete the chat", cx, move |api| api.delete_session(&id));
    }

    /// Copies the conversation up to `up_to_seq` into a new chat.
    pub fn fork_session(&self, id: &SessionId, up_to_seq: Option<u64>, cx: &mut App) -> Task<Option<Session>> {
        let id = id.clone();
        self.call("fork the chat", cx, move |api| api.fork_session(&id, up_to_seq))
    }

    /// Writes the transcript to a file; resolves to its path.
    pub fn export_session(
        &self,
        id: &SessionId,
        format: ExportFormat,
        options: ExportOptions,
        to: &Path,
        cx: &mut App,
    ) -> Task<Option<PathBuf>> {
        let (id, to) = (id.clone(), to.to_path_buf());
        self.call("export the chat", cx, move |api| api.export_session(&id, format, options, &to))
    }

    /// Stores image bytes for a message or an issue.
    pub fn put_image(&self, name: &str, media_type: &str, bytes: Vec<u8>, cx: &mut App) -> Task<Option<Attachment>> {
        let (name, media_type) = (name.to_string(), media_type.to_string());
        self.call("attach the image", cx, move |api| api.put_image(&name, &media_type, &bytes))
    }

    /// Restarts an MCP server of the session.
    pub fn reconnect_mcp(&self, id: &SessionId, server: &str, cx: &mut App) {
        let (session, server) = (id.clone(), server.to_string());
        let done = self.call("reconnect the server", cx, move |api| api.reconnect_mcp(&session, &server));
        self.refresh_after(id, done, cx);
    }

    // ------------------------------------------------------------ workdirs

    /// Clones on a worker; progress arrives as notices.
    pub fn clone_repo(&self, url: &str, dest: &Path, cx: &mut App) {
        let (url, dest) = (url.to_string(), dest.to_path_buf());
        self.fire("clone the repository", cx, move |api| api.clone_repo(&url, &dest));
    }

    // ------------------------------------------------------------ issues

    /// Creates an issue; resolves to it.
    pub fn create_issue(&self, new: NewIssue, cx: &mut App) -> Task<Option<Issue>> {
        self.call("create the issue", cx, move |api| api.create_issue(new))
    }

    /// Changes fields of an issue; resolves to the updated issue.
    pub fn update_issue(&self, key: &IssueKey, patch: IssuePatch, cx: &mut App) -> Task<Option<Issue>> {
        let key = key.clone();
        self.call("update the issue", cx, move |api| api.update_issue(&key, patch))
    }

    /// Deletes an issue.
    pub fn delete_issue(&self, key: &IssueKey, cx: &mut App) {
        let key = key.clone();
        self.fire("delete the issue", cx, move |api| api.delete_issue(&key));
    }

    /// Posts a comment on an issue.
    pub fn comment(&self, key: &IssueKey, text: &str, images: Vec<Attachment>, cx: &mut App) {
        let (key, text) = (key.clone(), text.to_string());
        self.fire("post the comment", cx, move |api| api.comment(&key, &text, images));
    }

    /// Attaches a runner to an issue, or detaches it with `None`.
    pub fn attach_runner(&self, key: &IssueKey, runner: Option<RunnableRef>, cx: &mut App) -> Task<Option<Issue>> {
        let key = key.clone();
        let action = if runner.is_some() { "attach it" } else { "detach it" };
        self.call(action, cx, move |api| api.attach_runner(&key, runner))
    }

    /// Starts the attached runner on the issue now.
    pub fn start_issue(&self, key: &IssueKey, cx: &mut App) {
        let key = key.clone();
        self.fire("start the issue", cx, move |api| api.start_issue(&key));
    }

    /// Tries again to wake a runnable whose wake-up failed.
    pub fn retry_wake(&self, key: &IssueKey, target: &RunnableRef, cx: &mut App) {
        let (key, target) = (key.clone(), target.clone());
        self.fire("wake it", cx, move |api| api.retry_wake(&key, &target));
    }

    /// Creates or replaces an autopilot.
    pub fn save_autopilot(&self, autopilot: Autopilot, cx: &mut App) {
        self.fire("save the autopilot", cx, move |api| api.save_autopilot(autopilot));
    }

    /// Deletes an autopilot.
    pub fn delete_autopilot(&self, id: &AutopilotId, cx: &mut App) {
        let id = id.clone();
        self.fire("delete the autopilot", cx, move |api| api.delete_autopilot(&id));
    }
}
