//! "New chat · pick a workdir": the screen shown in the chat area before a new session
//! starts. Runtime tabs (only the local machine is real), folder search / typed paths,
//! Recent folders and the selected repo's worktrees, Open folder… / Clone…, and the
//! selected folder's details: provider, other chats in the repo, how to work there
//! (new worktree / in place / read-only), what the agent will find, and Start.

use std::path::{Path, PathBuf};

use agent_tools::workdir::{self as wd, FolderScan, Worktree};
use gpui::{
    AnyElement, Context, ElementId, Entity, Focusable, FontWeight, IntoElement, PathPromptOptions,
    SharedString, Task, Window, deferred, div, prelude::*, px,
};
use ui::{Badge, Button, Status, TextInput, TextInputEvent, Tone, icon, kbd, status_dot, theme::*};

use crate::ChatScreen;
use crate::widgets::{prov_pill, spinner};
use crate::workdir::{
    Fact, Found, WorkMode, default_base, folder_name, found_rows, guarded, other_session,
    prov_view, recent_folders, recent_note, show_path, unique_branch,
};

fn eid(name: &'static str, ix: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), ix as u64)
}

/// The selected folder's details, read on a background thread.
pub(crate) struct Detail {
    pub dir: PathBuf,
    pub branches: Vec<String>,
    pub scan: FolderScan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    Folder,
    Tree,
    /// A typed path ("Open <path>").
    Open,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub path: PathBuf,
    pub kind: RowKind,
    /// Branch of a worktree row.
    pub branch: Option<String>,
}

pub(crate) struct Group {
    pub name: String,
    pub hint: String,
    pub rows: Vec<Row>,
}

/// Case-insensitive match of the search query on a row's name and shown path.
pub(crate) fn row_matches(query: &str, name: &str, shown: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || format!("{name} {shown}").to_lowercase().contains(&q)
}

/// The query looks like a path worth resolving.
pub(crate) fn looks_like_path(q: &str) -> bool {
    let q = q.trim();
    q.starts_with('/') || q.starts_with('~') || q.starts_with("./") || q.starts_with("../")
}

pub(crate) struct NewChat {
    pub query: String,
    pub selected: Option<PathBuf>,
    /// The typed query resolved to a directory.
    pub typed: Option<PathBuf>,
    /// Folders opened / cloned on this screen (listed first).
    pub added: Vec<PathBuf>,
    pub detail: Option<Detail>,
    /// Worktrees of the selected folder's repo.
    pub trees: Vec<Worktree>,
    pub mode: WorkMode,
    /// The user picked a mode (keep it when the selection changes).
    pub mode_touched: bool,
    pub base: Option<String>,
    pub base_open: bool,
    pub branch_edited: bool,
    pub clone_open: bool,
    /// Work in progress ("Creating worktree…"): Start is disabled.
    pub busy: Option<String>,
    pub error: Option<String>,
    resolve_task: Option<Task<()>>,
    detail_task: Option<Task<()>>,
    job_task: Option<Task<()>>,
}

impl NewChat {
    fn new() -> Self {
        Self {
            query: String::new(),
            selected: None,
            typed: None,
            added: Vec::new(),
            detail: None,
            trees: Vec::new(),
            mode: WorkMode::Worktree,
            mode_touched: false,
            base: None,
            base_open: false,
            branch_edited: false,
            clone_open: false,
            busy: None,
            error: None,
            resolve_task: None,
            detail_task: None,
            job_task: None,
        }
    }
}

/// One "How to work here" card.
struct ModeCard {
    mode: WorkMode,
    title: String,
    desc: String,
    rec: bool,
    disabled: bool,
}

impl ChatScreen {
    // ── Open / close ────────────────────────────────────────────────────

    pub(crate) fn open_new_chat(&mut self, cx: &mut Context<Self>) {
        if self.newchat.is_some() {
            return;
        }
        self.newchat = Some(NewChat::new());
        self.wd.open = false;
        self.wd.pending = None;
        self.pick_open = false;
        self.model_open = false;
        for input in [&self.nc_query, &self.nc_branch, &self.nc_clone] {
            input.update(cx, |i, cx| i.set_text("", cx));
        }
        self.focus_next = Some(self.nc_query.focus_handle(cx));
        self.refresh_workdirs(cx);
        let root = self.workspace.root.clone();
        self.nc_select(root, cx);
        cx.notify();
    }

    pub(crate) fn close_new_chat(&mut self, cx: &mut Context<Self>) {
        if self.newchat.take().is_some() {
            self.focus_next = Some(self.input.focus_handle(cx));
            cx.notify();
        }
    }

