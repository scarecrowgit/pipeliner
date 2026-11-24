//! The workdir popover (`workdir_popover()` of the mockups): where the chat works now, the
//! folders and worktrees it could move to, and the actions on its folder.

use gpui::{AnyElement, App, ElementId, Entity, Hsla, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{WorkMode, Workdir, WorkdirEntry};
use ui::dev::Probe;
use ui::motion;
use ui::prelude::*;
use ui_rich::text_field;

use super::WorkdirChip;
use super::common::{SWITCH_NOTE, display_path, entry_matches, git_state, host_label, line_text, path_line, workdir_label};

/// Width of the popover, px (the mockup's 480).
const WIDTH: f32 = 480.;

/// What one frame of the popover shows, read from the chip before anything is drawn.
struct Shown {
    session: bool,
    current: Option<Workdir>,
    state: Option<String>,
    places: Vec<WorkdirEntry>,
    loading: bool,
}

fn read(chip: &Entity<WorkdirChip>, cx: &App) -> Shown {
    let this = chip.read(cx);
    let query = this.search().read(cx).text().to_string();
    let (session, current, state, loading) = match this.model() {
        Some(model) => {
            let model = model.read(cx);
            let current = model.session().and_then(|session| session.workdir.clone());
            let state = model.detail().map(|detail| {
                let mut parts = Vec::new();
                if let Some(repo) = &detail.repo {
                    parts.push(git_state(repo));
                }
                if !detail.files.is_empty() {
                    let n = detail.files.len() as u64;
                    parts.push(format!("{} edited in this chat", base::fmt::plural(n, "file", "files")));
                }
                parts.join(" · ")
            });
            (true, current, state.filter(|s| !s.is_empty()), model.session().is_none())
        }
        None => (false, None, None, false),
    };
    let current_path = current.as_ref().map(|workdir| workdir.path.clone());
    let places = this
        .places()
        .iter()
        .filter(|entry| Some(&entry.path) != current_path.as_ref())
        .filter(|entry| entry_matches(entry, &query, cx))
        .cloned()
        .collect();
    Shown { session, current, state, places, loading }
}

/// The popover's content for `chip`, rebuilt every frame while it is open.
pub(super) fn build(chip: &Entity<WorkdirChip>, window: &mut Window, cx: &mut App) -> AnyElement {
    let shown = read(chip, cx);
    let search = chip.read(cx).search().clone();
    let mut pop = Popover::new().width(WIDTH);

    if shown.session {
        pop = pop.child(head(&shown, cx));
        pop = pop.child(PopSeparator);
    }
    pop = pop.child(
        div()
            .mt(px(SPACE_1_5))
            .mb(px(SPACE_1))
            .child(text_field("workdir-popover-search", &search).icon(IconName::Search).small()),
    );
    if !shown.session {
        pop = pop.child(PopLabel::new("Recent folders"));
    }

    let mut list = col().flex_none().id("workdir-popover-list").max_h(px(300.)).overflow_y_scroll();
    if let Some(current) = &shown.current {
        let entry = entry_of(current);
        list = list.child(item("workdir-popover-current".into(), &entry, true, ("this chat".into(), TEXT_MUTED), None, window, cx));
    }
    for (ix, entry) in shown.places.iter().enumerate() {
        let name: SharedString = format!("workdir-popover-folder-{ix}").into();
        let hint = entry.owner.clone().unwrap_or_else(|| entry.status.clone());
        let tone = if entry.status.contains("expires") { WARNING } else { TEXT_MUTED };
        let (chip, picked) = (chip.clone(), entry.clone());
        let on_click: Click = Box::new(move |window, cx| chip.update(cx, |chip, cx| chip.pick(picked.clone(), window, cx)));
        list = list.child(item(name, entry, false, (hint, tone), Some(on_click), window, cx));
    }
    if shown.places.is_empty() && shown.current.is_none() {
        let text = if shown.loading { "Loading…" } else { "No folders found" };
        list = list.child(div().px(px(SPACE_2_5)).py(px(SPACE_2)).type_style(LABEL).text_color(TEXT_MUTED).child(text));
    }
    pop = pop.child(list).child(PopSeparator);

    let browse = chip.clone();
    pop = pop.child(
        MenuItem::new("workdir-popover-switch", "All folders, runtimes and repos…")
            .icon(IconName::Folder)
            .kbd("⌘O")
            .on_click(move |_, window, cx| browse.update(cx, |chip, cx| chip.browse(window, cx)))
            .probe_name("workdir-popover-switch"),
    );
    let has_path = shown.current.is_some() || chip.read(cx).choice().is_some();
    if has_path {
        let copy = chip.clone();
        pop = pop.child(
            MenuItem::new("workdir-popover-copy", "Copy path")
                .icon(IconName::Copy)
                .on_click(move |_, window, cx| copy.update(cx, |chip, cx| chip.copy_path(window, cx)))
                .probe_name("workdir-popover-copy"),
        );
    }
    if shown.session {
        pop = pop.child(div().px(px(SPACE_2_5)).pt(px(SPACE_1)).pb(px(SPACE_1_5)).child(Note::new(SWITCH_NOTE)));
    } else if chip.read(cx).choice().is_some() {
        let clear = chip.clone();
        pop = pop.child(
            MenuItem::new("workdir-popover-none", "No folder")
                .icon(IconName::X)
                .subline("The agent can only talk")
                .on_click(move |_, window, cx| clear.update(cx, |chip, cx| chip.clear(window, cx)))
                .probe_name("workdir-popover-none"),
        );
    }
    pop.into_any_element()
}

/// "Working in": the session's folder, branch, runtime, path and git state.
fn head(shown: &Shown, cx: &App) -> AnyElement {
    let mut head = col().flex_none().px(px(SPACE_2_5)).pt(px(SPACE_2_5)).pb(px(SPACE_2));
    head = head.child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Working in"));
    match &shown.current {
        Some(workdir) => {
            let runtime = if workdir.runtime.is_empty() { "local".to_string() } else { workdir.runtime.clone() };
            head = head
                .child(
                    row()
                        .mt(px(SPACE_0_5))
                        .gap(px(SPACE_2))
                        .child(
                            div()
                                .flex_shrink()
                                .min_w_0()
                                .type_style(BODY_STRONG)
                                .font_family(FONT_MONO)
                                .child(line_text(workdir_label(&workdir.repo, workdir.branch.as_deref()), BODY.line)),
                        )
                        .child(Badge::new(host_label(workdir.host)).square()),
                )
                .child(
                    div()
                        .mt(px(SPACE_0_5))
                        .type_style(MONO_SM)
                        .text_color(TEXT_MUTED)
                        .child(path_line(format!("{runtime} · {}", display_path(&workdir.path, cx)), MONO_SM.line)),
                )
                .when_some(shown.state.clone(), |d, state| {
                    d.child(div().mt(px(SPACE_1)).type_style(LABEL).text_color(TEXT_SECONDARY).child(state))
                });
        }
        None => {
            let text = if shown.loading { "Loading…" } else { "No folder: the agent can only talk" };
            head = head.child(div().mt(px(SPACE_0_5)).text_color(TEXT_SECONDARY).child(text));
        }
    }
    head.into_any_element()
}

/// A session's workdir as a list entry.
fn entry_of(workdir: &Workdir) -> WorkdirEntry {
    WorkdirEntry {
        path: workdir.path.clone(),
        repo: workdir.repo.clone(),
        branch: workdir.branch.clone(),
        host: workdir.host,
        is_worktree: workdir.mode == WorkMode::Worktree || workdir.source.is_some(),
        status: String::new(),
        owner: Some("this chat".to_string()),
        sessions: 0,
    }
}

type Click = Box<dyn Fn(&mut Window, &mut App)>;

/// A workdir row (`.mi`): check column, `repo · branch` with its host, the path, a hint.
fn item(
    name: SharedString,
    entry: &WorkdirEntry,
    current: bool,
    (hint, tone): (String, Hsla),
    on_click: Option<Click>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (rest, hover) = motion::row_looks(false, SURFACE_HOVER);
    let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
    let selector = name.to_string();
    let el = row()
        .id(ElementId::Name(name.clone()))
        .debug_selector(move || selector)
        .flex_none()
        .gap(px(SPACE_2_5))
        .w_full()
        .min_h(px(MENU_ITEM_HEIGHT))
        .px(px(SPACE_2_5))
        .py(px(SPACE_1_5))
        .rounded(px(RADIUS_MD))
        .when(on_click.is_some(), |d| d.cursor_pointer())
        .when_some(on_click, |d, f| d.on_click(move |_, window, cx| f(window, cx)));
    faded
        .apply(el)
        .child(div().flex().flex_none().w(px(14.)).when(current, |d| d.child(icon(IconName::Check, 14., ACCENT_TEXT))))
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
                                .type_style(MONO)
                                .child(line_text(workdir_label(&entry.repo, entry.branch.as_deref()), MONO.line)),
                        )
                        .child(Badge::new(host_label(entry.host)).square()),
                )
                .child(path_line(display_path(&entry.path, cx), MONO_SM.line).type_style(MONO_SM).text_color(TEXT_MUTED)),
        )
        .child(div().flex_none().type_style(LABEL).text_color(tone).whitespace_nowrap().child(hint))
        .probe(name)
        .into_any_element()
}
