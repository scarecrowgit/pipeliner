//! Per-session working folder: work modes, the git facts cache, switching the workspace
//! (files, context, skills, agents, project MCP servers), moving / forking a chat to
//! another folder, and the pure helpers behind the chip dropdown and the new-chat screen.
//!
//! Every `agent_tools::workdir` call is blocking and runs on a background thread, wrapped
//! in [`guarded`] so a failure there degrades to "unknown" instead of taking the UI down.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_tools::Workspace;
use agent_tools::workdir::{self as wd, FolderScan, GitInfo, Worktree};
use gpui::{AppContext as _, Context, Hsla, Task};
use ui::Status;
use ui::theme::*;

use crate::ChatScreen;
use crate::session::{self, Divider, IndexEntry, Item};

/// How a session works in its folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkMode {
    /// In a git worktree created for this chat (own branch and folder).
    Worktree,
    /// Straight in the folder's checkout.
    #[default]
    InPlace,
    /// Only read-only tools are offered, whatever the agent / Build-Plan mode.
    ReadOnly,
}

impl WorkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkMode::Worktree => "worktree",
            WorkMode::InPlace => "in_place",
            WorkMode::ReadOnly => "read_only",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "worktree" => Some(WorkMode::Worktree),
            "in_place" => Some(WorkMode::InPlace),
            "read_only" => Some(WorkMode::ReadOnly),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WorkMode::Worktree => "worktree",
            WorkMode::InPlace => "in place",
            WorkMode::ReadOnly => "read-only",
        }
    }
}

/// Dropdown rows at most.
const MAX_MENU_ROWS: usize = 8;
/// Sessions read for counts / "also working here".
const SESSION_SCAN_LIMIT: usize = 200;

/// Run a blocking workdir call; a panic becomes `None`.
pub(crate) fn guarded<T>(f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// This machine's name for the local runtime tab.
pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "this machine".into())
}

/// `~/code/x` style path (home replaced by `~`).
pub(crate) fn tilde(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home.filter(|h| !h.as_os_str().is_empty())
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

pub(crate) fn show_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    tilde(path, home.as_deref())
}

pub(crate) fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Git facts of one folder, computed on a background thread.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Fact {
    /// `None`: not a git repository.
    pub git: Option<GitInfo>,
    /// `GitInfo::summary` (empty when not a repo).
    pub summary: String,
}

impl Fact {
    pub(crate) fn read(dir: &Path) -> Fact {
        let git = guarded(|| wd::git_info(dir)).flatten();
        let summary = git
            .as_ref()
            .and_then(|g| guarded(|| g.summary()))
            .unwrap_or_default();
        Fact { git, summary }
    }
}

/// `name · branch` (or the folder name).
pub(crate) fn row_name(dir: &Path, fact: Option<&Fact>) -> String {
    let name = folder_name(dir);
    match fact
        .and_then(|f| f.git.as_ref())
        .and_then(|g| g.branch.as_ref())
    {
        Some(b) => format!("{name} · {b}"),
        None => name,
    }
}

/// The provider pill / provider row of a folder.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProvView {
    pub label: String,
    pub host: String,
    pub state: String,
    pub status: Status,
    pub state_color: Hsla,
}

/// `fact == None`: still loading.
pub(crate) fn prov_view(fact: Option<&Fact>) -> ProvView {
    let Some(fact) = fact else {
        return ProvView {
            label: "…".into(),
            host: String::new(),
            state: "checking…".into(),
            status: Status::Draft,
            state_color: TEXT_MUTED,
        };
    };
    match &fact.git {
        None => ProvView {
            label: "Folder".into(),
            host: "not a git repo".into(),
            state: "no version control".into(),
            status: Status::Draft,
            state_color: TEXT_MUTED,
        },
        Some(g) => match &g.provider {
            Some(p) => {
                let path = g.remote_path.clone().unwrap_or_default();
                let host = match path.rsplit_once('/') {
                    Some((h, _)) => h.to_string(),
                    None => path,
                };
                ProvView {
                    label: p.label().to_string(),
                    host,
                    state: "origin remote set".into(),
                    status: Status::Live,
                    state_color: SUCCESS,
                }
            }
            None => ProvView {
                label: "Local".into(),
                host: "no remote".into(),
                state: "git only · no remote".into(),
                status: Status::Draft,
                state_color: TEXT_MUTED,
            },
        },
    }
}