    /// Leave the screen into a fresh session in `dir`.
    fn start_session_in(
        &mut self,
        dir: PathBuf,
        mode: WorkMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.newchat = None;
        self.new_session(cx);
        self.work_mode = mode;
        self.apply_workdir(dir.clone(), cx);
        self.touch_recent(dir, cx);
        self.focus_input(window, cx);
        cx.notify();
    }

    // ── Selection ───────────────────────────────────────────────────────

    pub(crate) fn nc_select(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let draft = self.draft.trim().to_string();
        let Some(nc) = self.newchat.as_mut() else {
            return;
        };
        if nc.selected.as_ref() == Some(&dir) && nc.detail.is_some() {
            return;
        }
        nc.selected = Some(dir.clone());
        nc.detail = None;
        nc.base = None;
        nc.base_open = false;
        nc.error = None;
        let task = cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let (fact, branches, scan, trees, suggested) = cx
                .background_spawn(async move {
                    (
                        Fact::read(&d),
                        guarded(|| wd::branches(&d)).unwrap_or_default(),
                        guarded(|| wd::scan_folder(&d)).unwrap_or_default(),
                        guarded(|| wd::worktrees(&d)).unwrap_or_default(),
                        guarded(|| wd::suggest_branch(&draft))
                            .unwrap_or_else(|| "chat/session".into()),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let is_git = fact.git.is_some();
                let base = default_base(&branches, fact.git.as_ref());
                this.wd.facts.insert(dir.clone(), fact);
                let Some(nc) = this.newchat.as_mut() else {
                    return;
                };
                if nc.selected.as_ref() != Some(&dir) {
                    return;
                }
                if !nc.mode_touched || (nc.mode == WorkMode::Worktree && !is_git) {
                    nc.mode = if is_git {
                        WorkMode::Worktree
                    } else {
                        WorkMode::InPlace
                    };
                }
                nc.base = base;
                nc.trees = trees;
                let edited = nc.branch_edited;
                let branch = unique_branch(&suggested, &branches);
                nc.detail = Some(Detail {
                    dir,
                    branches,
                    scan,
                });
                if !edited {
                    this.nc_branch.update(cx, |i, cx| i.set_text(branch, cx));
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(nc) = self.newchat.as_mut() {
            nc.detail_task = Some(task);
        }
        cx.notify();
    }

    /// Rows by group, filtered by the query.
    pub(crate) fn nc_groups(&self) -> Vec<Group> {
        let Some(nc) = &self.newchat else {
            return Vec::new();
        };
        let mut groups = Vec::new();
        if let Some(p) = &nc.typed {
            groups.push(Group {
                name: "Path".into(),
                hint: String::new(),
                rows: vec![Row {
                    path: p.clone(),
                    kind: RowKind::Open,
                    branch: None,
                }],
            });
        }
        let mut folders = nc.added.clone();
        for p in recent_folders(&self.wd.recent, &self.wd.sessions, &self.workspace.root) {
            if !folders.contains(&p) {
                folders.push(p);
            }
        }
        let hint = folders
            .first()
            .and_then(|p| p.parent())
            .map(show_path)
            .unwrap_or_default();
        let recent: Vec<Row> = folders
            .iter()
            .filter(|p| row_matches(&nc.query, &folder_name(p), &show_path(p)))
            .map(|p| Row {
                path: p.clone(),
                kind: RowKind::Folder,
                branch: None,
            })
            .collect();
        if !recent.is_empty() {
            groups.push(Group {
                name: "Recent".into(),
                hint,
                rows: recent,
            });
        }
        let linked: Vec<&Worktree> = nc.trees.iter().filter(|w| !w.is_main).collect();
        let repo = nc
            .trees
            .iter()
            .find(|w| w.is_main)
            .map(|w| folder_name(&w.path))
            .unwrap_or_default();
        let trees: Vec<Row> = linked
            .iter()
            .filter(|w| !folders.contains(&w.path))
            .filter(|w| {
                let name = w.branch.clone().unwrap_or_else(|| folder_name(&w.path));
                row_matches(&nc.query, &name, &show_path(&w.path))
            })
            .map(|w| Row {
                path: w.path.clone(),
                kind: RowKind::Tree,
                branch: w.branch.clone(),
            })
            .collect();
        if !trees.is_empty() {
            let n = linked.len();
            groups.push(Group {
                name: format!("Worktrees · {repo}"),
                hint: if trees.len() == n {
                    format!("{n}")
                } else {
                    format!("{} of {n}", trees.len())
                },
                rows: trees,
            });
        }
        groups
    }

    fn nc_rows(&self) -> Vec<Row> {
        self.nc_groups().into_iter().flat_map(|g| g.rows).collect()
    }

    fn nc_move(&mut self, up: bool, cx: &mut Context<Self>) {
        let rows = self.nc_rows();
        if rows.is_empty() {
            return;
        }
        let cur = self
            .newchat
            .as_ref()
            .and_then(|nc| nc.selected.as_ref())
            .and_then(|s| rows.iter().position(|r| &r.path == s));
        let n = rows.len();
        let next = match cur {
            None => 0,
            Some(i) if up => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
        };
        self.nc_select(rows[next].path.clone(), cx);
    }

    /// The effective mode (a disabled pick falls back to the first enabled card).
    pub(crate) fn nc_mode(&self) -> WorkMode {
        let cards = self.mode_cards();
        let want = self.newchat.as_ref().map_or(WorkMode::InPlace, |n| n.mode);
        cards
            .iter()
            .find(|c| c.mode == want && !c.disabled)
            .or_else(|| cards.iter().find(|c| !c.disabled))
            .map_or(WorkMode::InPlace, |c| c.mode)
    }

    fn sel_fact(&self) -> Option<&Fact> {
        let sel = self.newchat.as_ref()?.selected.as_ref()?;
        self.wd.facts.get(sel)
    }

    fn mode_cards(&self) -> Vec<ModeCard> {
        let fact = self.sel_fact();
        let git = fact.and_then(|f| f.git.as_ref());
        let linked = git.is_some_and(|g| g.is_linked_worktree);
        let changed = git.map_or(0, |g| g.changed);
        let in_place_title = match git.and_then(|g| g.branch.as_ref()) {
            Some(b) if linked => format!("Join this worktree on {b}"),
            Some(b) => format!("In place on {b}"),
            None => "In place".into(),
        };
        let in_place_desc = if changed > 0 {
            format!(
                "Uses the {changed} uncommitted file{} as they are.",
                if changed == 1 { "" } else { "s" }
            )
        } else if linked {
            "Pick up where the work in this worktree left off.".into()
        } else {
            "Edits land straight in your checkout.".into()
        };
        vec![
            ModeCard {
                mode: WorkMode::Worktree,
                title: "New worktree".into(),
                desc: "Own branch and folder. Other agents in this repo are not affected.".into(),
                rec: git.is_some(),
                disabled: git.is_none(),
            },
            ModeCard {
                mode: WorkMode::InPlace,
                title: in_place_title,
                desc: in_place_desc,
                rec: false,
                disabled: false,
            },
            ModeCard {
                mode: WorkMode::ReadOnly,
                title: "Read-only".into(),
                desc: "Nothing is written. Good for Plan and for questions about the code.".into(),
                rec: false,
                disabled: false,
            },
        ]
    }

    fn nc_pick_mode(&mut self, mode: WorkMode, cx: &mut Context<Self>) {
        if let Some(nc) = self.newchat.as_mut() {
            nc.mode = mode;
            nc.mode_touched = true;
            nc.error = None;
            cx.notify();
        }
    }

    // ── Input events ────────────────────────────────────────────────────

    pub(crate) fn on_nc_query_event(
        &mut self,
        input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                let q = input.read(cx).text().to_string();
                self.nc_set_query(q, cx);
            }
            TextInputEvent::Up => self.nc_move(true, cx),
            TextInputEvent::Down => self.nc_move(false, cx),
            TextInputEvent::Submit => {
                let typed = self.newchat.as_ref().and_then(|n| n.typed.clone());
                let selected = self.newchat.as_ref().and_then(|n| n.selected.clone());
                match typed {
                    Some(p) if selected.as_ref() != Some(&p) => self.nc_add_folder(p, cx),
                    _ => self.nc_start(window, cx),
                }
            }
            TextInputEvent::Cancel => self.close_new_chat(cx),
            _ => {}
        }
    }

    fn nc_set_query(&mut self, q: String, cx: &mut Context<Self>) {
        let Some(nc) = self.newchat.as_mut() else {
            return;
        };
        nc.query = q.clone();
        nc.typed = None;
        if looks_like_path(&q) {
            let task = cx.spawn(async move |this, cx| {
                let q2 = q.clone();
                let dir = cx
                    .background_spawn(async move { guarded(|| wd::resolve_dir(&q2)).flatten() })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(nc) = this.newchat.as_mut()
                        && nc.query == q
                    {
                        nc.typed = dir;
                        cx.notify();
                    }
                })
                .ok();
            });
            nc.resolve_task = Some(task);
        }
        cx.notify();
    }

