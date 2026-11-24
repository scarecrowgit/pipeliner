//! What the chip, the switch dialog and the new-chat page share: how a workdir is named, the
//! three work modes and their cards, the facts of the folder being looked at, and what keeps a
//! choice from being used.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{FolderInfo, RepoFacts, RepoHost, WorkMode, WorkdirChoice, WorkdirEntry};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

/// The work modes in the order the mockups list them.
pub(crate) const MODES: [WorkMode; 3] = [WorkMode::Worktree, WorkMode::InPlace, WorkMode::ReadOnly];

/// The note under the workdir popover and in the switch dialog.
pub(crate) const SWITCH_NOTE: &str =
    "The next message runs in the folder you pick. Uncommitted changes stay where they are.";

/// The part of a probe name that names a mode: `new-chat-mode-worktree`.
pub(crate) fn mode_slug(mode: WorkMode) -> &'static str {
    match mode {
        WorkMode::Worktree => "worktree",
        WorkMode::InPlace => "in-place",
        WorkMode::ReadOnly => "read-only",
    }
}

/// A mode card's title: "New worktree", "In place on main", "Read-only".
pub(crate) fn mode_title(mode: WorkMode, branch: Option<&str>) -> String {
    match (mode, branch) {
        (WorkMode::Worktree, _) => "New worktree".to_string(),
        (WorkMode::InPlace, Some(branch)) => format!("In place on {branch}"),
        (WorkMode::InPlace, None) => "In place".to_string(),
        (WorkMode::ReadOnly, _) => "Read-only".to_string(),
    }
}

/// The muted line of a mode card.
pub(crate) fn mode_description(mode: WorkMode) -> &'static str {
    match mode {
        WorkMode::Worktree => "Own branch and folder. Other agents in this repo are not affected.",
        WorkMode::InPlace => "Edits land straight in your checkout.",
        WorkMode::ReadOnly => "Nothing is written. Good for Plan and for questions about the code.",
    }
}

/// A mode in a sentence: "new worktree", "in place", "read-only".
pub(crate) fn mode_words(mode: WorkMode) -> &'static str {
    match mode {
        WorkMode::Worktree => "new worktree",
        WorkMode::InPlace => "in place",
        WorkMode::ReadOnly => "read-only",
    }
}

/// The host badge: "GitHub", "GitLab", "Local".
pub(crate) fn host_label(host: RepoHost) -> &'static str {
    match host {
        RepoHost::GitHub => "GitHub",
        RepoHost::GitLab => "GitLab",
        RepoHost::Local => "Local",
    }
}

/// `repo · branch`, or the repo alone.
pub(crate) fn workdir_label(repo: &str, branch: Option<&str>) -> String {
    match branch {
        Some(branch) if !branch.is_empty() => format!("{repo} · {branch}"),
        _ => repo.to_string(),
    }
}

/// The last part of a path: the folder's name.
pub(crate) fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// A path as the user reads it: `~/code/acme` under the home directory.
pub(crate) fn display_path(path: &Path, cx: &App) -> String {
    hub::display_path(path, cx)
}

/// One line of text in its parent's font and size, cut at the end with `…`. Machine text
/// (names, branches) goes through this rather than `truncate`, whose cut line comes out in
/// the sans face.
pub(crate) fn line_text(text: impl Into<SharedString>, line_height: f32) -> gpui::Div {
    ui::truncate::truncate_styled(text, Vec::<(std::ops::Range<usize>, gpui::HighlightStyle)>::new(), line_height)
}

/// A path on one line in its parent's font: the head and the last parts stay, the middle
/// gives way (`~/…/payments-api/fix-PAY-1423`). Wrapped in a row so it fills a column.
pub(crate) fn path_line(text: impl Into<SharedString>, line_height: f32) -> gpui::Div {
    row().min_w_0().child(ui::truncate::truncate_middle(text, line_height))
}

/// A path as the user typed it: `~` resolves to the home directory.
pub(crate) fn expand_path(text: &str, cx: &App) -> PathBuf {
    hub::hub(cx).read(cx).paths().expand(text.trim())
}

/// What was typed reads as a path rather than a search: it starts with `/` or `~`.
pub(crate) fn looks_like_path(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with('/') || text.starts_with('~')
}

