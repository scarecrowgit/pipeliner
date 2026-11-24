//! The left column of the new-chat page: the heading, the runtimes, the folder list and the
//! action bar.

use gpui::{AnyElement, App, Context, ElementId, IntoElement, KeyDownEvent, SharedString, Window, div, prelude::*, px};
use hub::api::{FolderInfo, Runtime, RuntimeKind, WorkMode, WorkdirEntry};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;
use ui_rich::text_field;

use super::NewChatView;
use super::complete::dir_row;
use crate::workdir::common::{Blocker, FolderRow, display_path, folder_name, line_text, mode_words, path_line, radio};

/// Width of the folder column's content, px.
const COLUMN: f32 = 720.;

/// The probe part of a runtime: `new-chat-runtime-local`.
pub(crate) fn runtime_slug(kind: RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::Local => "local",
        RuntimeKind::Cloud => "cloud",
        RuntimeKind::Remote => "remote",
    }
}

/// The line under a runtime's name: "local · online · 3 agents", "Coming soon".
pub(crate) fn runtime_line(runtime: &Runtime) -> String {
    if !runtime.available {
        "Coming soon".to_string()
    } else if runtime.kind == RuntimeKind::Local {
        let state = if runtime.online { "online" } else { "offline" };
        format!("local · {state} · {}", base::fmt::plural(u64::from(runtime.slots_busy), "agent", "agents"))
    } else {
        runtime.note.clone()
    }
}

