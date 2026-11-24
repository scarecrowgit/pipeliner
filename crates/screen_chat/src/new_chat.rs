//! The new-chat page (pick where the chat works) and the list of every chat.
//!
//! The page is `new_chat()` of the mockups: a runtime, a folder (recent folders, the worktrees
//! of the chosen repo, a typed path, or a repo to clone), how to work there, what was found
//! in the folder, and an action bar whose "Start in …" creates the session and opens it. The
//! chat starts with the default agent and model; the chat's own pickers change them. The
//! choice is mirrored in a [`WorkdirChip`] without a session ([`NewChatView::chip`]) for
//! whoever wants to mount it.

mod chats;
mod panel;
mod picker;

#[cfg(test)]
pub(crate) mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window,
    div, prelude::*,
};
use hub::api::{
    ChatMode, FolderInfo, NewSession, RunnableRef, Runtime, WorkMode, WorkdirChoice, WorkdirEntry,
};
use hub::{CatalogModel, OpsModel, Route};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent};

use crate::workdir::common::{
    self, Blocker, FolderFacts, display_path, entry_matches, expand_path, folder_name, looks_like_path,
    merge_places, mode_for_entry,
};
use crate::workdir::{WorkdirChip, WorkdirChipEvent};

pub use chats::ChatsView;

/// How long typing a path rests before the folder is looked at.
const TYPING_REST: Duration = Duration::from_millis(250);

/// Worktrees listed before the search narrows them ("3 of 8").
pub(crate) const WORKTREES_SHOWN: usize = 3;

/// What the folder list shows for what was typed in the search field.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    /// Worktrees of the chosen repo there are in all, before the cap.
    pub worktrees_total: usize,
    /// A typed path, offered as "Use …".
    pub typed: Option<PathBuf>,
    /// Known folders under the typed path.
    pub completions: Vec<WorkdirEntry>,
    pub recent: Vec<WorkdirEntry>,
    pub worktrees: Vec<WorkdirEntry>,
}

impl Listing {
    /// Every row in list order, for the keyboard and the probes.
    pub(crate) fn rows(&self) -> Vec<&WorkdirEntry> {
        self.completions.iter().chain(&self.recent).chain(&self.worktrees).collect()
    }
}

/// The whole content of `Route::NewChat`.
pub struct NewChatView {
    chip: Entity<WorkdirChip>,
    search: Entity<TextInput>,
    branch: Entity<TextInput>,
    clone_url: Entity<TextInput>,
    cloning: bool,
    catalog: Entity<CatalogModel>,
    ops: Entity<OpsModel>,
    recent: Vec<WorkdirEntry>,
    worktrees: Vec<WorkdirEntry>,
    facts: FolderFacts,
    typed: FolderFacts,
    typing: Option<Task<()>>,
    focus_pending: bool,
    branches: Vec<String>,
    base: Option<String>,
    mode: WorkMode,
    branch_edited: bool,
    runtime: Option<String>,
    failed: Option<SharedString>,
    starting: bool,
    shown: bool,
    _subs: Vec<Subscription>,
}