/// Text color of a row note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteTone {
    Muted,
    Info,
    Warning,
}

impl NoteTone {
    pub fn color(self) -> Hsla {
        match self {
            NoteTone::Muted => TEXT_MUTED,
            NoteTone::Info => INFO,
            NoteTone::Warning => WARNING,
        }
    }
}

/// Right note of a chip-dropdown row. `fact == None`: still loading.
pub(crate) fn menu_note(
    is_current: bool,
    other_chat: Option<&str>,
    fact: Option<&Fact>,
) -> (String, NoteTone) {
    if is_current {
        return ("this chat".into(), NoteTone::Muted);
    }
    if let Some(title) = other_chat {
        return (session::truncate(title, 24), NoteTone::Info);
    }
    match fact {
        None => ("…".into(), NoteTone::Muted),
        Some(Fact { git: None, .. }) => ("no git".into(), NoteTone::Muted),
        Some(Fact { git: Some(g), .. }) if g.changed > 0 => {
            (format!("{} changed", g.changed), NoteTone::Warning)
        }
        Some(_) => ("clean".into(), NoteTone::Muted),
    }
}

/// Right note of a "Recent" row on the new-chat screen.
pub(crate) fn recent_note(changed: usize, sessions: usize) -> (String, NoteTone) {
    if changed > 0 {
        return (format!("{changed} changed"), NoteTone::Warning);
    }
    let text = match sessions {
        0 => "no sessions".to_string(),
        1 => "1 session".to_string(),
        n => format!("{n} sessions"),
    };
    (text, NoteTone::Muted)
}

/// Push `p` unless it is already in `out`.
fn push_unique(out: &mut Vec<PathBuf>, p: &Path) {
    if !out.iter().any(|x| x == p) {
        out.push(p.to_path_buf());
    }
}

/// Folders for the new-chat "Recent" list: recently used, then folders of saved sessions,
/// then the current one (deduplicated, in that order).
pub(crate) fn recent_folders(
    recent: &[PathBuf],
    sessions: &[IndexEntry],
    current: &Path,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in recent {
        push_unique(&mut out, p);
    }
    for p in sessions.iter().filter_map(|s| s.workdir.as_deref()) {
        push_unique(&mut out, p);
    }
    push_unique(&mut out, current);
    out
}

/// Chip dropdown rows: the current folder, recent folders, then the worktrees of the
/// current repo; at most [`MAX_MENU_ROWS`].
pub(crate) fn menu_rows(
    current: &Path,
    recent: &[PathBuf],
    worktrees: &[Worktree],
) -> Vec<PathBuf> {
    let mut out = vec![current.to_path_buf()];
    for p in recent
        .iter()
        .map(PathBuf::as_path)
        .chain(worktrees.iter().map(|w| w.path.as_path()))
    {
        push_unique(&mut out, p);
    }
    out.truncate(MAX_MENU_ROWS);
    out
}

/// Another saved session working in `dir` (not `current_id`), most recent first.
pub(crate) fn other_session<'a>(
    sessions: &'a [IndexEntry],
    dir: &Path,
    current_id: &str,
) -> Option<&'a IndexEntry> {
    sessions
        .iter()
        .find(|s| s.id != current_id && s.workdir.as_deref() == Some(dir))
}

/// Base branch preselected for a new worktree: the repo's default branch, else the
/// current one, else the first.
pub(crate) fn default_base(branches: &[String], git: Option<&GitInfo>) -> Option<String> {
    let has = |b: &String| branches.iter().any(|x| x == b);
    git.and_then(|g| g.default_branch.as_ref().filter(|b| has(b)))
        .or_else(|| git.and_then(|g| g.branch.as_ref().filter(|b| has(b))))
        .or_else(|| branches.first())
        .cloned()
}

/// `name`, or `name-2`, `name-3`… when a branch of that name exists.
pub(crate) fn unique_branch(name: &str, existing: &[String]) -> String {
    if !existing.iter().any(|b| b == name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name}-{i}"))
        .find(|c| !existing.iter().any(|b| b == c))
        .unwrap_or_else(|| name.to_string())
}