/// The host and owner of a remote, for the folder head: `github.com/acme`.
pub(crate) fn remote_owner(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest).replacen(':', "/", 1);
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(host), Some(owner)) => format!("{host}/{owner}"),
        (Some(host), None) => host.to_string(),
        _ => url.to_string(),
    }
}

/// "2 uncommitted files · 1 ahead of main", "clean on main".
pub(crate) fn git_state(facts: &RepoFacts) -> String {
    let branch = facts.branch.as_deref().unwrap_or("a detached head");
    let mut parts = Vec::new();
    if facts.dirty > 0 {
        parts.push(base::fmt::plural(u64::from(facts.dirty), "uncommitted file", "uncommitted files"));
    }
    if facts.ahead > 0 {
        let against = facts.compared_to.as_deref().unwrap_or("its base");
        parts.push(format!("{} ahead of {against}", facts.ahead));
    }
    if facts.behind > 0 {
        let against = facts.compared_to.as_deref().unwrap_or("its base");
        parts.push(format!("{} behind {against}", facts.behind));
    }
    if parts.is_empty() { format!("clean on {branch}") } else { parts.join(" · ") }
}

/// The facts of the folder being looked at. Only the answer to the newest request is kept:
/// an answer for a folder that is no longer chosen is dropped.
#[derive(Debug, Default)]
pub(crate) struct FolderFacts {
    path: Option<PathBuf>,
    info: Option<FolderInfo>,
    loading: bool,
    generation: u64,
}

impl FolderFacts {
    /// Starts looking at `path`; returns the generation its answer must carry.
    pub(crate) fn begin(&mut self, path: PathBuf) -> u64 {
        self.generation += 1;
        self.path = Some(path);
        self.info = None;
        self.loading = true;
        self.generation
    }

    /// Takes an answer; `false` (and nothing changes) when a newer request was made since.
    pub(crate) fn finish(&mut self, generation: u64, info: FolderInfo) -> bool {
        if generation != self.generation {
            return false;
        }
        self.info = Some(info);
        self.loading = false;
        true
    }

    /// Forgets the folder.
    pub(crate) fn clear(&mut self) {
        self.generation += 1;
        *self = Self { generation: self.generation, ..Self::default() };
    }

    /// The generation of the newest request.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The folder looked at.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// What was found there, once the answer is in.
    pub(crate) fn info(&self) -> Option<&FolderInfo> {
        self.info.as_ref()
    }

    /// Git facts, when the answer is in and the folder is in a repository.
    pub(crate) fn repo(&self) -> Option<&RepoFacts> {
        self.info.as_ref().and_then(|info| info.facts.as_ref())
    }

    /// The answer is not in yet.
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    /// The folder's checked-out branch, when known.
    pub(crate) fn branch(&self) -> Option<&str> {
        self.repo().and_then(|facts| facts.branch.as_deref())
    }
}

/// Why a folder and mode cannot be used yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blocker {
    /// The folder's facts are still being read.
    Loading,
    /// Worktree mode on a folder outside any repository.
    NotARepo,
    /// Worktree mode with an empty branch name.
    NoBranch,
}

impl Blocker {
    /// What the page says, under the work modes; `None` for a wait that needs no words.
    pub(crate) fn message(self) -> Option<&'static str> {
        match self {
            Blocker::Loading => None,
            Blocker::NotARepo => {
                Some("Not a git repository. A new worktree needs one: work in place or read-only instead.")
            }
            Blocker::NoBranch => Some("Name the new branch."),
        }
    }
}

/// What keeps `mode` in the folder of `facts` from being used, if anything.
pub(crate) fn blocker(mode: WorkMode, facts: &FolderFacts, branch: &str) -> Option<Blocker> {
    facts.path()?;
    if mode != WorkMode::Worktree {
        return None;
    }
    if facts.is_loading() {
        return Some(Blocker::Loading);
    }
    if facts.repo().is_none() {
        return Some(Blocker::NotARepo);
    }
    if branch.trim().is_empty() {
        return Some(Blocker::NoBranch);
    }
    None
}

/// The engine's choice for a folder: base and branch only in worktree mode.
pub(crate) fn choice(path: &Path, mode: WorkMode, base: Option<&str>, branch: &str) -> WorkdirChoice {
    let worktree = mode == WorkMode::Worktree;
    let branch = branch.trim();
    WorkdirChoice {
        path: path.to_path_buf(),
        mode,
        base: base.filter(|_| worktree).map(str::to_string),
        branch: (worktree && !branch.is_empty()).then(|| branch.to_string()),
    }
}

