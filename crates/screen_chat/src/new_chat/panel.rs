//! The right panel of the new-chat page ("Selected folder"): the folder with its remote, who
//! else works in the repo, how to work there, and what the agent will find.

use std::rc::Rc;

use gpui::{AnyElement, App, Context, IntoElement, Window, div, prelude::*, px};
use hub::api::WorkMode;
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::text_field;

use super::NewChatView;
use crate::workdir::common::{
    self, Blocker, blocker_line, display_path, folder_name, git_state, host_label, mode_cards, remote_owner,
};

/// Width of the panel, px (the mockup's 400).
const WIDTH: f32 = 400.;
/// The base-branch trigger's id and anchor.
const BASE: &str = "new-chat-base";

impl NewChatView {
    pub(super) fn render_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let panel = div()
            .id("new-chat-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(WIDTH))
            .h_full()
            .border_l_1()
            .border_color(LINE)
            .bg(BG)
            .overflow_y_scroll();
        if self.facts.path().is_none() {
            return panel
                .child(Section::new("No folder yet").last().child(Note::new(
                    "Pick a folder to see its branch, its remote and what the agent will find there. \
                     Without one the agent can only talk.",
                )))
                .into_any_element();
        }
        panel
            .child(self.render_head(cx))
            .children(self.render_others())
            .child(self.render_modes(window, cx))
            .child(self.render_found())
            .into_any_element()
    }