/// One "Found in this folder" line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub ok: bool,
    pub key: String,
    pub value: String,
}

pub(crate) fn found_rows(scan: &FolderScan) -> Vec<Found> {
    let row = |ok: bool, key: &str, value: String| Found {
        ok,
        key: key.into(),
        value,
    };
    let mut out = vec![match &scan.instructions {
        Some((name, lines)) => row(true, name, format!("{lines} lines · loaded every session")),
        None => row(false, "AGENTS.md", "missing".into()),
    }];
    if !scan.skills.is_empty() {
        out.push(row(true, "skills", scan.skills.join(", ")));
    }
    if !scan.commands.is_empty() {
        let cmds: Vec<String> = scan.commands.iter().map(|c| format!("/{c}")).collect();
        out.push(row(true, "commands", cmds.join(", ")));
    }
    if !scan.mcp_servers.is_empty() {
        out.push(row(true, ".mcp.json", scan.mcp_servers.join(", ")));
    }
    out.push(match &scan.git_status {
        Some(s) => row(true, "git status", s.clone()),
        None => row(false, "git", "not a repository".into()),
    });
    out
}

/// Workdir state of the chat screen (chip dropdown + shared caches).
pub(crate) struct WdState {
    /// The startup workspace: the folder of sessions saved without one.
    pub default_root: PathBuf,
    pub host: String,
    pub open: bool,
    /// Row picked in the dropdown, awaiting Move / Fork / Cancel.
    pub pending: Option<PathBuf>,
    pub facts: HashMap<PathBuf, Fact>,
    pub recent: Vec<PathBuf>,
    /// Saved sessions (newest first) with their folders.
    pub sessions: Vec<IndexEntry>,
    /// Worktrees of the repo containing the key folder.
    pub worktrees: HashMap<PathBuf, Vec<Worktree>>,
    info_task: Option<Task<()>>,
    refresh_task: Option<Task<()>>,
}

impl WdState {
    pub fn new(default_root: PathBuf) -> Self {
        Self {
            default_root,
            host: hostname(),
            open: false,
            pending: None,
            facts: HashMap::new(),
            recent: Vec::new(),
            sessions: Vec::new(),
            worktrees: HashMap::new(),
            info_task: None,
            refresh_task: None,
        }
    }
}

impl ChatScreen {
    pub(crate) fn cur_fact(&self) -> Option<&Fact> {
        self.wd.facts.get(&self.workspace.root)
    }

