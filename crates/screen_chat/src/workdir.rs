//! Where a chat works: the folder chip, the popover it opens, and the switch dialog.
//!
//! The chip has two lives:
//!
//! - **For a session** (`WorkdirChip::new(Some(id), ..)`): it names the session's folder and
//!   branch; its popover shows where the chat works, lists other folders and worktrees, and
//!   opens the [switch dialog](switch) that moves the session there.
//! - **For the chat about to start** (`WorkdirChip::new(None, ..)`): it holds the folder and
//!   work mode the new-chat page has chosen. The page reads it with [`WorkdirChip::choice`],
//!   changes it with [`WorkdirChip::set_choice`], and hears about changes made in the chip's
//!   own popover through [`WorkdirChipEvent`].

pub(crate) mod common;
mod popover;
pub(crate) mod switch;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use gpui::{
    App, ClipboardItem, Context, ElementId, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use hub::SessionModel;
use hub::api::{WorkMode, WorkdirChoice, WorkdirEntry};
use hub::model::SessionId;
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::{TextInput, TextInputEvent};

use common::{display_path, folder_name, merge_places, mode_for_entry, workdir_label};

/// The chip's element id, probe name and popover anchor.
pub const CHIP: &str = "workdir-chip";

/// What a chip without a session tells the page that owns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkdirChipEvent {
    /// The choice changed (a folder picked in the popover, or [`WorkdirChip::set_choice`]).
    Changed,
    /// "All folders, runtimes and repos…" was chosen: the page should offer its folder field.
    BrowseRequested,
}