    fn render_head(&self, cx: &App) -> impl IntoElement {
        let path = self.facts.path().map(|path| path.to_path_buf()).unwrap_or_default();
        let runtime = self.runtime(cx).map_or_else(|| "this machine".to_string(), |runtime| runtime.name);
        // The API says nothing about the connection to the host, so no "connected" note.
        let remote = match (self.facts.is_loading(), self.facts.repo()) {
            (true, _) => row().gap(px(SPACE_2)).text_color(TEXT_MUTED).child("Reading the folder…"),
            (false, None) => row()
                .gap(px(SPACE_2))
                .child(StatusDot::new(Status::Idle).small())
                .child(div().text_color(TEXT_MUTED).child("Not a git repository")),
            (false, Some(facts)) => row()
                .gap(px(SPACE_2))
                .child(StatusDot::new(if facts.remote_url.is_some() { Status::Live } else { Status::Idle }).small())
                .child(div().flex_none().child(host_label(facts.host)))
                .children(match facts.remote_url.as_deref().map(remote_owner) {
                    Some(owner) => Some(div().flex_shrink().min_w_0().type_style(MONO).child(truncate(owner))),
                    None => Some(div().flex_none().text_color(TEXT_MUTED).child("no remote")),
                }),
        };
        col()
            .flex_none()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .border_b_1()
            .border_color(LINE)
            .child(
                row()
                    .gap(px(SPACE_2_5))
                    .child(Tile::new(IconName::Folder).large())
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                truncate_block(folder_name(&path), HEADING.line)
                                    .type_style(HEADING)
                                    .font_family(FONT_MONO)
                                    .debug_selector(|| "new-chat-panel-name".into())
                                    .probe("new-chat-panel-name"),
                            )
                            .child(
                                truncate_block(format!("{runtime} · {}", display_path(&path)), MONO_SM.line)
                                    .type_style(MONO_SM)
                                    .text_color(TEXT_MUTED)
                                    .debug_selector(|| "new-chat-panel-path".into())
                                    .probe("new-chat-panel-path"),
                            ),
                    ),
            )
            .child(div().mt(px(SPACE_3)).type_style(LABEL).child(remote))
    }

    /// "Also working in this repo": worktrees someone else works in.
    fn render_others(&self) -> Option<AnyElement> {
        let others: Vec<_> = self.worktrees.iter().filter(|entry| entry.owner.is_some()).collect();
        if others.is_empty() {
            return None;
        }
        let mut list = col().gap(px(SPACE_1_5));
        for entry in &others {
            list = list.child(
                row()
                    .gap(px(SPACE_2))
                    .child(StatusDot::new(Status::Running).small())
                    .child(div().flex_none().type_style(BODY_STRONG).child(entry.owner.clone().unwrap_or_default()))
                    .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child("worktree"))
                    .child(grow())
                    .child(
                        div()
                            .flex_shrink()
                            .min_w_0()
                            .type_style(MONO)
                            .child(truncate(entry.branch.clone().unwrap_or_default())),
                    ),
            );
        }
        Some(Section::new("Also working in this repo").meta(others.len().to_string()).child(list).into_any_element())
    }

    fn render_modes(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.weak_entity();
        let on_mode: common::ModeHandler = Rc::new(move |mode, _, cx| {
            this.update(cx, |this, cx| this.set_mode(mode, cx)).ok();
        });
        let worktree = self.mode == WorkMode::Worktree && self.facts.repo().is_some();
        let blocked = self.blocker(cx).and_then(Blocker::message);
        let label = |text: &'static str| div().type_style(LABEL).text_color(TEXT_MUTED).child(text);
        Section::new("How to work here")
            .child(mode_cards("new-chat", self.mode, self.facts.branch(), on_mode))
            .when(worktree, |d| {
                let open = cx.listener(|this, _, window, cx| {
                    let pick = {
                        let this = cx.weak_entity();
                        Rc::new(move |branch: String, _: &mut Window, cx: &mut App| {
                            this.update(cx, |this, cx| this.set_base(branch, cx)).ok();
                        })
                    };
                    common::open_branch_menu(BASE, this.branches.clone(), this.base.clone(), pick, window, cx);
                });
                d.child(
                    row()
                        .mt(px(SPACE_3))
                        .gap(px(SPACE_2))
                        .items_start()
                        .child(
                            col()
                                .w(px(120.))
                                .flex_none()
                                .gap(px(SPACE_1_5))
                                .child(label("From"))
                                .child(common::branch_select(BASE, self.base.as_deref(), open)),
                        )
                        .child(
                            col()
                                .flex_1()
                                .gap(px(SPACE_1_5))
                                .child(label("New branch"))
                                .child(text_field("new-chat-branch", &self.branch).small()),
                        ),
                )
            })
            .when_some(blocked, |d, message| d.child(blocker_line(message)))
    }

    /// "Found in this folder": instructions, skills, commands and git state.
    fn render_found(&self) -> impl IntoElement {
        let value = |text: String| div().type_style(LABEL).text_color(TEXT_SECONDARY).child(text);
        let mut facts = KeyValue::new().key_width(128.);
        match self.facts.info() {
            None => {
                facts = facts.row("git status", value("Reading…".into()));
            }
            Some(info) => {
                let agents = info
                    .agents_md_lines
                    .map_or_else(|| "none".to_string(), |n| format!("{n} lines · loaded every session"));
                let list = |items: &[String], prefix: &str| {
                    if items.is_empty() {
                        "none".to_string()
                    } else {
                        items.iter().map(|item| format!("{prefix}{item}")).collect::<Vec<_>>().join(", ")
                    }
                };
                let git = match &info.facts {
                    Some(repo) => {
                        let text = git_state(repo);
                        let clean = text.starts_with("clean");
                        div().type_style(LABEL).text_color(if clean { SUCCESS } else { WARNING }).child(text)
                    }
                    None => div().type_style(LABEL).text_color(TEXT_MUTED).child("not a git repository"),
                };
                facts = facts
                    .mono_row("AGENTS.md", value(agents))
                    .mono_row(".harness/skills", value(list(&info.skills, "")).font_family(FONT_MONO))
                    .mono_row(".harness/commands", value(list(&info.commands, "/")).font_family(FONT_MONO))
                    .mono_row("git status", git);
            }
        }
        Section::new("Found in this folder").last().child(facts)
    }
}