/// The mode a freshly picked folder starts in: an existing worktree is worked in as it is,
/// anything else keeps the mode chosen before.
pub(crate) fn mode_for_entry(entry: &WorkdirEntry, current: WorkMode) -> WorkMode {
    if entry.is_worktree && current == WorkMode::Worktree { WorkMode::InPlace } else { current }
}

/// Recent folders and worktrees as one list, each path once, `first` ahead of the rest.
pub(crate) fn merge_places(lists: impl IntoIterator<Item = Vec<WorkdirEntry>>) -> Vec<WorkdirEntry> {
    let mut out: Vec<WorkdirEntry> = Vec::new();
    for list in lists {
        for entry in list {
            if !out.iter().any(|seen| seen.path == entry.path) {
                out.push(entry);
            }
        }
    }
    out
}

/// The entry matches what was typed in a folder search.
pub(crate) fn entry_matches(entry: &WorkdirEntry, query: &str, cx: &App) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {}",
        entry.repo,
        entry.branch.as_deref().unwrap_or_default(),
        display_path(&entry.path, cx)
    )
    .to_lowercase();
    query.split_whitespace().all(|word| hay.contains(word))
}

/// Called with the mode a card picks.
pub(crate) type ModeHandler = Rc<dyn Fn(WorkMode, &mut Window, &mut App)>;

/// The three work-mode cards (`.opt`), probed `<prefix>-mode-<slug>`; "Recommended" follows
/// the worktree card's title, as drawn.
pub(crate) fn mode_cards(prefix: &str, selected: WorkMode, branch: Option<&str>, on_select: ModeHandler) -> AnyElement {
    col()
        .flex_none()
        .gap(px(SPACE_1_5))
        .children(MODES.into_iter().map(|mode| {
            let name: SharedString = format!("{prefix}-mode-{}", mode_slug(mode)).into();
            let on_select = on_select.clone();
            ModeCard { name, title: mode_title(mode, branch), mode, selected: mode == selected, on_select }
        }))
        .into_any_element()
}

/// One radio card of [`mode_cards`].
#[derive(IntoElement)]
struct ModeCard {
    name: SharedString,
    title: String,
    mode: WorkMode,
    selected: bool,
    on_select: ModeHandler,
}

impl RenderOnce for ModeCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (rest, hover) = if self.selected {
            let look = Look::bg(SURFACE_RAISED).border(ACCENT_RING);
            (look, look)
        } else {
            (Look::bg(SURFACE).border(LINE), Look::bg(SURFACE_RAISED).border(LINE))
        };
        let faded = motion::hover_fade_state(self.name.clone(), rest, hover, window, cx);
        let selector = self.name.to_string();
        let (mode, on_select) = (self.mode, self.on_select);
        let el = div()
            .id(ElementId::Name(self.name.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_start()
            .gap(px(SPACE_2_5))
            .w_full()
            .py(px(SPACE_2_5))
            .px(px(SPACE_3))
            .border_1()
            .rounded(px(RADIUS_LG))
            .cursor_pointer()
            .when(self.selected, |d| d.shadow(selected_glow()))
            .when(!self.selected, |d| d.on_click(move |_, window, cx| on_select(mode, window, cx)));
        faded
            .apply(el)
            .child(div().mt(px(SPACE_0_5)).child(radio(self.selected)))
            .child(
                col()
                    .flex_1()
                    .child(
                        row()
                            .gap(px(SPACE_1_5))
                            .child(div().flex_none().type_style(BODY_STRONG).child(self.title))
                            .when(mode == WorkMode::Worktree, |d| {
                                d.child(Badge::new("Recommended").tone(BadgeTone::Accent))
                            }),
                    )
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(mode_description(mode))),
            )
            .probe(self.name)
    }
}

/// A 16px radio circle (`.radio`, `.radio.on`).
pub(crate) fn radio(on: bool) -> gpui::Div {
    div()
        .flex_none()
        .size(px(16.))
        .rounded_full()
        .when(on, |d| d.border(px(5.)).border_color(ACCENT_STRONG).bg(ON_ACCENT))
        .when(!on, |d| d.border_1().border_color(LINE_STRONG).bg(SURFACE_RAISED))
}