/// The chip that names a session's folder and branch and opens the workdir popover. With no
/// session it belongs to the new-chat page and edits the choice for the chat about to start.
pub struct WorkdirChip {
    session: Option<SessionId>,
    model: Option<Entity<SessionModel>>,
    draft: Option<WorkdirChoice>,
    branch_hint: Option<String>,
    small: bool,
    places: Vec<WorkdirEntry>,
    places_generation: u64,
    search: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<WorkdirChipEvent> for WorkdirChip {}

impl WorkdirChip {
    /// A chip for `session`, or for the chat that does not exist yet.
    pub fn new(session: Option<SessionId>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx).placeholder("Find a folder, repo or worktree"));
        let mut subs = vec![cx.subscribe(&search, |_: &mut Self, _, event: &TextInputEvent, cx| {
            if *event == TextInputEvent::Changed {
                cx.notify();
            }
        })];
        let model = session.as_ref().map(|id| hub::session(id, cx));
        if let Some(model) = &model {
            subs.push(cx.observe(model, |_, _, cx| cx.notify()));
        }
        Self {
            session,
            model,
            draft: None,
            branch_hint: None,
            small: false,
            places: Vec::new(),
            places_generation: 0,
            search,
            _subs: subs,
        }
    }

    /// Builder: the 28px trigger of the composer's control row.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The session it belongs to, if any.
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// Without a session: the folder and mode chosen for the chat about to start, if any.
    /// With one: always `None` (the session's folder is in its model).
    pub fn choice(&self) -> Option<&WorkdirChoice> {
        self.draft.as_ref()
    }

    /// Without a session: replaces the choice and emits [`WorkdirChipEvent::Changed`] when it
    /// differs. Ignored on a session's chip.
    pub fn set_choice(&mut self, choice: Option<WorkdirChoice>, cx: &mut Context<Self>) {
        if self.session.is_some() || self.draft == choice {
            return;
        }
        self.draft = choice;
        cx.emit(WorkdirChipEvent::Changed);
        cx.notify();
    }

    /// Without a session: the branch the chosen folder has checked out, shown after its name
    /// when the choice is not a new worktree. Emits nothing.
    pub fn set_branch_hint(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        if self.branch_hint != branch {
            self.branch_hint = branch;
            cx.notify();
        }
    }

    /// What the chip reads: `payments-api · fix/PAY-1423`, or "No folder".
    pub fn label(&self, cx: &App) -> String {
        match &self.model {
            Some(model) => {
                let model = model.read(cx);
                match model.session() {
                    Some(session) => session.workdir.as_ref().map_or_else(
                        || "No folder".to_string(),
                        |workdir| workdir_label(&workdir.repo, workdir.branch.as_deref()),
                    ),
                    None => "Loading…".to_string(),
                }
            }
            None => match &self.draft {
                Some(choice) => {
                    let branch = match choice.mode {
                        WorkMode::Worktree => choice.branch.as_deref(),
                        _ => self.branch_hint.as_deref(),
                    };
                    workdir_label(&folder_name(&choice.path), branch)
                }
                None => "No folder".to_string(),
            },
        }
    }

    /// The popover is open.
    pub fn is_open(&self, window: &Window, cx: &App) -> bool {
        overlays(window, cx).is_open(CHIP, cx)
    }

    /// Opens the workdir popover under the chip. Other views call it through the entity
    /// (`chip.update(cx, |chip, cx| chip.open(window, cx))`), e.g. a "Change" link.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.load_places(cx);
        let chip = cx.entity();
        let anchor = Anchor::of(CHIP, window, cx);
        overlays(window, cx).open_popover(
            anchor,
            Placement::below(),
            move |window, cx| popover::build(&chip, window, cx),
            window,
            cx,
        );
        let search = self.search.clone();
        window.defer(cx, move |window, cx| search.read(cx).focus(window));
    }

    /// Folders the popover lists besides the current one, newest answer only.
    pub(crate) fn places(&self) -> &[WorkdirEntry] {
        &self.places
    }

    pub(crate) fn search(&self) -> &Entity<TextInput> {
        &self.search
    }

    pub(crate) fn model(&self) -> Option<&Entity<SessionModel>> {
        self.model.as_ref()
    }

    /// The checkout the session's (or the choice's) folder belongs to, for its worktrees.
    fn repo_root(&self, cx: &App) -> Option<PathBuf> {
        match &self.model {
            Some(model) => {
                let workdir = model.read(cx).session()?.workdir.as_ref()?;
                Some(workdir.source.clone().unwrap_or_else(|| workdir.path.clone()))
            }
            None => self.draft.as_ref().map(|choice| choice.path.clone()),
        }
    }

    /// Reads recent folders and the worktrees of the current repo off the UI thread.
    fn load_places(&mut self, cx: &mut Context<Self>) {
        self.places_generation += 1;
        let generation = self.places_generation;
        let queries = hub::queries(cx);
        let recent = queries.recent_workdirs(cx);
        let worktrees = self.repo_root(cx).map(|root| queries.worktrees(&root, cx));
        cx.spawn(async move |this, cx| {
            let worktrees = match worktrees {
                Some(task) => task.await,
                None => Vec::new(),
            };
            let recent = recent.await;
            this.update(cx, |this, cx| {
                if this.places_generation == generation {
                    this.places = merge_places([worktrees, recent]);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// A folder was picked in the popover: a new choice, or the switch dialog for a session.
    pub(crate) fn pick(&mut self, entry: WorkdirEntry, window: &mut Window, cx: &mut Context<Self>) {
        overlays(window, cx).close(CHIP, window, cx);
        match &self.session {
            Some(id) => self.open_switch(id.clone(), Some(entry), window, cx),
            None => {
                let mode = mode_for_entry(&entry, self.draft.as_ref().map_or(WorkMode::Worktree, |c| c.mode));
                let choice = common::choice(&entry.path, mode, None, "");
                self.branch_hint = entry.branch.clone();
                self.set_choice(Some(choice), cx);
            }
        }
    }

    /// "All folders, runtimes and repos…": the switch dialog, or the page's folder field.
    pub(crate) fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        overlays(window, cx).close(CHIP, window, cx);
        match &self.session {
            Some(id) => self.open_switch(id.clone(), None, window, cx),
            None => cx.emit(WorkdirChipEvent::BrowseRequested),
        }
    }

    /// "No folder": the chat about to start only talks.
    pub(crate) fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        overlays(window, cx).close(CHIP, window, cx);
        self.set_choice(None, cx);
    }

    /// Copies the folder's path and says so.
    pub(crate) fn copy_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        overlays(window, cx).close(CHIP, window, cx);
        let path = match &self.model {
            Some(model) => model.read(cx).session().and_then(|s| s.workdir.as_ref()).map(|w| w.path.clone()),
            None => self.draft.as_ref().map(|choice| choice.path.clone()),
        };
        let Some(path) = path else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        overlays(window, cx).toast(Toast::new("Path copied").detail(display_path(&path, cx)), cx);
    }

    fn open_switch(&mut self, id: SessionId, target: Option<WorkdirEntry>, window: &mut Window, cx: &mut Context<Self>) {
        let (title, current) = self
            .model
            .as_ref()
            .and_then(|model| model.read(cx).session())
            .map(|session| (session.title.clone(), session.workdir.clone()))
            .unwrap_or_default();
        let places = self.places.clone();
        switch::open(switch::Start { session: id, title, current, places, target }, window, cx);
    }
}

impl Render for WorkdirChip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.is_open(window, cx);
        let empty = match &self.model {
            Some(model) => model.read(cx).session().is_some_and(|s| s.workdir.is_none()),
            None => self.draft.is_none(),
        };
        let border = if open { ACCENT } else { LINE_STRONG };
        let faded = motion::hover_fade_state(
            "workdir-chip-fade",
            Look::bg(SURFACE_RAISED).border(border),
            Look::bg(SURFACE_HOVER).border(border),
            window,
            cx,
        );
        let (h, radius, text, icon_size) = if self.small {
            (CONTROL_HEIGHT_SM, RADIUS_MD, LABEL, 12.)
        } else {
            (CONTROL_HEIGHT_MD, RADIUS_LG, BODY, 14.)
        };
        let el = row()
            .id(ElementId::Name(CHIP.into()))
            .debug_selector(|| CHIP.into())
            .flex_none()
            .max_w(px(300.))
            .gap(px(SPACE_2))
            .h(px(h))
            .pl(px(SPACE_2_5))
            .pr(px(SPACE_2))
            .border_1()
            .rounded(px(radius))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| this.open(window, cx)));
        let label: SharedString = self.label(cx).into();
        faded
            .apply(el)
            .child(icon(IconName::Folder, icon_size, TEXT_MUTED))
            .child(
                div()
                    .flex_shrink()
                    .min_w_0()
                    .type_style(text)
                    .font_family(if empty { FONT_SANS } else { FONT_MONO })
                    .text_color(if empty { TEXT_MUTED } else { TEXT })
                    .child(truncate_lh(label.clone(), text.line)),
            )
            .child(icon(IconName::ChevronDown, icon_size, TEXT_MUTED))
            .probe_value(CHIP, label)
            .popover_anchor(CHIP)
    }
}
