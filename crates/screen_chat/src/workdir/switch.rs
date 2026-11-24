//! The switch dialog: move a running session to another folder or work mode. The next message
//! runs there; uncommitted changes stay where they are.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, Render, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use hub::api::{FolderInfo, WorkMode, Workdir, WorkdirEntry};
use hub::model::SessionId;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent, text_field};

use super::common::{
    self, Blocker, FolderFacts, FolderRow, SWITCH_NOTE, blocker, blocker_line, display_path, expand_path, folder_name,
    looks_like_path, mode_cards, mode_for_entry, workdir_label,
};

/// The dialog's id.
pub(crate) const DIALOG: &str = "switch-workdir";
/// The confirm button's id and probe.
pub(crate) const CONFIRM: &str = "switch-workdir-confirm";
/// How long typing in the path field rests before the folder is looked at.
const TYPING_REST: Duration = Duration::from_millis(250);
/// Folders listed under the path field.
const LISTED: usize = 5;

/// What the dialog opens with.
pub(crate) struct Start {
    pub session: SessionId,
    pub title: String,
    pub current: Option<Workdir>,
    pub places: Vec<WorkdirEntry>,
    /// The folder picked in the popover, if one was.
    pub target: Option<WorkdirEntry>,
}

/// Opens the dialog over the window.
pub(crate) fn open(start: Start, window: &mut Window, cx: &mut App) {
    let title = start.title.clone();
    let view = cx.new(|cx| SwitchWorkdir::new(start, window, cx));
    overlays(window, cx).open_dialog(
        DIALOG,
        move |_, cx| {
            let ready = view.read(cx).can_confirm(cx);
            let (click, key) = (view.clone(), view.clone());
            Dialog::new(DIALOG, "Switch workdir")
                .subtitle(SharedString::from(title.clone()))
                .width(600.)
                .child(view.clone())
                .cancel_button()
                .action(
                    Button::new(CONFIRM, "Switch workdir")
                        .primary()
                        .kbd("⌘↵")
                        .disabled(!ready)
                        .on_click(move |_, window, cx| {
                            click.update(cx, |dialog, cx| dialog.confirm(window, cx));
                        })
                        .probe(CONFIRM),
                )
                .on_confirm(move |window, cx| {
                    key.update(cx, |dialog, cx| dialog.confirm(window, cx));
                })
                .into_any_element()
        },
        window,
        cx,
    );
}

/// The body of the switch dialog and its state.
pub(crate) struct SwitchWorkdir {
    session: SessionId,
    title: String,
    current: Option<Workdir>,
    places: Vec<WorkdirEntry>,
    path: Entity<TextInput>,
    mode: WorkMode,
    facts: FolderFacts,
    base: Option<String>,
    branch: Entity<TextInput>,
    branch_edited: bool,
    branches: Vec<String>,
    typing: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl SwitchWorkdir {
    fn new(start: Start, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| TextInput::new(cx).placeholder("~/code/repo or a folder path"));
        let branch = cx.new(|cx| TextInput::new(cx).placeholder("chat/branch-name"));
        let subs = vec![
            cx.subscribe(&path, |this: &mut Self, _, event: &TextInputEvent, cx| this.on_path(event, cx)),
            cx.subscribe(&branch, |this: &mut Self, _, event: &TextInputEvent, cx| {
                if *event == TextInputEvent::Changed {
                    this.branch_edited = true;
                    cx.notify();
                }
            }),
        ];
        let mode = start.current.as_ref().map_or(WorkMode::Worktree, |workdir| workdir.mode);
        let mut this = Self {
            session: start.session,
            title: start.title,
            current: start.current,
            places: start.places,
            path,
            mode,
            facts: FolderFacts::default(),
            base: None,
            branch,
            branch_edited: false,
            branches: Vec::new(),
            typing: None,
            _subs: subs,
        };
        match start.target {
            Some(entry) => this.pick(&entry, cx),
            None => {
                let input = this.path.clone();
                window.defer(cx, move |window, cx| input.read(cx).focus(window));
            }
        }
        this
    }