/// The danger line under the mode cards that says why a choice cannot be used.
pub(crate) fn blocker_line(message: &'static str) -> AnyElement {
    row()
        .flex_none()
        .items_start()
        .gap(px(SPACE_1_5))
        .mt(px(SPACE_2))
        .type_style(LABEL)
        .text_color(DANGER)
        .child(div().mt(px(SPACE_0_5)).child(icon(IconName::Alert, 12., DANGER)))
        .child(div().flex_1().min_w_0().child(message))
        .into_any_element()
}

/// A folder in a list (`li.row` of the new-chat page): a tile, the name with its host badge
/// and branch tag, the path under it, and something at the right. 52px, faded on hover.
pub(crate) struct FolderRow {
    pub name: SharedString,
    pub entry: WorkdirEntry,
    pub selected: bool,
    pub compact: bool,
}

impl FolderRow {
    /// The row as an element that calls `on_click` when pressed.
    pub(crate) fn render(
        self,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let entry = &self.entry;
        let (rest, hover) = if self.selected {
            let look = Look::bg(SURFACE_HOVER).border(ACCENT);
            (look, look)
        } else {
            (Look::bg(TRANSPARENT).border(TRANSPARENT), Look::bg(SURFACE_RAISED).border(TRANSPARENT))
        };
        let faded = motion::hover_fade_state(self.name.clone(), rest, hover, window, cx);
        let selector = self.name.to_string();
        let (title, tag) = if entry.is_worktree {
            (entry.branch.clone().unwrap_or_else(|| entry.repo.clone()), None)
        } else {
            (entry.repo.clone(), entry.branch.clone())
        };
        let (right, tone) = right_text(entry);
        let el = row()
            .id(ElementId::Name(self.name.clone()))
            .debug_selector(move || selector)
            .flex_none()
            .gap(px(SPACE_3))
            .h(px(if self.compact { 44. } else { 52. }))
            .px(px(SPACE_3))
            .border_l_2()
            .rounded(px(RADIUS_LG))
            .cursor_pointer()
            .on_click(on_click);
        faded
            .apply(el)
            .child(Tile::new(if entry.is_worktree { IconName::Branch } else { IconName::Folder }))
            .child(
                col()
                    .flex_1()
                    .child(
                        row()
                            .gap(px(SPACE_1_5))
                            .child(
                                div()
                                    .flex_shrink()
                                    .min_w_0()
                                    .type_style(BODY_STRONG)
                                    .font_family(FONT_MONO)
                                    .child(line_text(title, BODY.line)),
                            )
                            .child(Badge::new(host_label(entry.host)).square())
                            .when_some(tag, |d, branch| d.child(Tag::new(branch).mono())),
                    )
                    .child(path_line(display_path(&entry.path, cx), MONO_SM.line).type_style(MONO_SM).text_color(TEXT_MUTED)),
            )
            .child(
                row()
                    .flex_none()
                    .gap(px(SPACE_1_5))
                    .type_style(LABEL)
                    .text_color(tone)
                    .when(entry.owner.is_some() && entry.is_worktree, |d| d.child(StatusDot::new(Status::Running).small()))
                    .child(right),
            )
            .probe(self.name)
            .into_any_element()
    }
}

/// What a folder row says at the right, and in which colour: who works there, the changes
/// waiting in it, or how often it was used.
pub(crate) fn right_text(entry: &WorkdirEntry) -> (String, gpui::Hsla) {
    if let Some(owner) = &entry.owner {
        return (owner.clone(), TEXT_SECONDARY);
    }
    let status = entry.status.as_str();
    if !status.is_empty() && status != "clean" && status != "no remote" {
        return (status.to_string(), WARNING);
    }
    let sessions = match entry.sessions {
        0 => "no sessions".to_string(),
        n => base::fmt::plural(u64::from(n), "session", "sessions"),
    };
    (sessions, TEXT_MUTED)
}

/// The branch picker trigger ("From"), probed `<name>`; `on_click` opens the menu.
pub(crate) fn branch_select(
    name: &'static str,
    branch: Option<&str>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    SelectBox::new(name, branch.unwrap_or("Pick a branch").to_string())
        .icon(IconName::Branch)
        .small()
        .placeholder(branch.is_none())
        .on_click(on_click)
        .popover_anchor(name)
        .into_any_element()
}