    /// Point the session at `dir`: new workspace, then reload files, context, repo label,
    /// git facts, skills, agents and (when the set changed) project MCP servers.
    pub(crate) fn apply_workdir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self.workspace.root == dir {
            return;
        }
        self.workspace = Arc::new(Workspace::new(dir));
        self.repo = None;
        self.files = Arc::new(Vec::new());
        self.context = None;
        self.wd.pending = None;
        self.load_workspace_info(cx);
        self.discover_skills(cx);
        self.discover_agents(cx);
        self.load_mcp(cx);
        cx.notify();
    }

    /// Repo label, @-mention files, context summary and git facts of the workspace.
    pub(crate) fn load_workspace_info(&mut self, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let task = cx.spawn(async move |this, cx| {
            let root = ws.root.clone();
            let (label, mut files, context, fact) = cx
                .background_spawn(async move {
                    (
                        ws.repo_label(),
                        ws.files(2000),
                        ws.context_summary(),
                        Fact::read(&ws.root),
                    )
                })
                .await;
            files.sort();
            this.update(cx, |this, cx| {
                if this.workspace.root != root {
                    return;
                }
                this.repo = Some(label);
                this.files = Arc::new(files);
                this.context = Some(context);
                this.wd.facts.insert(root, fact);
                cx.notify();
            })
            .ok();
        });
        self.wd.info_task = Some(task);
    }

    /// Remember `dir` as recently used (best effort, background).
    pub(crate) fn touch_recent(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let dirs = cx
                .background_spawn(async move {
                    guarded(|| {
                        let mut r = wd::RecentWorkdirs::load();
                        r.touch(&dir);
                        r.dirs
                    })
                })
                .await;
            if let Some(dirs) = dirs {
                this.update(cx, |this, cx| {
                    this.wd.recent = dirs;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Reload recent folders, saved sessions and the current repo's worktrees, then the
    /// git facts of every folder involved (for the dropdown and the new-chat screen).
    pub(crate) fn refresh_workdirs(&mut self, cx: &mut Context<Self>) {
        let root = self.workspace.root.clone();
        let task = cx.spawn(async move |this, cx| {
            let (recent, sessions, trees) = cx
                .background_spawn({
                    let root = root.clone();
                    async move {
                        (
                            guarded(|| wd::RecentWorkdirs::load().dirs).unwrap_or_default(),
                            session::list(&session::sessions_dir(), SESSION_SCAN_LIMIT),
                            guarded(|| wd::worktrees(&root)).unwrap_or_default(),
                        )
                    }
                })
                .await;
            let Ok(dirs) = this.update(cx, |this, cx| {
                this.wd.recent = recent;
                this.wd.sessions = sessions;
                this.wd.worktrees.insert(root.clone(), trees);
                cx.notify();
                let mut dirs = recent_folders(&this.wd.recent, &this.wd.sessions, &root);
                if let Some(t) = this.wd.worktrees.get(&root) {
                    for w in t {
                        push_unique(&mut dirs, &w.path);
                    }
                }
                dirs
            }) else {
                return;
            };
            Self::read_facts(this, dirs, cx).await;
        });
        self.wd.refresh_task = Some(task);
    }

    /// Git facts for `dirs` on a background thread, merged into the cache.
    pub(crate) async fn read_facts(
        this: gpui::WeakEntity<Self>,
        dirs: Vec<PathBuf>,
        cx: &mut gpui::AsyncApp,
    ) {
        let facts = cx
            .background_spawn(async move {
                dirs.into_iter()
                    .map(|d| {
                        let f = Fact::read(&d);
                        (d, f)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        this.update(cx, |this, cx| {
            this.wd.facts.extend(facts);
            cx.notify();
        })
        .ok();
    }

    pub(crate) fn menu_rows(&self) -> Vec<PathBuf> {
        let root = &self.workspace.root;
        let trees = self
            .wd
            .worktrees
            .get(root)
            .map(Vec::as_slice)
            .unwrap_or_default();
        menu_rows(root, &self.wd.recent, trees)
    }

    pub(crate) fn toggle_wd_menu(&mut self, cx: &mut Context<Self>) {
        if self.wd.open {
            self.close_wd_menu(cx);
        } else {
            self.open_wd_menu(cx);
        }
    }

    pub(crate) fn open_wd_menu(&mut self, cx: &mut Context<Self>) {
        self.wd.open = true;
        self.wd.pending = None;
        self.pick_open = false;
        self.model_open = false;
        self.refresh_workdirs(cx);
        cx.notify();
    }

    pub(crate) fn close_wd_menu(&mut self, cx: &mut Context<Self>) {
        self.wd.open = false;
        self.wd.pending = None;
        cx.notify();
    }

    /// The mode a chat gets in `dir` after a move / fork (read-only stays read-only).
    fn mode_for(&self, dir: &Path) -> WorkMode {
        if self.work_mode == WorkMode::ReadOnly {
            return WorkMode::ReadOnly;
        }
        let linked = self
            .wd
            .facts
            .get(dir)
            .and_then(|f| f.git.as_ref())
            .is_some_and(|g| g.is_linked_worktree);
        if linked {
            WorkMode::Worktree
        } else {
            WorkMode::InPlace
        }
    }

    fn workdir_note(&self, dir: &Path, what: &str) -> String {
        let branch = self
            .wd
            .facts
            .get(dir)
            .and_then(|f| f.git.as_ref())
            .and_then(|g| g.branch.clone())
            .map(|b| format!(" (git branch `{b}`)"))
            .unwrap_or_default();
        format!(
            "{what}: the working folder is now {}{branch}. From here on every tool runs \
             there and paths are relative to it; files read earlier may differ there.",
            dir.display()
        )
    }

    /// Same session, new folder; a divider marks the move in the transcript.
    pub(crate) fn move_chat(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.interrupt(cx);
        let fact = self.wd.facts.get(&dir);
        let name = row_name(&dir, fact);
        let text = match fact.map(|f| f.summary.as_str()).filter(|s| !s.is_empty()) {
            Some(s) => format!("Moved to {name} · {s}"),
            None => format!("Moved to {name}"),
        };
        let note = self.workdir_note(&dir, "The user moved this chat to another folder");
        self.items.push(Item::Divider(Divider { text, note }));
        self.work_mode = self.mode_for(&dir);
        self.wd.open = false;
        self.apply_workdir(dir.clone(), cx);
        self.touch_recent(dir, cx);
        self.save_session(cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// New session in `dir` with this transcript's history; this one stays where it is.
    pub(crate) fn fork_chat(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.interrupt(cx);
        self.save_session(cx);
        let history = session::build_history(&self.items);
        let model_label = self
            .model
            .as_ref()
            .map(crate::models::ModelRef::label)
            .unwrap_or_default();
        let title = self.title();
        let mode = self.mode_for(&dir);
        let name = row_name(&dir, self.wd.facts.get(&dir));
        let note = self.workdir_note(&dir, "This chat was forked into another folder");
        self.reset_session_state();
        self.session_id = session::new_id();
        self.persisted = false;
        self.items = session::items_from_history(&history, &model_label);
        self.items.push(Item::Divider(Divider {
            text: format!(
                "Forked from “{}” · now in {name}",
                session::truncate(&title, 40)
            ),
            note,
        }));
        self.work_mode = mode;
        self.wd.open = false;
        self.wd.pending = None;
        self.apply_workdir(dir.clone(), cx);
        self.touch_recent(dir, cx);
        self.save_session(cx);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(branch: Option<&str>, default: Option<&str>) -> GitInfo {
        GitInfo {
            root: "/r".into(),
            main_root: "/r".into(),
            is_linked_worktree: false,
            branch: branch.map(Into::into),
            default_branch: default.map(Into::into),
            remote_url: None,
            provider: None,
            remote_path: None,
            changed: 0,
            ahead: 0,
            behind: 0,
        }
    }

    fn entry(id: &str, dir: Option<&str>) -> IndexEntry {
        IndexEntry {
            id: id.into(),
            title: format!("chat {id}"),
            workdir: dir.map(PathBuf::from),
            updated: 0,
            agent: "Build".into(),
            cost_usd: 0.,
            wd_label: String::new(),
        }
    }

    #[test]
    fn work_mode_round_trips() {
        for m in [WorkMode::Worktree, WorkMode::InPlace, WorkMode::ReadOnly] {
            assert_eq!(WorkMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(WorkMode::parse("x"), None);
        assert_eq!(WorkMode::default(), WorkMode::InPlace);
    }

    #[test]
    fn paths_and_names() {
        let home = Path::new("/home/u");
        assert_eq!(tilde(Path::new("/home/u/code/x"), Some(home)), "~/code/x");
        assert_eq!(tilde(Path::new("/home/u"), Some(home)), "~");
        assert_eq!(tilde(Path::new("/srv/x"), Some(home)), "/srv/x");
        assert_eq!(tilde(Path::new("/srv/x"), None), "/srv/x");
        let fact = Fact {
            git: Some(git(Some("main"), None)),
            summary: "clean on main".into(),
        };
        assert_eq!(row_name(Path::new("/c/api"), Some(&fact)), "api · main");
        assert_eq!(row_name(Path::new("/c/api"), None), "api");
    }

    #[test]
    fn default_base_prefers_default_branch() {
        let branches: Vec<String> = vec!["feat/x".into(), "main".into(), "dev".into()];
        let g = git(Some("feat/x"), Some("main"));
        assert_eq!(default_base(&branches, Some(&g)).as_deref(), Some("main"));
        let g = git(Some("dev"), Some("trunk"));
        assert_eq!(default_base(&branches, Some(&g)).as_deref(), Some("dev"));
        assert_eq!(default_base(&branches, None).as_deref(), Some("feat/x"));
        assert_eq!(default_base(&[], None), None);
    }

    #[test]
    fn unique_branch_appends_a_counter() {
        let existing: Vec<String> = vec!["chat/a".into(), "chat/a-2".into()];
        assert_eq!(unique_branch("chat/b", &existing), "chat/b");
        assert_eq!(unique_branch("chat/a", &existing), "chat/a-3");
    }

    #[test]
    fn notes() {
        let clean = Fact {
            git: Some(git(Some("main"), None)),
            summary: String::new(),
        };
        let mut dirty = clean.clone();
        dirty.git.as_mut().unwrap().changed = 3;
        let plain = Fact::default();
        assert_eq!(
            menu_note(true, Some("x"), None),
            ("this chat".into(), NoteTone::Muted)
        );
        assert_eq!(
            menu_note(false, Some("Other chat"), Some(&clean)).1,
            NoteTone::Info
        );
        assert_eq!(menu_note(false, None, None).0, "…");
        assert_eq!(menu_note(false, None, Some(&plain)).0, "no git");
        assert_eq!(menu_note(false, None, Some(&clean)).0, "clean");
        assert_eq!(
            menu_note(false, None, Some(&dirty)),
            ("3 changed".into(), NoteTone::Warning)
        );
        assert_eq!(recent_note(2, 5), ("2 changed".into(), NoteTone::Warning));
        assert_eq!(recent_note(0, 0).0, "no sessions");
        assert_eq!(recent_note(0, 1).0, "1 session");
        assert_eq!(recent_note(0, 4).0, "4 sessions");
    }

    #[test]
    fn rows_are_deduplicated_and_capped() {
        let recent: Vec<PathBuf> = vec!["/a".into(), "/b".into()];
        let sessions = vec![
            entry("1", Some("/b")),
            entry("2", Some("/c")),
            entry("3", None),
        ];
        assert_eq!(
            recent_folders(&recent, &sessions, Path::new("/a")),
            vec![PathBuf::from("/a"), "/b".into(), "/c".into()]
        );
        assert_eq!(
            recent_folders(&[], &[], Path::new("/x")),
            vec![PathBuf::from("/x")]
        );
        let trees = vec![
            Worktree {
                path: "/a".into(),
                branch: Some("main".into()),
                is_main: true,
            },
            Worktree {
                path: "/wt/1".into(),
                branch: Some("chat/x".into()),
                is_main: false,
            },
        ];
        assert_eq!(
            menu_rows(Path::new("/b"), &recent, &trees),
            vec![PathBuf::from("/b"), "/a".into(), "/wt/1".into()]
        );
        let many: Vec<PathBuf> = (0..20).map(|i| PathBuf::from(format!("/d{i}"))).collect();
        assert_eq!(menu_rows(Path::new("/x"), &many, &[]).len(), MAX_MENU_ROWS);
        let other = other_session(&sessions, Path::new("/b"), "2").map(|e| e.id.as_str());
        assert_eq!(other, Some("1"));
        assert!(other_session(&sessions, Path::new("/b"), "1").is_none());
    }

    #[test]
    fn found_in_folder() {
        let scan = FolderScan {
            instructions: Some(("AGENTS.md".into(), 32)),
            skills: vec!["a".into(), "b".into()],
            commands: vec!["release".into()],
            mcp_servers: Vec::new(),
            git_status: Some("clean on main".into()),
        };
        let rows = found_rows(&scan);
        let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, ["AGENTS.md", "skills", "commands", "git status"]);
        assert_eq!(rows[0].value, "32 lines · loaded every session");
        assert_eq!(rows[2].value, "/release");
        let empty = found_rows(&FolderScan::default());
        assert_eq!(empty.len(), 2);
        assert!(empty.iter().all(|r| !r.ok));
    }

    #[test]
    fn provider_views() {
        assert_eq!(prov_view(None).state, "checking…");
        assert_eq!(prov_view(Some(&Fact::default())).label, "Folder");
        let mut g = git(Some("main"), None);
        assert_eq!(
            prov_view(Some(&Fact {
                git: Some(g.clone()),
                summary: String::new()
            }))
            .label,
            "Local"
        );
        g.provider = Some(wd::Provider::GitHub);
        g.remote_path = Some("github.com/acme/api".into());
        let v = prov_view(Some(&Fact {
            git: Some(g),
            summary: String::new(),
        }));
        assert_eq!(
            (v.label.as_str(), v.host.as_str()),
            ("GitHub", "github.com/acme")
        );
        assert_eq!(v.status, Status::Live);
    }
}