impl NewChatView {
    pub(super) fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // A definite width, from the scroll area's width in the last frame: as `w_full` capped
        // by `max_w`, a long path measured the search field wider than the column and left
        // its text no room.
        let area = f32::from(self.main_scroll.bounds().size.width);
        let content = col()
            .when(area > 0., |d| d.w(px(area.min(COLUMN))))
            .when(area <= 0., |d| d.w_full().max_w(px(COLUMN)))
            .overflow_hidden()
            .px(px(SPACE_6))
            .py(px(SPACE_8))
            .child(self.render_heading(cx))
            .child(self.render_runtimes(window, cx))
            .child(self.render_folders(window, cx));
        col()
            .flex_1()
            .min_w_0()
            .h_full()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                if keys.key == "o" && (keys.modifiers.platform || keys.modifiers.control) {
                    cx.stop_propagation();
                    this.browse(window, cx);
                }
            }))
            .child(
                div()
                    .id("new-chat-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.main_scroll)
                    .child(col().w_full().items_center().child(content)),
            )
            .child(self.render_action_bar(cx))
            .into_any_element()
    }

    /// Draws again when the scroll area's width changed since this frame was built.
    pub(super) fn follow_main_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let used = f32::from(self.main_scroll.bounds().size.width);
        cx.defer_in(window, move |this, _, cx| {
            if (f32::from(this.main_scroll.bounds().size.width) - used).abs() >= 0.5 {
                cx.notify();
            }
        });
    }

    fn render_heading(&self, cx: &App) -> impl IntoElement {
        col()
            .flex_none()
            .child(div().type_style(TITLE).child(format!("Where should {} work?", self.agent_name(cx))))
            .child(
                div()
                    .mt(px(SPACE_1))
                    .text_color(TEXT_SECONDARY)
                    .child("The agent sees only this folder. Pick a runtime, then a repo or worktree."),
            )
    }

    fn render_runtimes(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let runtimes = self.runtimes(cx);
        let chosen = self.runtime(cx).map(|runtime| runtime.name);
        let mut grid = div().grid().grid_cols(3).gap(px(SPACE_2)).mt(px(SPACE_1_5));
        for runtime in &runtimes {
            let selected = chosen.as_deref() == Some(runtime.name.as_str());
            grid = grid.child(self.runtime_card(runtime, selected, window, cx));
        }
        col()
            .flex_none()
            .mt(px(SPACE_5))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Runtime"))
            .child(grid)
    }

    /// A runtime card (`button.card`): tile, name, a line about it, a radio. Runtimes that are
    /// not available are dimmed, say "Coming soon" and take no clicks.
    fn runtime_card(&self, runtime: &Runtime, selected: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let name: SharedString = format!("new-chat-runtime-{}", runtime_slug(runtime.kind)).into();
        let disabled = !runtime.available;
        let rest = if selected { Look::bg(SURFACE_RAISED).border(ACCENT_RING) } else { Look::bg(SURFACE).border(LINE) };
        let hover = if selected || disabled { rest } else { Look::bg(SURFACE_RAISED).border(LINE) };
        let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
        let icon_name = match runtime.kind {
            RuntimeKind::Local => IconName::Server,
            RuntimeKind::Cloud => IconName::Cloud,
            RuntimeKind::Remote => IconName::Laptop,
        };
        let sub = runtime_line(runtime);
        let selector = name.to_string();
        let picked = runtime.name.clone();
        let el = div()
            .id(ElementId::Name(name.clone()))
            .debug_selector(move || selector)
            .flex()
            .items_center()
            .gap(px(SPACE_2))
            .p(px(SPACE_3))
            .min_w_0()
            .border_1()
            .rounded(px(RADIUS_LG))
            .when(selected, |d| d.shadow(selected_glow()))
            .when(disabled, |d| d.opacity(0.55).cursor_not_allowed())
            .when(!disabled, |d| {
                d.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.set_runtime(&picked, cx)))
            });
        let dot = radio(selected);
        faded
            .apply(el)
            .child(Tile::new(icon_name).tone(if selected { TileTone::Accent } else { TileTone::Neutral }))
            .child(
                col()
                    .flex_1()
                    .child(div().type_style(BODY_STRONG).child(truncate(runtime.name.clone())))
                    // Wraps rather than cuts: "local · online · 3 agents" must read whole.
                    .child(div().type_style(LABEL).text_color(if disabled { WARNING } else { TEXT_MUTED }).child(sub)),
            )
            .child(dot)
            .probe(name)
            .into_any_element()
    }

    fn render_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let listing = self.listing(cx);
        let chosen = self.facts.path().map(|path| path.to_path_buf());
        let mut list = col().flex_none().mt(px(SPACE_5)).child(
            text_field("new-chat-folder", &self.search).icon(IconName::Search).kbd("/").probe("new-chat-folder"),
        );

        let mut ix = 0usize;
        let mut rows = |entries: &[WorkdirEntry], list: gpui::Div, window: &mut Window, cx: &mut Context<Self>| {
            let mut group = col().flex_none().gap(px(1.));
            for entry in entries {
                let name: SharedString = format!("new-chat-folder-row-{ix}").into();
                ix += 1;
                let selected = chosen.as_ref() == Some(&entry.path);
                let picked = entry.clone();
                let on_click = cx.listener(move |this, _, window, cx| {
                    this.select_entry(&picked, cx);
                    this.search.read(cx).focus(window);
                });
                let row = FolderRow { name, entry: entry.clone(), selected, compact: false };
                group = group.child(row.render(on_click, window, cx));
            }
            list.child(group)
        };

        if let Some(typed) = &listing.typed {
            list = list.child(group_head("Open folder", String::new()));
            let info = self.typed_folder(typed).cloned();
            let exists = self.typed_exists(typed);
            list = list.child(typed_row(typed, info, exists, chosen.as_deref() == Some(typed.as_path()), window, cx));
            let dirs: Vec<_> = self.dir_rows().to_vec();
            if !dirs.is_empty() {
                list = list.child(group_head("Folders here", dirs.len().to_string()));
                let mut group = col().flex_none().gap(px(1.));
                for (dir_ix, dir) in dirs.iter().enumerate() {
                    group = group.child(dir_row(dir_ix, dir, window, cx));
                }
                list = list.child(group);
            }
            if !listing.completions.is_empty() {
                list = list.child(group_head("Known folders here", listing.completions.len().to_string()));
                list = rows(&listing.completions, list, window, cx);
            }
        } else {
            let parent = self
                .recent
                .first()
                .and_then(|entry| entry.path.parent())
                .map(|p| display_path(p, cx))
                .unwrap_or_default();
            list = list.child(group_head("Recent", parent));
            if listing.recent.is_empty() {
                let text = if self.recent.is_empty() { "No folders used yet" } else { "No recent folder matches" };
                list = list.child(empty_line(text));
            }
            list = rows(&listing.recent, list, window, cx);
            if !listing.worktrees.is_empty() {
                let repo = self.facts.repo().map(|facts| facts.name.clone()).unwrap_or_default();
                let (shown, total) = (listing.worktrees.len(), listing.worktrees_total);
                let count = if total > shown { format!("{shown} of {total}") } else { total.to_string() };
                list = list.child(group_head(&format!("Worktrees · {repo}"), count));
                list = rows(&listing.worktrees, list, window, cx);
            }
        }

        list.child(
            row()
                .flex_none()
                .mt(px(SPACE_3))
                .gap(px(SPACE_2))
                .child(
                    Button::new("new-chat-open-folder", "Open folder…")
                        .ghost()
                        .small()
                        .icon(IconName::Folder)
                        .kbd("⌘O")
                        .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
                )
                .child(
                    Button::new("new-chat-clone", "Clone from GitHub…")
                        .ghost()
                        .small()
                        .icon(IconName::Source)
                        .selected(self.cloning)
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_clone(window, cx))),
                ),
        )
        .when(self.cloning, |d| d.child(self.render_clone(cx)))
    }

    fn render_clone(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.clone_url.read(cx).text().trim().is_empty();
        col()
            .flex_none()
            .mt(px(SPACE_2))
            .gap(px(SPACE_1_5))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(text_field("new-chat-clone-url", &self.clone_url).icon(IconName::Source).small()),
                    )
                    .child(
                        Button::new("new-chat-clone-here", "Clone here")
                            .secondary()
                            .small()
                            .disabled(empty)
                            .on_click(cx.listener(|this, _, _, cx| this.clone_repo(cx))),
                    ),
            )
            .child(Note::new(format!("Clones into {}", display_path(&self.clone_parent(cx), cx))))
    }

    fn render_action_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary(cx);
        let blocked = self.blocker(cx).and_then(Blocker::message);
        let note = match (&self.failed, blocked) {
            (Some(failed), _) => div().text_color(DANGER).child(failed.clone()),
            (None, Some(message)) => div().text_color(DANGER).child(message),
            (None, None) => div().text_color(TEXT_MUTED).child("Default for new chats: last used folder · change in Settings"),
        };
        let label = match self.facts.path() {
            Some(path) => format!("Start in {}", folder_name(path)),
            None => "Start without a folder".to_string(),
        };
        row()
            .flex_none()
            .gap(px(SPACE_2))
            .h(px(ACTION_BAR_HEIGHT))
            .pl(px(SPACE_5))
            .pr(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                col()
                    .flex_1()
                    .child(summary)
                    .child(note.type_style(LABEL)),
            )
            // Stacked, as drawn: the summary keeps the room.
            .child(
                col()
                    .flex_none()
                    .gap(px(SPACE_1))
                    .child(KeyHints::new().hint(["↑", "↓"], "move"))
                    .child(KeyHints::new().hint(["⌘O"], "open folder")),
            )
            .child(
                Button::new("new-chat-start", label)
                    .primary()
                    .kbd("↵")
                    .disabled(!self.can_start(cx))
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                    .probe("new-chat-start"),
            )
    }

    /// "payments-api on mac-mini · new worktree chat/fix from main".
    fn summary(&self, cx: &App) -> AnyElement {
        let Some(path) = self.facts.path() else {
            return div().text_color(TEXT_MUTED).child("No folder: the agent can only talk").into_any_element();
        };
        let runtime = self.runtime(cx).map_or_else(|| "this machine".to_string(), |runtime| runtime.name);
        let muted = |text: String| div().flex_none().text_color(TEXT_MUTED).child(text);
        let mono = |text: String| div().flex_none().font_family(FONT_MONO).type_style(MONO).child(text);
        // The branch gives way first: it ends in `…` rather than being clipped.
        let branch_cell =
            |text: String| div().flex_shrink().min_w_0().font_family(FONT_MONO).type_style(MONO).child(line_text(text, MONO.line));
        let mut line = row()
            .gap(px(SPACE_1))
            .overflow_hidden()
            .child(mono(folder_name(path)).font_weight(gpui::FontWeight::MEDIUM))
            .child(muted("on".into()))
            .child(div().flex_none().child(runtime))
            .child(muted(format!("· {}", mode_words(self.mode))));
        if self.mode == WorkMode::Worktree {
            let branch = self.branch.read(cx).text().to_string();
            if !branch.is_empty() {
                line = line.child(branch_cell(branch));
            }
            if let Some(base) = &self.base {
                line = line.child(muted("from".into())).child(mono(base.clone()));
            }
        } else if let Some(branch) = self.facts.branch() {
            line = line.child(muted("on".into())).child(branch_cell(branch.to_string()));
        }
        line.into_any_element()
    }
}