impl NewChatView {
    /// An empty new-chat page; the folder search takes focus.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chip = cx.new(|cx| WorkdirChip::new(None, window, cx).small());
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Search folders, repos and worktrees"));
        let branch = cx.new(|cx| TextInput::new(cx).placeholder("chat/branch-name"));
        let clone_url = cx.new(|cx| TextInput::new(cx).placeholder("owner/name or a clone URL"));
        let catalog = hub::catalog(cx);
        let ops = hub::ops(cx);
        let router = hub::router(cx);
        let subs = vec![
            cx.subscribe_in(&chip, window, |this: &mut Self, _, event: &WorkdirChipEvent, window, cx| {
                this.on_chip(event, window, cx)
            }),
            cx.subscribe_in(&search, window, |this: &mut Self, _, event: &TextInputEvent, window, cx| {
                this.on_search(event, window, cx)
            }),
            cx.subscribe(&branch, |this: &mut Self, _, event: &TextInputEvent, cx| {
                if *event == TextInputEvent::Changed {
                    this.branch_edited = true;
                    this.push_choice(cx);
                }
            }),
            cx.subscribe(&clone_url, |_: &mut Self, _, _: &TextInputEvent, cx| cx.notify()),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
            cx.observe(&ops, |_, _, cx| cx.notify()),
            cx.observe(&router, |this: &mut Self, router, cx| {
                let here = *router.read(cx).current() == Route::NewChat;
                if here && !this.shown {
                    this.load_recent(cx);
                    this.focus_pending = true;
                    cx.notify();
                }
                this.shown = here;
            }),
        ];
        let shown = *router.read(cx).current() == Route::NewChat;
        let mut this = Self {
            chip,
            search,
            branch,
            clone_url,
            cloning: false,
            catalog,
            ops,
            recent: Vec::new(),
            worktrees: Vec::new(),
            facts: FolderFacts::default(),
            typed: FolderFacts::default(),
            typing: None,
            focus_pending: true,
            branches: Vec::new(),
            base: None,
            mode: WorkMode::Worktree,
            branch_edited: false,
            runtime: None,
            failed: None,
            starting: false,
            shown,
            _subs: subs,
        };
        // Focus is taken on the first frame drawn (see `render`): the shell builds the view
        // before it is in the window, and the overlay host takes focus when it is made.
        this.load_recent(cx);
        this
    }

    /// What the shell's header shows as the title.
    pub fn title(&self, _cx: &gpui::App) -> SharedString {
        "New session".into()
    }

    /// The controls on the right of the shell's header, left to right. The page has none.
    pub fn header_actions(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    /// The badges right after the header's title: "no folder yet" until a folder is chosen.
    pub fn header_badges(&self, _cx: &App) -> Vec<AnyElement> {
        if self.facts.path().is_some() {
            return Vec::new();
        }
        vec![Badge::new("no folder yet").dot(Status::Draft).into_any_element()]
    }

    /// A chip without a session that mirrors the folder choice, for whoever mounts it.
    pub fn chip(&self) -> &Entity<WorkdirChip> {
        &self.chip
    }

    // ------------------------------------------------------------ loading

    fn load_recent(&mut self, cx: &mut Context<Self>) {
        let recent = hub::queries(cx).recent_workdirs(cx);
        cx.spawn(async move |this, cx| {
            let recent = recent.await;
            this.update(cx, |this, cx| {
                this.recent = recent;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ the folder

    /// The folder looked at, with its facts once they are in.
    #[cfg(test)]
    pub(crate) fn facts(&self) -> &FolderFacts {
        &self.facts
    }

    /// A folder from the list: its mode follows what it is, then its facts are read.
    pub(crate) fn select_entry(&mut self, entry: &WorkdirEntry, cx: &mut Context<Self>) {
        self.mode = mode_for_entry(entry, self.mode);
        self.select_folder(entry.path.clone(), cx);
    }

    /// Chooses `path` and reads its facts, worktrees and branches off the UI thread. Answers
    /// about a folder chosen before are dropped.
    pub(crate) fn select_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let generation = self.facts.begin(path.clone());
        self.worktrees.clear();
        self.branches.clear();
        self.base = None;
        self.failed = None;
        self.push_choice(cx);
        cx.notify();
        let info = hub::queries(cx).folder_info(&path, cx);
        cx.spawn(async move |this, cx| {
            let info = info.await;
            this.update(cx, |this, cx| this.on_facts(generation, info, cx)).ok();
        })
        .detach();
    }

    /// Forgets the folder: the chat will only talk.
    pub(crate) fn clear_folder(&mut self, cx: &mut Context<Self>) {
        self.facts.clear();
        self.worktrees.clear();
        self.branches.clear();
        self.base = None;
        self.push_choice(cx);
        cx.notify();
    }

    fn on_facts(&mut self, generation: u64, info: FolderInfo, cx: &mut Context<Self>) {
        if !self.facts.finish(generation, info) {
            return;
        }
        self.push_choice(cx);
        cx.notify();
        let Some(facts) = self.facts.repo().cloned() else { return };
        self.base = facts.default_branch.clone().or(facts.branch.clone());
        let queries = hub::queries(cx);
        let worktrees = queries.worktrees(&facts.main_checkout, cx);
        let branches = queries.branches(&facts.main_checkout, cx);
        cx.spawn(async move |this, cx| {
            let (worktrees, branches) = (worktrees.await, branches.await);
            this.update(cx, |this, cx| {
                if this.facts.generation() == generation {
                    this.worktrees = worktrees;
                    this.branches = branches;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        self.suggest_branch(cx);
    }

    /// Names the new branch, unless the name was typed by hand.
    fn suggest_branch(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.facts.repo().map(|facts| facts.main_checkout.clone()) else { return };
        if self.branch_edited {
            return;
        }
        let generation = self.facts.generation();
        let suggestion = hub::queries(cx).suggest_branch(&root, "new chat", cx);
        cx.spawn(async move |this, cx| {
            let name = suggestion.await;
            this.update(cx, |this, cx| {
                if this.facts.generation() == generation && !this.branch_edited {
                    this.branch.update(cx, |input, cx| input.set_text(&name, cx));
                    this.push_choice(cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Picks how to work in the folder.
    pub(crate) fn set_mode(&mut self, mode: WorkMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.push_choice(cx);
        cx.notify();
    }

    /// The mode chosen.
    #[cfg(test)]
    pub(crate) fn mode(&self) -> WorkMode {
        self.mode
    }

    /// Picks the branch a new worktree starts from.
    pub(crate) fn set_base(&mut self, base: String, cx: &mut Context<Self>) {
        self.base = Some(base);
        self.push_choice(cx);
        cx.notify();
    }

    /// The folder and mode as the engine takes them; `None` without a folder.
    pub(crate) fn workdir_choice(&self, cx: &App) -> Option<WorkdirChoice> {
        let path = self.facts.path()?;
        Some(common::choice(path, self.mode, self.base.as_deref(), self.branch.read(cx).text()))
    }

    /// Hands the current choice to the chip, which shows it.
    fn push_choice(&mut self, cx: &mut Context<Self>) {
        let choice = self.workdir_choice(cx);
        let branch = self.facts.branch().map(str::to_string);
        self.chip.update(cx, |chip, cx| {
            chip.set_branch_hint(branch, cx);
            chip.set_choice(choice, cx);
        });
    }

    fn on_chip(&mut self, event: &WorkdirChipEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            WorkdirChipEvent::Changed => match self.chip.read(cx).choice().cloned() {
                None if self.facts.path().is_some() => self.clear_folder(cx),
                None => {}
                Some(choice) => {
                    if choice.mode != self.mode {
                        self.mode = choice.mode;
                        cx.notify();
                    }
                    if self.facts.path() != Some(choice.path.as_path()) {
                        self.select_folder(choice.path, cx);
                    }
                }
            },
            WorkdirChipEvent::BrowseRequested => self.browse(window, cx),
        }
    }

    /// "Open folder…": the search field takes a path.
    pub(crate) fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| {
            if !looks_like_path(input.text()) {
                input.set_text("~/", cx);
            }
            input.focus(window);
        });
        cx.notify();
    }

    /// What the folder list shows now.
    pub(crate) fn listing(&self, cx: &App) -> Listing {
        let text = self.search.read(cx).text().to_string();
        if looks_like_path(&text) {
            let typed = expand_path(&text, cx);
            let prefix = typed.to_string_lossy().to_lowercase();
            let completions = merge_places([self.recent.clone(), self.worktrees.clone()])
                .into_iter()
                .filter(|entry| entry.path.to_string_lossy().to_lowercase().starts_with(&prefix))
                .collect();
            return Listing { typed: Some(typed), completions, ..Listing::default() };
        }
        let recent: Vec<WorkdirEntry> = self.recent.iter().filter(|e| entry_matches(e, &text, cx)).cloned().collect();
        let worktrees: Vec<WorkdirEntry> = self
            .worktrees
            .iter()
            .filter(|entry| !recent.iter().any(|seen| seen.path == entry.path))
            .filter(|entry| entry_matches(entry, &text, cx))
            .cloned()
            .collect();
        let worktrees_total = worktrees.len();
        let cap = if text.trim().is_empty() { WORKTREES_SHOWN } else { usize::MAX };
        let worktrees = worktrees.into_iter().take(cap).collect();
        Listing { worktrees_total, typed: None, completions: Vec::new(), recent, worktrees }
    }

    fn on_search(&mut self, event: &TextInputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Changed => {
                self.look_at_typed(cx);
                cx.notify();
            }
            TextInputEvent::Up => self.move_selection(-1, cx),
            TextInputEvent::Down => self.move_selection(1, cx),
            TextInputEvent::Tab => {
                let listing = self.listing(cx);
                if let Some(first) = listing.completions.first() {
                    let text = format!("{}/", display_path(&first.path, cx));
                    self.search.update(cx, |input, cx| input.set_text(&text, cx));
                    cx.notify();
                }
            }
            // Enter takes a typed path first, else starts in the chosen folder (the ↵ of the
            // start button).
            TextInputEvent::Submit => match self.listing(cx).typed {
                Some(typed) if self.facts.path() != Some(typed.as_path()) => self.select_folder(typed, cx),
                _ => self.start(window, cx),
            },
            _ => {}
        }
    }

    /// A typed path is looked at once typing rests, so its row can say what is there.
    fn look_at_typed(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.listing(cx).typed else {
            self.typing = None;
            self.typed.clear();
            return;
        };
        if self.typed.path() == Some(path.as_path()) {
            return;
        }
        self.typing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TYPING_REST).await;
            let Ok(info) = this.update(cx, |this, cx| {
                let generation = this.typed.begin(path.clone());
                (generation, hub::queries(cx).folder_info(&path, cx))
            }) else {
                return;
            };
            let (generation, info) = (info.0, info.1.await);
            this.update(cx, |this, cx| {
                if this.typed.finish(generation, info) {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// What is known about the typed path: `None` until the folder was looked at.
    pub(crate) fn typed_folder(&self, path: &Path) -> Option<&hub::api::FolderInfo> {
        (self.typed.path() == Some(path)).then(|| self.typed.info()).flatten()
    }

    /// Up and Down in the search field choose the folder above or below.
    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let listing = self.listing(cx);
        let rows = listing.rows();
        if rows.is_empty() {
            return;
        }
        let at = self.facts.path().and_then(|path| rows.iter().position(|entry| entry.path == path));
        let next = match at {
            Some(ix) => (ix as isize + step).clamp(0, rows.len() as isize - 1) as usize,
            None if step > 0 => 0,
            None => rows.len() - 1,
        };
        if Some(next) != at {
            let entry = rows[next].clone();
            self.select_entry(&entry, cx);
        }
    }

    /// Opens or closes the clone row.
    pub(crate) fn toggle_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloning = !self.cloning;
        if self.cloning {
            self.clone_url.read(cx).focus(window);
        }
        cx.notify();
    }

    /// Where a clone lands: beside the chosen repo, else beside the recent folders.
    pub(crate) fn clone_parent(&self, cx: &App) -> PathBuf {
        let near = self
            .facts
            .repo()
            .map(|facts| facts.main_checkout.clone())
            .or_else(|| self.recent.iter().find(|entry| !entry.is_worktree).map(|entry| entry.path.clone()));
        near.and_then(|path| path.parent().map(Path::to_path_buf)).unwrap_or_else(|| expand_path("~", cx))
    }

    /// Clones what was typed beside the other repos; the search then offers the new folder.
    pub(crate) fn clone_repo(&mut self, cx: &mut Context<Self>) {
        let typed = self.clone_url.read(cx).text().trim().to_string();
        if typed.is_empty() {
            return;
        }
        let url = if typed.contains("://") || typed.starts_with("git@") {
            typed.clone()
        } else {
            format!("https://github.com/{}", typed.trim_matches('/'))
        };
        let name = folder_name(Path::new(url.trim_end_matches('/').trim_end_matches(".git")));
        let dest = self.clone_parent(cx).join(name);
        hub::commands(cx).clone_repo(&url, &dest, cx);
        self.cloning = false;
        self.clone_url.update(cx, |input, cx| input.set_text("", cx));
        let shown = display_path(&dest, cx);
        self.search.update(cx, |input, cx| input.set_text(&shown, cx));
        cx.notify();
    }

    // ------------------------------------------------------------ agent, model, runtime

    /// The agent's name as the heading writes it: the default chat agent, "Build".
    pub(crate) fn agent_name(&self, cx: &App) -> String {
        self.catalog
            .read(cx)
            .runnable(&RunnableRef::agent("build"))
            .map_or_else(|| "Build".to_string(), |runnable| runnable.name.clone())
    }

    /// The runtimes as the page draws them.
    pub(crate) fn runtimes(&self, cx: &App) -> Vec<Runtime> {
        self.ops.read(cx).runtimes().value().map(|list| list.as_ref().clone()).unwrap_or_default()
    }

    /// The runtime the chat runs on: the one picked, else the first available.
    pub(crate) fn runtime(&self, cx: &App) -> Option<Runtime> {
        let runtimes = self.runtimes(cx);
        let picked = self.runtime.as_ref().and_then(|name| runtimes.iter().find(|rt| &rt.name == name && rt.available));
        picked.or_else(|| runtimes.iter().find(|rt| rt.available)).cloned()
    }

    /// Picks a runtime; runtimes that are not available cannot be picked.
    pub(crate) fn set_runtime(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.runtimes(cx).iter().any(|rt| rt.name == name && rt.available) {
            self.runtime = Some(name.to_string());
            cx.notify();
        }
    }

    // ------------------------------------------------------------ starting

    /// What keeps the chat from starting, if anything.
    pub(crate) fn blocker(&self, cx: &App) -> Option<Blocker> {
        common::blocker(self.mode, &self.facts, self.branch.read(cx).text())
    }

    /// The session the start button would create; `None` while something blocks it.
    pub(crate) fn new_session(&self, cx: &App) -> Option<NewSession> {
        if self.blocker(cx).is_some() {
            return None;
        }
        Some(NewSession {
            runnable: None,
            mode: ChatMode::Build,
            model: None,
            workdir: self.workdir_choice(cx),
            forced_skills: Vec::new(),
            issue: None,
        })
    }

    /// The start button can be pressed.
    pub(crate) fn can_start(&self, cx: &App) -> bool {
        !self.starting && self.new_session(cx).is_some()
    }

    /// Creates the session and opens it, where its composer takes focus. Blocked choices stay
    /// here with their message; a failure from the engine is said under the summary.
    pub(crate) fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        let Some(new) = self.new_session(cx) else {
            cx.notify();
            return;
        };
        self.starting = true;
        self.failed = None;
        cx.notify();
        let commands = hub::commands(cx);
        let created = commands.new_session(new, cx);
        cx.spawn_in(window, async move |this, cx| {
            let Some(session) = created.await else {
                this.update(cx, |this, cx| {
                    this.starting = false;
                    this.failed = Some("The chat did not start; the notice says why.".into());
                    cx.notify();
                })
                .ok();
                return;
            };
            cx.update(|_, cx| hub::navigate(Route::Chat { session: session.id.clone() }, cx)).ok();
            this.update(cx, |this, cx| this.started(cx)).ok();
        })
        .detach();
    }

    /// The chat is open: the page starts over with the same folder.
    fn started(&mut self, cx: &mut Context<Self>) {
        self.starting = false;
        self.branch_edited = false;
        self.suggest_branch(cx);
        cx.notify();
    }
}

impl Render for NewChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            let search = self.search.clone();
            window.defer(cx, move |window, cx| search.read(cx).focus(window));
        }
        div()
            .flex()
            .flex_row()
            .size_full()
            .min_w_0()
            .bg(BG)
            .child(self.render_main(window, cx))
            .child(self.render_panel(window, cx))
    }
}
