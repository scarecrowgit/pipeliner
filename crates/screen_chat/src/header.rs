//! Page header: breadcrumb with the session title, the workdir chip with its "Working
//! folder" dropdown (move / fork the chat to another folder), New session.

use std::path::PathBuf;

use gpui::{Context, ElementId, FontWeight, IntoElement, deferred, div, prelude::*, px};
use ui::{Button, icon, theme::*};

use crate::widgets::prov_pill;
use crate::workdir::{menu_note, other_session, prov_view, row_name, show_path};
use crate::{ChatScreen, Trigger};

/// Width of the "Working folder" dropdown.
const WD_MENU_WIDTH: f32 = 420.;

fn eid(name: &'static str, ix: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), ix as u64)
}

impl ChatScreen {
    /// `repo · branch` of the current folder.
    pub(crate) fn wd_label(&self) -> String {
        match &self.repo {
            Some((name, Some(branch))) => format!("{name} · {branch}"),
            Some((name, None)) => name.clone(),
            None => self.repo_name(),
        }
    }

    pub(crate) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let new_chat = self.newchat.is_some();
        let title = if new_chat {
            "New session".to_string()
        } else {
            self.title()
        };
        let breadcrumb = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .text_size(px(14.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
            .child(
                div()
                    .id("crumb-chat")
                    .flex_none()
                    .text_color(TEXT_MUTED)
                    .when(new_chat, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .on_click(cx.listener(|this, _, _, cx| this.close_new_chat(cx)))
                    })
                    .child("Chat"),
            )
            .child(icon("chevron-right", 12., TEXT_DISABLED))
            .child(div().min_w_0().truncate().child(title));

        let chip = if new_chat {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.))
                .h(px(24.))
                .px(px(8.))
                .rounded_full()
                .border_1()
                .border_dashed()
                .border_color(LINE_STRONG)
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(icon("folder", 12., TEXT_MUTED))
                .child("no folder yet")
                .into_any_element()
        } else {
            self.wd_chip(cx).into_any_element()
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(12.))
            .min_h(px(HEADER_HEIGHT))
            .px(px(24.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .bg(BG_SIDEBAR)
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(ACCENT_SOFT)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("pipelines", 16., ACCENT_TEXT)),
            )
            .child(breadcrumb)
            .child(chip)
            .child(div().flex_1())
            .when(!new_chat, |d| {
                d.child(
                    Button::new("new-session", "New session")
                        .icon("plus")
                        .on_click(cx.listener(|this, _, _, cx| this.open_new_chat(cx))),
                )
            })
    }

    fn wd_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.wd.open;
        let pv = prov_view(self.cur_fact());
        let trigger = div()
            .id("wd-chip")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .h(px(28.))
            .px(px(8.))
            .rounded_full()
            .border_1()
            .border_color(if open { ACCENT_RING } else { LINE_STRONG })
            .when(open, |d| d.bg(SURFACE_HOVER))
            .hover(|s| s.bg(SURFACE_HOVER))
            .cursor_pointer()
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .text_color(TEXT_SECONDARY)
            .whitespace_nowrap()
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                if *hovered {
                    this.trigger_hover = Some(Trigger::Workdir);
                } else if this.trigger_hover == Some(Trigger::Workdir) {
                    this.trigger_hover = None;
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_wd_menu(cx)))
            .child(ui::status_dot(pv.status, true))
            .child(icon("folder", 12., TEXT_SECONDARY))
            .child(self.wd_label())
            .child(icon("chevron-down", 12., TEXT_MUTED));
        div().relative().child(trigger).when(open, |d| {
            d.child(
                deferred(
                    div()
                        .absolute()
                        .left_0()
                        .top_full()
                        .pt(px(6.))
                        .w(px(WD_MENU_WIDTH))
                        .child(div().id("wd-menu").occlude().child(self.wd_menu(cx))),
                )
                .with_priority(1),
            )
        })
    }

    fn wd_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let root = self.workspace.root.clone();
        let fact = self.cur_fact();
        let pv = prov_view(fact);
        let git = match fact {
            None => "checking git…".to_string(),
            Some(f) if f.summary.is_empty() => "not a git repository".to_string(),
            Some(f) => f.summary.clone(),
        };
        let current = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(14.))
            .py(px(12.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(format!("Working in · {}", self.work_mode.label())),
            )
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
                            .child(row_name(&root, fact)),
                    )
                    .child(prov_pill(pv.label.clone(), pv.status)),
            )
            .child(
                div()
                    .truncate()
                    .font_family(FONT_MONO)
                    .text_size(px(11.))
                    .text_color(TEXT_MUTED)
                    .child(format!("{} · {}", self.wd.host, show_path(&root))),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(TEXT_SECONDARY)
                    .child(git),
            );

        let pending = self.wd.pending.clone();
        let mut rows = Vec::new();
        for (i, dir) in self.menu_rows().into_iter().enumerate() {
            rows.push(self.wd_row(i, dir, pending.as_ref(), cx));
        }

        let confirm = pending.map(|dir| {
            let name = row_name(&dir, self.wd.facts.get(&dir));
            let left = match fact.and_then(|f| f.git.as_ref()) {
                Some(g) if g.changed > 0 => format!(
                    "The {} uncommitted file{} stay in {}, nothing is copied.",
                    g.changed,
                    if g.changed == 1 { "" } else { "s" },
                    self.wd_label()
                ),
                _ => "Nothing to leave behind.".to_string(),
            };
            let (move_dir, fork_dir) = (dir.clone(), dir);
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(14.))
                .py(px(12.))
                .border_t_1()
                .border_color(LINE)
                .bg(WARNING_SOFT)
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(WARNING)
                        .child(format!("Move this chat to {name}?")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(TEXT_SECONDARY)
                        .child(format!(
                            "{} continues there with the same history. {left}",
                            self.agent_name()
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .child(
                            Button::new("wd-move", "Move chat")
                                .small()
                                .primary()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.move_chat(move_dir.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new("wd-fork", "Fork a new chat there")
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.fork_chat(fork_dir.clone(), cx)
                                })),
                        )
                        .child(Button::new("wd-cancel", "Cancel").small().ghost().on_click(
                            cx.listener(|this, _, _, cx| {
                                this.wd.pending = None;
                                cx.notify();
                            }),
                        )),
                )
        });

        div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(10.))
            .bg(SURFACE_RAISED)
            .shadow(popover_shadow())
            .overflow_hidden()
            .text_size(px(13.))
            .font_weight(FontWeight::NORMAL)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.trigger_hover != Some(Trigger::Workdir) {
                    this.close_wd_menu(cx);
                }
            }))
            .child(current)
            .child(
                div()
                    .id("wd-rows")
                    .flex()
                    .flex_col()
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .p(px(4.))
                    .children(rows),
            )
            .children(confirm)
            .child(
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(LINE)
                    .child(
                        div()
                            .id("wd-all")
                            .flex()
                            .items_center()
                            .h(px(28.))
                            .px(px(10.))
                            .rounded(px(RADIUS_MD))
                            .text_size(px(12.))
                            .text_color(TEXT_SECONDARY)
                            .cursor_pointer()
                            .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT))
                            .on_click(cx.listener(|this, _, _, cx| this.open_new_chat(cx)))
                            .child("All folders, runtimes and repos…"),
                    ),
            )
    }

    fn wd_row(
        &self,
        ix: usize,
        dir: PathBuf,
        pending: Option<&PathBuf>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let is_current = dir == self.workspace.root;
        let on = pending.map_or(is_current, |p| *p == dir);
        let fact = self.wd.facts.get(&dir);
        let pv = prov_view(fact);
        let other = other_session(&self.wd.sessions, &dir, &self.session_id);
        let (note, tone) = menu_note(is_current, other.map(|e| e.title.as_str()), fact);
        let pick = dir.clone();
        div()
            .id(eid("wd-row", ix))
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .min_h(px(44.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(RADIUS_MD))
            .when(on, |d| d.bg(SURFACE_HOVER))
            .hover(|s| s.bg(SURFACE_HOVER))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.wd.pending = (pick != this.workspace.root).then(|| pick.clone());
                cx.notify();
            }))
            .child(
                div()
                    .w(px(14.))
                    .flex_none()
                    .when(is_current, |d| d.child(icon("check", 14., ACCENT_TEXT))),
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
                            .gap(px(6.))
                            .min_w_0()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(TEXT)
                                    .child(row_name(&dir, fact)),
                            )
                            .child(prov_pill(pv.label, pv.status)),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .line_height(px(16.))
                            .text_color(TEXT_MUTED)
                            .child(show_path(&dir)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(tone.color())
                    .whitespace_nowrap()
                    .child(note),
            )
            .into_any_element()
    }
}