/// Called with the branch a menu picks.
pub(crate) type BranchHandler = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Opens the menu of `branches` under the trigger `anchor`; `pick` gets the chosen one.
pub(crate) fn open_branch_menu(
    anchor: &'static str,
    branches: Vec<String>,
    current: Option<String>,
    pick: BranchHandler,
    window: &mut Window,
    cx: &mut App,
) {
    let at = Anchor::of(anchor, window, cx);
    overlays(window, cx).open_menu(
        at,
        Placement::below(),
        move |_, _| {
            let mut menu = Menu::new(ElementId::Name(format!("{anchor}-menu").into())).width(260.).max_height(320.);
            if branches.is_empty() {
                menu = menu.item(MenuItem::new("none", "No branches found").disabled(true));
            }
            for (ix, branch) in branches.iter().enumerate() {
                let pick = pick.clone();
                let chosen = branch.clone();
                menu = menu.item(
                    MenuItem::new(ElementId::NamedInteger(format!("{anchor}-item").into(), ix as u64), branch.clone())
                        .checked(current.as_deref() == Some(branch.as_str()))
                        .check_column()
                        .on_click(move |_, window, cx| pick(chosen.clone(), window, cx)),
                );
            }
            menu
        },
        window,
        cx,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(repo: bool) -> FolderInfo {
        FolderInfo {
            agents_md_lines: None,
            skills: Vec::new(),
            commands: Vec::new(),
            facts: repo.then(|| RepoFacts {
                root: PathBuf::from("/r"),
                name: "r".into(),
                branch: Some("main".into()),
                head: None,
                host: RepoHost::Local,
                remote_url: None,
                dirty: 0,
                ahead: 0,
                behind: 0,
                is_worktree: false,
                main_checkout: PathBuf::from("/r"),
                default_branch: Some("main".into()),
                compared_to: None,
            }),
        }
    }

    #[test]
    fn an_answer_for_an_older_request_is_dropped() {
        let mut facts = FolderFacts::default();
        let first = facts.begin(PathBuf::from("/a"));
        let second = facts.begin(PathBuf::from("/b"));
        assert!(facts.finish(second, info(false)));
        assert!(!facts.finish(first, info(true)), "the answer about /a came late");
        assert_eq!(facts.path(), Some(Path::new("/b")));
        assert!(facts.repo().is_none());
    }

    #[test]
    fn worktree_mode_needs_a_repo_and_a_branch() {
        let mut facts = FolderFacts::default();
        assert_eq!(blocker(WorkMode::Worktree, &facts, ""), None, "no folder blocks nothing");
        let generation = facts.begin(PathBuf::from("/a"));
        assert_eq!(blocker(WorkMode::Worktree, &facts, "x"), Some(Blocker::Loading));
        assert_eq!(blocker(WorkMode::InPlace, &facts, ""), None);
        facts.finish(generation, info(false));
        assert_eq!(blocker(WorkMode::Worktree, &facts, "x"), Some(Blocker::NotARepo));
        assert_eq!(blocker(WorkMode::ReadOnly, &facts, ""), None);
        let generation = facts.begin(PathBuf::from("/r"));
        facts.finish(generation, info(true));
        assert_eq!(blocker(WorkMode::Worktree, &facts, "  "), Some(Blocker::NoBranch));
        assert_eq!(blocker(WorkMode::Worktree, &facts, "chat/x"), None);
    }

    #[test]
    fn a_choice_carries_base_and_branch_only_for_a_worktree() {
        let worktree = choice(Path::new("/r"), WorkMode::Worktree, Some("main"), " chat/x ");
        assert_eq!(worktree.base.as_deref(), Some("main"));
        assert_eq!(worktree.branch.as_deref(), Some("chat/x"));
        let in_place = choice(Path::new("/r"), WorkMode::InPlace, Some("main"), "chat/x");
        assert_eq!((in_place.base, in_place.branch), (None, None));
    }


    #[test]
    fn remotes_read_as_host_and_owner() {
        assert_eq!(remote_owner("https://github.com/acme/payments-api"), "github.com/acme");
        assert_eq!(remote_owner("git@gitlab.acme.dev:platform/sdk.git"), "gitlab.acme.dev/platform");
    }
}