    /// A folder from the list: its path goes in the field and its facts are read.
    pub(crate) fn pick(&mut self, entry: &WorkdirEntry, cx: &mut Context<Self>) {
        let shown = display_path(&entry.path, cx);
        self.path.update(cx, |input, cx| input.set_text(&shown, cx));
        self.mode = mode_for_entry(entry, self.mode);
        self.select(entry.path.clone(), cx);
    }

    /// Picks a mode.
    pub(crate) fn set_mode(&mut self, mode: WorkMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    fn on_path(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        match event {
            TextInputEvent::Changed => {
                cx.notify();
                self.typing = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(TYPING_REST).await;
                    this.update(cx, |this, cx| this.take_typed(cx)).ok();
                }));
            }
            TextInputEvent::Submit => {
                self.typing = None;
                self.take_typed(cx);
            }
            _ => {}
        }
    }

    /// Looks at the folder typed in the path field, if it reads as one.
    fn take_typed(&mut self, cx: &mut Context<Self>) {
        let text = self.path.read(cx).text().to_string();
        if !looks_like_path(&text) {
            return;
        }
        let path = expand_path(&text, cx);
        if self.facts.path() != Some(path.as_path()) {
            self.select(path, cx);
        }
    }

    fn select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let generation = self.facts.begin(path.clone());
        self.branches.clear();
        self.base = None;
        cx.notify();
        let info = hub::queries(cx).folder_info(&path, cx);
        cx.spawn(async move |this, cx| {
            let info = info.await;
            this.update(cx, |this, cx| this.on_facts(generation, info, cx)).ok();
        })
        .detach();
    }

    fn on_facts(&mut self, generation: u64, info: FolderInfo, cx: &mut Context<Self>) {
        if !self.facts.finish(generation, info) {
            return;
        }
        cx.notify();
        let Some(facts) = self.facts.repo().cloned() else { return };
        self.base = facts.default_branch.clone().or(facts.branch.clone());
        let queries = hub::queries(cx);
        let branches = queries.branches(&facts.main_checkout, cx);
        let suggestion = (!self.branch_edited).then(|| queries.suggest_branch(&facts.main_checkout, &self.title, cx));
        cx.spawn(async move |this, cx| {
            let branches = branches.await;
            let suggestion = match suggestion {
                Some(task) => Some(task.await),
                None => None,
            };
            this.update(cx, |this, cx| {
                if this.facts.generation() != generation {
                    return;
                }
                this.branches = branches;
                if let Some(name) = suggestion.filter(|_| !this.branch_edited) {
                    this.branch.update(cx, |input, cx| input.set_text(&name, cx));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn blocker(&self, cx: &App) -> Option<Blocker> {
        blocker(self.mode, &self.facts, self.branch.read(cx).text())
    }

    /// A folder is chosen and nothing blocks it.
    pub(crate) fn can_confirm(&self, cx: &App) -> bool {
        self.facts.path().is_some() && self.blocker(cx).is_none()
    }

    /// Moves the session and closes the dialog; does nothing while something blocks it.
    pub(crate) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_confirm(cx) {
            return;
        }
        let Some(path) = self.facts.path() else { return };
        let choice = common::choice(path, self.mode, self.base.as_deref(), self.branch.read(cx).text());
        let label = workdir_label(&folder_name(path), choice.branch.as_deref().or(self.facts.branch()));
        let moved = hub::commands(cx).set_session_workdir(&self.session, choice, cx);
        overlays(window, cx).close(DIALOG, window, cx);
        cx.spawn_in(window, async move |_, cx| {
            if moved.await.is_some() {
                cx.update(|window, cx| overlays(window, cx).toast(Toast::new("Workdir switched").detail(label), cx))
                    .ok();
            }
        })
        .detach();
    }

    fn open_branches(&self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let pick = Rc::new(move |branch: String, _: &mut Window, cx: &mut App| {
            this.update(cx, |this, cx| {
                this.base = Some(branch);
                cx.notify();
            })
            .ok();
        });
        common::open_branch_menu("switch-workdir-base", self.branches.clone(), self.base.clone(), pick, window, cx);
    }

    fn render_places(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let text = self.path.read(cx).text().to_string();
        let query = if looks_like_path(&text) || self.facts.path().is_some() { String::new() } else { text };
        let chosen = self.facts.path().map(PathBuf::from);
        let current = self.current.as_ref().map(|workdir| workdir.path.clone());
        let rows: Vec<WorkdirEntry> = self
            .places
            .iter()
            .filter(|entry| Some(&entry.path) != current.as_ref())
            .filter(|entry| common::entry_matches(entry, &query, cx))
            .take(LISTED)
            .cloned()
            .collect();
        let mut list = col().flex_none().gap(px(1.));
        for (ix, entry) in rows.into_iter().enumerate() {
            let selected = chosen.as_ref() == Some(&entry.path);
            let picked = entry.clone();
            let row = FolderRow { name: format!("switch-workdir-folder-{ix}").into(), entry, selected, compact: true };
            let on_click = cx.listener(move |this, _, _, cx| this.pick(&picked, cx));
            list = list.child(row.render(on_click, window, cx));
        }
        list.into_any_element()
    }
}

impl Render for SwitchWorkdir {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.current.as_ref().map_or_else(
            || "No folder".to_string(),
            |workdir| workdir_label(&workdir.repo, workdir.branch.as_deref()),
        );
        let now_path = self.current.as_ref().map(|workdir| display_path(&workdir.path, cx));
        let this = cx.weak_entity();
        let on_mode: common::ModeHandler = Rc::new(move |mode, _, cx| {
            this.update(cx, |this, cx| this.set_mode(mode, cx)).ok();
        });
        let blocked = self.blocker(cx).and_then(Blocker::message);
        let worktree = self.mode == WorkMode::Worktree && self.facts.repo().is_some();
        let label = |text: &'static str| div().type_style(LABEL).text_color(TEXT_MUTED).child(text);

        col()
            .gap(px(DIALOG_GAP))
            .child(
                col()
                    .gap(px(SPACE_1))
                    .child(label("Working in now"))
                    .child(div().type_style(BODY_STRONG).font_family(FONT_MONO).child(now))
                    .when_some(now_path, |d, path| {
                        d.child(div().type_style(MONO_SM).text_color(TEXT_MUTED).child(path))
                    }),
            )
            .child(
                col()
                    .gap(px(SPACE_1_5))
                    .child(label("Move to"))
                    .child(text_field("switch-workdir-path", &self.path).icon(IconName::Folder).small())
                    .child(self.render_places(window, cx)),
            )
            .child(
                col()
                    .gap(px(SPACE_1_5))
                    .child(label("How to work there"))
                    .child(mode_cards("switch-workdir", self.mode, self.facts.branch(), on_mode))
                    .when(worktree, |d| {
                        let (base, open) = (self.base.clone(), cx.listener(|this, _, window, cx| this.open_branches(window, cx)));
                        d.child(
                            row()
                                .mt(px(SPACE_1_5))
                                .gap(px(SPACE_2))
                                .child(
                                    col()
                                        .w(px(160.))
                                        .flex_none()
                                        .gap(px(SPACE_1_5))
                                        .child(label("From"))
                                        .child(common::branch_select("switch-workdir-base", base.as_deref(), open)),
                                )
                                .child(
                                    col()
                                        .flex_1()
                                        .gap(px(SPACE_1_5))
                                        .child(label("New branch"))
                                        .child(text_field("switch-workdir-branch", &self.branch).small()),
                                ),
                        )
                    })
                    .when_some(blocked, |d, message| d.child(blocker_line(message))),
            )
            .child(
                Banner::new()
                    .tone(BannerTone::Warning)
                    .child(icon(IconName::Alert, 16., WARNING))
                    .child(div().flex_1().min_w_0().child(SWITCH_NOTE)),
            )
            .probe("switch-workdir-body")
    }
}