/// A group label of the folder list (`Recent ~/code/acme`).
fn group_head(title: &str, meta: String) -> impl IntoElement {
    row()
        .flex_none()
        .h(px(32.))
        .mt(px(SPACE_3))
        .gap(px(SPACE_2))
        .type_style(LABEL)
        .text_color(TEXT_MUTED)
        .child(
            div().flex_none().font_weight(gpui::FontWeight::MEDIUM).text_color(TEXT_SECONDARY).child(title.to_string()),
        )
        // A long path loses its middle and then its end; it never pushes the column wider.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .type_style(MONO_SM)
                .text_right()
                .child(path_line(meta, MONO_SM.line).justify_end()),
        )
}

fn empty_line(text: &'static str) -> impl IntoElement {
    div().flex_none().h(px(LIST_ITEM_HEIGHT)).px(px(SPACE_3)).type_style(LABEL).text_color(TEXT_MUTED).child(text)
}

/// "Use ~/code/x": the typed path as a row.
fn typed_row(
    path: &std::path::Path,
    info: Option<FolderInfo>,
    exists: Option<bool>,
    selected: bool,
    window: &mut Window,
    cx: &mut Context<NewChatView>,
) -> AnyElement {
    let name: SharedString = "new-chat-folder-path".into();
    let rest = if selected { Look::bg(SURFACE_HOVER) } else { Look::bg(TRANSPARENT) };
    let hover = if selected { rest } else { Look::bg(SURFACE_RAISED) };
    let faded = motion::hover_fade_state(name.clone(), rest, hover, window, cx);
    let (sub, tone) = typed_words(info.as_ref(), exists);
    let picked = path.to_path_buf();
    let el = row()
        .id(ElementId::Name(name.clone()))
        .debug_selector(|| "new-chat-folder-path".into())
        .flex_none()
        .gap(px(SPACE_3))
        .h(px(52.))
        .px(px(SPACE_3))
        .rounded(px(RADIUS_LG))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| {
            this.select_folder(picked.clone(), cx);
            this.search.read(cx).focus(window);
        }));
    faded
        .apply(el)
        .child(Tile::new(IconName::Folder))
        .child(
            col()
                .flex_1()
                .child(
                    row()
                        .gap(px(SPACE_1_5))
                        .child(div().flex_none().type_style(BODY_STRONG).child("Open"))
                        .child(div().flex_1().min_w_0().type_style(MONO).child(path_line(display_path(path, cx), MONO.line))),
                )
                .child(div().type_style(LABEL).text_color(tone).child(sub)),
        )
        .child(Kbd::new("↵"))
        .probe(name)
        .into_any_element()
}

/// What the typed folder's row says under its path, and in which colour: what is known of
/// the folder once it was looked at, "No such folder" apart from "not a git repository".
pub(crate) fn typed_words(info: Option<&FolderInfo>, exists: Option<bool>) -> (String, gpui::Hsla) {
    let Some(info) = info else { return ("Looking at the folder…".to_string(), TEXT_MUTED) };
    if let Some(facts) = &info.facts {
        return (format!("git repository · {}", facts.branch.as_deref().unwrap_or("detached head")), TEXT_SECONDARY);
    }
    match exists {
        Some(false) => ("No such folder".to_string(), WARNING),
        Some(true) => ("Folder · not a git repository".to_string(), TEXT_SECONDARY),
        None => ("Looking at the folder…".to_string(), TEXT_MUTED),
    }
}