    pub(crate) fn on_nc_branch_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Changed => {
                if let Some(nc) = self.newchat.as_mut() {
                    nc.branch_edited = true;
                    nc.error = None;
                }
            }
            TextInputEvent::Submit => self.nc_start(window, cx),
            TextInputEvent::Cancel => self.close_new_chat(cx),
            _ => {}
        }
    }

    pub(crate) fn on_nc_clone_event(
        &mut self,
        _input: &Entity<TextInput>,
        event: &TextInputEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextInputEvent::Submit => self.nc_clone(cx),
            TextInputEvent::Cancel => {
                if let Some(nc) = self.newchat.as_mut() {
                    nc.clone_open = false;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    // ── Actions ─────────────────────────────────────────────────────────

    fn nc_add_folder(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if let Some(nc) = self.newchat.as_mut() {
            nc.added.retain(|p| p != &dir);
            nc.added.insert(0, dir.clone());
        }
        self.nc_select(dir, cx);
    }

    fn nc_open_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    let msg = format!("Could not open the folder picker: {e:#}");
                    this.update(cx, |this, cx| {
                        if let Some(nc) = this.newchat.as_mut() {
                            nc.error = Some(msg);
                            cx.notify();
                        }
                    })
                    .ok();
                    None
                }
                _ => None,
            };
            if let Some(p) = picked {
                this.update(cx, |this, cx| this.nc_add_folder(p, cx)).ok();
            }
        })
        .detach();
    }

    /// Where clones go: next to the most recent folder (not a worktree), else `~/code`.
    fn clone_parent(&self) -> PathBuf {
        let recent = recent_folders(&self.wd.recent, &self.wd.sessions, &self.workspace.root);
        recent
            .iter()
            .find(|p| {
                self.wd
                    .facts
                    .get(*p)
                    .and_then(|f| f.git.as_ref())
                    .is_none_or(|g| !g.is_linked_worktree)
            })
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var("HOME").unwrap_or_default()).join("code")
            })
    }

    fn nc_clone(&mut self, cx: &mut Context<Self>) {
        let spec = self.nc_clone.read(cx).text().trim().to_string();
        if spec.is_empty() || self.newchat.as_ref().is_none_or(|n| n.busy.is_some()) {
            return;
        }
        let parent = self.clone_parent();
        let Some(nc) = self.newchat.as_mut() else {
            return;
        };
        nc.busy = Some(format!("Cloning {spec} into {}…", show_path(&parent)));
        nc.error = None;
        let task = cx.spawn(async move |this, cx| {
            let res = cx
                .background_spawn(async move {
                    std::fs::create_dir_all(&parent).ok();
                    guarded(|| wd::clone_repo(&spec, &parent).map_err(|e| format!("{e:#}")))
                        .unwrap_or_else(|| Err("clone failed (internal error)".into()))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(nc) = this.newchat.as_mut() else {
                    return;
                };
                nc.busy = None;
                match res {
                    Ok(dir) => {
                        nc.clone_open = false;
                        this.nc_clone.update(cx, |i, cx| i.set_text("", cx));
                        this.nc_add_folder(dir, cx);
                    }
                    Err(e) => nc.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        });
        nc.job_task = Some(task);
        cx.notify();
    }

    pub(crate) fn nc_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.nc_mode();
        let branch = self.nc_branch.read(cx).text().trim().to_string();
        let Some(nc) = self.newchat.as_mut() else {
            return;
        };
        if nc.busy.is_some() {
            return;
        }
        let Some(dir) = nc.selected.clone() else {
            return;
        };
        if mode != WorkMode::Worktree {
            self.start_session_in(dir, mode, window, cx);
            return;
        }
        let Some(base) = nc.base.clone() else {
            nc.error = Some("Pick a branch to start the worktree from.".into());
            cx.notify();
            return;
        };
        if branch.is_empty() {
            nc.error = Some("Name the new branch.".into());
            cx.notify();
            return;
        }
        nc.busy = Some(format!("Creating worktree {branch} from {base}…"));
        nc.error = None;
        let task = cx.spawn_in(window, async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    guarded(|| {
                        wd::create_worktree(&dir, &base, &branch).map_err(|e| format!("{e:#}"))
                    })
                    .unwrap_or_else(|| Err("could not create the worktree (internal error)".into()))
                })
                .await;
            this.update_in(cx, |this, window, cx| match res {
                Ok(path) => this.start_session_in(path, WorkMode::Worktree, window, cx),
                Err(e) => {
                    if let Some(nc) = this.newchat.as_mut() {
                        nc.busy = None;
                        nc.error = Some(e);
                        cx.notify();
                    }
                }
            })
            .ok();
        });
        if let Some(nc) = self.newchat.as_mut() {
            nc.job_task = Some(task);
        }
        cx.notify();
    }

    // ── Rendering ───────────────────────────────────────────────────────

    pub(crate) fn render_new_chat(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(nc) = &self.newchat else {
            return div().into_any_element();
        };
        let hints = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(16.))
            .gap_y(px(8.))
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(kbd("↑"))
                    .child(kbd("↓"))
                    .child("move"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(kbd("Enter"))
                    .child("start"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(kbd("Esc"))
                    .child("back to the chat"),
            )
            .child("Default for new chats: the last used folder");
        div()
            .id("new-chat")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(24.))
            .py(px(32.))
            .child(
                div()
                    .w_full()
                    .max_w(px(1000.))
                    .mx_auto()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .line_height(px(28.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(TEXT)
                                    .child(format!("Where should {} work?", self.agent_name())),
                            )
                            .child(div().text_color(TEXT_SECONDARY).child(
                                "The agent sees only this folder. Pick a runtime, then a repo or worktree.",
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            // Inside the scrolling column: never shrink to the viewport
                            // (overflow_hidden would clip the Start button).
                            .flex_shrink_0()
                            .border_1()
                            .border_color(LINE)
                            .rounded(px(10.))
                            .bg(SURFACE)
                            .overflow_hidden()
                            .child(self.nc_left(nc, cx))
                            .child(self.nc_right(nc, cx)),
                    )
                    .child(hints),
            )
            .into_any_element()
    }

    fn runtime_tab(&self, name: String, sub: &str, on: bool) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(2.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(if on {
                LINE_STRONG
            } else {
                gpui::transparent_black()
            })
            .when(on, |d| d.bg(SURFACE_HOVER))
            .when(!on, |d| d.opacity(0.55))
            .text_color(if on { TEXT } else { TEXT_SECONDARY })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(status_dot(
                        if on { Status::Live } else { Status::Draft },
                        true,
                    ))
                    .child(div().truncate().child(name)),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(TEXT_MUTED)
                    .child(sub.to_string()),
            )
    }

    fn nc_left(&self, nc: &NewChat, cx: &mut Context<Self>) -> impl IntoElement {
        let runtimes = div()
            .flex()
            .gap(px(4.))
            .p(px(10.))
            .border_b_1()
            .border_color(LINE)
            .child(self.runtime_tab(self.wd.host.clone(), "local · online", true))
            .child(self.runtime_tab("Cloud sandbox".into(), "soon", false))
            .child(self.runtime_tab("Remote machine".into(), "soon", false));
        let search = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(40.))
            .pl(px(14.))
            .pr(px(12.))
            .border_b_1()
            .border_color(LINE)
            .text_color(TEXT)
            .child(icon("search", 14., TEXT_MUTED))
            .child(div().flex_1().h(px(20.)).child(self.nc_query.clone()));

        let groups = self.nc_groups();
        let mut list: Vec<AnyElement> = Vec::new();
        let mut ix = 0;
        for g in &groups {
            list.push(
                div()
                    .flex()
                    .justify_between()
                    .px(px(8.))
                    .pt(px(10.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(g.name.clone())
                    .child(g.hint.clone())
                    .into_any_element(),
            );
            for r in &g.rows {
                list.push(self.nc_row(ix, r, nc, cx));
                ix += 1;
            }
        }
        if groups.is_empty() {
            list.push(
                div()
                    .px(px(8.))
                    .py(px(16.))
                    .text_color(TEXT_MUTED)
                    .child(format!(
                        "Nothing matches “{}”. Paste a full path or clone a repo.",
                        nc.query
                    ))
                    .into_any_element(),
            );
        }

        let footer = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .border_t_1()
            .border_color(LINE)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        Button::new("nc-open-folder", "Open folder…")
                            .small()
                            .ghost()
                            .icon("plus")
                            .on_click(cx.listener(|this, _, _, cx| this.nc_open_folder(cx))),
                    )
                    .child(
                        Button::new("nc-clone", "Clone from GitHub…")
                            .small()
                            .ghost()
                            .icon("source")
                            .selected(nc.clone_open)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(nc) = this.newchat.as_mut() {
                                    nc.clone_open = !nc.clone_open;
                                    if nc.clone_open {
                                        this.nc_clone.focus_handle(cx).focus(window);
                                    }
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .when(nc.clone_open, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .h(px(32.))
                                .px(px(10.))
                                .rounded(px(RADIUS_MD))
                                .border_1()
                                .border_color(LINE_STRONG)
                                .bg(SURFACE_RAISED)
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT)
                                .child(div().flex_1().h(px(18.)).child(self.nc_clone.clone())),
                        )
                        .child(
                            Button::new("nc-clone-go", "Clone")
                                .small()
                                .primary()
                                .disabled(nc.busy.is_some())
                                .on_click(cx.listener(|this, _, _, cx| this.nc_clone(cx))),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(TEXT_MUTED)
                        .child(format!("Clones into {}", show_path(&self.clone_parent()))),
                )
            });

        div()
            .flex_basis(px(360.))
            .flex_grow()
            .flex_shrink()
            .min_w_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(LINE)
            .child(runtimes)
            .child(search)
            .child(
                div()
                    .id("nc-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .max_h(px(470.))
                    .overflow_y_scroll()
                    .px(px(6.))
                    .pt(px(4.))
                    .pb(px(8.))
                    .children(list),
            )
            .child(footer)
    }

    fn nc_row(&self, ix: usize, r: &Row, nc: &NewChat, cx: &mut Context<Self>) -> AnyElement {
        let on = nc.selected.as_ref() == Some(&r.path);
        let fact = self.wd.facts.get(&r.path);
        let git = fact.and_then(|f| f.git.as_ref());
        let pv = prov_view(fact);
        let (name, tile_icon, note, tone, pad) = match r.kind {
            RowKind::Open => (
                format!("Open {}", show_path(&r.path)),
                "folder",
                String::new(),
                crate::workdir::NoteTone::Muted,
                8.,
            ),
            RowKind::Folder => {
                let sessions = self
                    .wd
                    .sessions
                    .iter()
                    .filter(|s| s.workdir.as_deref() == Some(r.path.as_path()))
                    .count();
                let (note, tone) = recent_note(git.map_or(0, |g| g.changed), sessions);
                (folder_name(&r.path), "folder", note, tone, 8.)
            }
            RowKind::Tree => {
                let (note, tone) = match other_session(&self.wd.sessions, &r.path, &self.session_id)
                {
                    Some(s) => (
                        crate::session::truncate(&s.title, 24),
                        crate::workdir::NoteTone::Info,
                    ),
                    None if r.path == self.workspace.root => {
                        ("this chat".into(), crate::workdir::NoteTone::Muted)
                    }
                    None => ("worktree".into(), crate::workdir::NoteTone::Muted),
                };
                (
                    r.branch.clone().unwrap_or_else(|| folder_name(&r.path)),
                    "git-branch",
                    note,
                    tone,
                    26.,
                )
            }
        };
        let branch = match r.kind {
            RowKind::Folder => git.and_then(|g| g.branch.clone()).unwrap_or_default(),
            _ => String::new(),
        };
        let path = r.path.clone();
        let kind = r.kind;
        div()
            .id(eid("nc-row", ix))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .min_h(px(48.))
            .pl(px(pad))
            .pr(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(if on {
                ACCENT_RING
            } else {
                gpui::transparent_black()
            })
            .when(on, |d| d.bg(SURFACE_HOVER))
            .when(!on, |d| d.hover(|s| s.bg(SURFACE_HOVER)))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if kind == RowKind::Open {
                    this.nc_add_folder(path.clone(), cx);
                } else {
                    this.nc_select(path.clone(), cx);
                }
            }))
            .child(
                div()
                    .size(px(26.))
                    .flex_none()
                    .rounded(px(6.))
                    .bg(SURFACE_HOVER)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(tile_icon, 14., TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT)
                                    .child(name),
                            )
                            .when(r.kind != RowKind::Open, |d| {
                                d.child(prov_pill(pv.label.clone(), pv.status))
                            })
                            .when(!branch.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .font_family(FONT_MONO)
                                        .text_size(px(11.))
                                        .text_color(TEXT_MUTED)
                                        .whitespace_nowrap()
                                        .child(branch),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .child(match r.kind {
                                RowKind::Open => "folder on this machine".to_string(),
                                _ => show_path(&r.path),
                            }),
                    ),
            )
            .when(!note.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(tone.color())
                        .whitespace_nowrap()
                        .child(note),
                )
            })
            .into_any_element()
    }

    fn nc_right(&self, nc: &NewChat, cx: &mut Context<Self>) -> impl IntoElement {
        let col = div()
            .flex_basis(px(400.))
            .flex_grow()
            .flex_shrink()
            .min_w_0()
            .flex()
            .flex_col();
        let Some(sel) = nc.selected.clone() else {
            return col.child(
                div()
                    .p(px(20.))
                    .text_color(TEXT_MUTED)
                    .child("Pick a folder on the left."),
            );
        };
        let fact = self.wd.facts.get(&sel);
        let git = fact.and_then(|f| f.git.as_ref());
        let pv = prov_view(fact);
        let name = folder_name(&sel);

        let head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded(px(8.))
                    .bg(SURFACE_HOVER)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("folder", 16., TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(18.))
                            .line_height(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(TEXT)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(format!("{} · {}", self.wd.host, show_path(&sel))),
                    ),
            );

        let prov = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(12.))
            .gap_y(px(8.))
            .px(px(20.))
            .py(px(12.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .child(status_dot(pv.status, true))
                    .child(pv.label.clone()),
            )
            .child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(pv.host.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(pv.state_color)
                    .child(pv.state.clone()),
            );

        // Other saved sessions whose folder is in the same repository.
        let main_root = git.map(|g| g.main_root.clone());
        let busy: Vec<(String, String)> = match &main_root {
            None => Vec::new(),
            Some(main) => self
                .wd
                .sessions
                .iter()
                .filter_map(|s| {
                    let dir = s.workdir.as_ref()?;
                    let g = self.wd.facts.get(dir)?.git.as_ref()?;
                    (&g.main_root == main).then(|| {
                        let where_ = match (&g.branch, g.is_linked_worktree) {
                            (Some(b), true) => format!("worktree {b}"),
                            (Some(b), false) => format!("{} · {b}", folder_name(dir)),
                            (None, _) => show_path(dir),
                        };
                        (crate::session::truncate(&s.title, 40), where_)
                    })
                })
                .take(5)
                .collect(),
        };
        let also = (!busy.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(20.))
                .py(px(12.))
                .border_b_1()
                .border_color(LINE)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child("Also working in this repo"),
                )
                .children(busy.into_iter().map(|(who, where_)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .text_size(px(12.))
                        .child(status_dot(Status::Running, true))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(220.))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(TEXT)
                                .child(who),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(FONT_MONO)
                                .text_color(TEXT_MUTED)
                                .child(where_),
                        )
                }))
        });

        let mode = self.nc_mode();
        let mut card_els = Vec::new();
        for (i, c) in self.mode_cards().into_iter().enumerate() {
            card_els.push(self.mode_card(i, c, mode, cx));
        }
        let how = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .mb(px(4.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .child("How to work here"),
            )
            .children(card_els)
            .when(mode == WorkMode::Worktree, |d| {
                d.child(self.branch_fields(nc, cx))
            });

        let found = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(TEXT)
                    .child("Found in this folder"),
            )
            .map(|d| match nc.detail.as_ref().filter(|det| det.dir == sel) {
                None => d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child(spinner("nc-scan", 12., TEXT_MUTED))
                        .child("Looking around…"),
                ),
                Some(det) => d.children(found_rows(&det.scan).into_iter().map(found_line)),
            });

        let mode_text = match mode {
            WorkMode::Worktree => format!(
                "new worktree {} from {}",
                self.nc_branch.read(cx).text(),
                nc.base.clone().unwrap_or_else(|| "…".into())
            ),
            WorkMode::InPlace => "in place".into(),
            WorkMode::ReadOnly => "read-only".into(),
        };
        let summary = nc
            .busy
            .clone()
            .unwrap_or_else(|| format!("{name} on {} · {mode_text}", self.wd.host));
        let start = div()
            .mt_auto()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(20.))
            .py(px(14.))
            .children(nc.error.clone().map(|e| {
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(DANGER)
                    .child(e)
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_basis(px(200.))
                            .flex_grow()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .when(nc.busy.is_some(), |d| {
                                d.child(spinner("nc-busy", 12., TEXT_MUTED))
                            })
                            .child(summary),
                    )
                    .child(
                        Button::new("nc-start", format!("Start in {name}"))
                            .primary()
                            .icon("play")
                            .disabled(nc.busy.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.nc_start(window, cx))),
                    ),
            );

        // gpui sizes this row from a one-line measurement of the wrapped card texts, so the
        // column can end up shorter than its content: the details scroll inside the column
        // and the summary + Start button stay pinned at its bottom.
        let body = div()
            .flex()
            .flex_col()
            .child(head)
            .child(prov)
            .children(also)
            .child(how)
            .child(found);
        col.child(
            div()
                .id("nc-right-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .child(start.flex_none())
    }

    fn mode_card(
        &self,
        ix: usize,
        c: ModeCard,
        current: WorkMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = c.mode == current;
        let mode = c.mode;
        let radio = div()
            .mt(px(3.))
            .size(px(14.))
            .flex_none()
            .rounded_full()
            .border_1()
            .border_color(if on { ACCENT } else { LINE_STRONG })
            .bg(SURFACE_RAISED)
            .flex()
            .items_center()
            .justify_center()
            .when(on, |d| {
                d.child(div().size(px(6.)).rounded_full().bg(ACCENT))
            });
        div()
            .id(eid("nc-mode", ix))
            .flex()
            .gap(px(10.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .border_1()
            .border_color(if on { ACCENT_RING } else { LINE })
            .when(on, |d| d.bg(ACCENT_GLOW))
            .when(c.disabled, |d| d.opacity(0.45).cursor_not_allowed())
            .when(!c.disabled, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.nc_pick_mode(mode, cx)))
            })
            .child(radio)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT)
                                    .child(c.title),
                            )
                            .when(c.rec, |d| {
                                d.child(Badge::new("Recommended").tone(Tone::Accent))
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(TEXT_MUTED)
                            .child(c.desc),
                    ),
            )
            .into_any_element()
    }

    fn branch_fields(&self, nc: &NewChat, cx: &mut Context<Self>) -> impl IntoElement {
        let label = |t: &'static str| {
            div()
                .mb(px(6.))
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(t)
        };
        let field = || {
            div()
                .flex()
                .items_center()
                .h(px(36.))
                .px(px(10.))
                .rounded(px(8.))
                .border_1()
                .border_color(LINE_STRONG)
                .bg(SURFACE_RAISED)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(TEXT)
        };
        let branches: Vec<String> = nc
            .detail
            .as_ref()
            .map(|d| d.branches.clone())
            .unwrap_or_default();
        let base_value: SharedString = nc.base.clone().unwrap_or_else(|| "…".into()).into();
        let select = field()
            .id("nc-base")
            .justify_between()
            .gap(px(8.))
            .cursor_pointer()
            .hover(|s| s.bg(SURFACE_HOVER))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(nc) = this.newchat.as_mut() {
                    nc.base_open = !nc.base_open;
                    cx.notify();
                }
            }))
            .child(div().flex_1().min_w_0().truncate().child(base_value))
            .child(icon("chevron-down", 14., TEXT_MUTED));
        let menu = (nc.base_open && !branches.is_empty()).then(|| {
            deferred(
                div()
                    .absolute()
                    .top_full()
                    .left_0()
                    .right_0()
                    .pt(px(4.))
                    .child(
                        div()
                            .id("nc-base-menu")
                            .occlude()
                            .flex()
                            .flex_col()
                            .max_h(px(240.))
                            .overflow_y_scroll()
                            .p(px(4.))
                            .border_1()
                            .border_color(LINE_STRONG)
                            .rounded(px(RADIUS_XL))
                            .bg(SURFACE_RAISED)
                            .shadow(popover_shadow())
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                if let Some(nc) = this.newchat.as_mut() {
                                    nc.base_open = false;
                                    cx.notify();
                                }
                            }))
                            .children(branches.iter().enumerate().map(|(i, b)| {
                                let on = nc.base.as_ref() == Some(b);
                                let pick = b.clone();
                                div()
                                    .id(eid("nc-base-opt", i))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .h(px(30.))
                                    .px(px(8.))
                                    .rounded(px(RADIUS_MD))
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(TEXT)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(SURFACE_HOVER))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(nc) = this.newchat.as_mut() {
                                            nc.base = Some(pick.clone());
                                            nc.base_open = false;
                                            cx.notify();
                                        }
                                    }))
                                    .child(
                                        div()
                                            .w(px(14.))
                                            .flex_none()
                                            .when(on, |d| d.child(icon("check", 12., ACCENT_TEXT))),
                                    )
                                    .child(div().min_w_0().truncate().child(b.clone()))
                            })),
                    ),
            )
            .with_priority(2)
        });
        div()
            .mt(px(4.))
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(label("From"))
                    .child(div().relative().child(select).children(menu)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(label("New branch"))
                    .child(field().child(div().flex_1().h(px(18.)).child(self.nc_branch.clone()))),
            )
    }
}

fn found_line(f: Found) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .text_size(px(12.))
        .child(icon(
            if f.ok { "check" } else { "x" },
            12.,
            if f.ok { SUCCESS } else { TEXT_MUTED },
        ))
        .child(
            div()
                .flex_none()
                .font_family(FONT_MONO)
                .text_color(TEXT_SECONDARY)
                .child(f.key),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(TEXT_MUTED)
                .child(f.value),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_matching() {
        assert!(row_matches("", "api", "~/code/api"));
        assert!(row_matches("API", "api", "~/code/api"));
        assert!(row_matches("code/a", "api", "~/code/api"));
        assert!(!row_matches("web", "api", "~/code/api"));
        assert!(looks_like_path("~/code"));
        assert!(looks_like_path("/srv"));
        assert!(looks_like_path("../x"));
        assert!(!looks_like_path("payments"));
    }
}
