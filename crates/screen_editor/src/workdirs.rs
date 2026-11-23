//! Workdirs tab: the pipeline's folders. Each is a card; the expanded one edits its name,
//! folder, worktree mode, base branch, branch prefix, setup command and environment.

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};
use pipeline::{PipelineDef, WorkdirDef};
use ui::{Badge, Button, ButtonVariant, Tone, icon, theme::*};

use crate::ops::steps_using;
use crate::widgets::*;
use crate::{Confirm, Field, PipelineEditorScreen};

impl PipelineEditorScreen {
    pub(crate) fn ins_workdirs(&self, def: &PipelineDef, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = vec![
            section(10.)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(strong("Workdirs"))
                        .child(
                            Button::new("add-workdir", "Add workdir")
                                .small()
                                .icon("plus")
                                .on_click(cx.listener(|this, _, _, cx| this.add_workdir(cx))),
                        ),
                )
                .child(help(
                    "Folders the steps work in. With “New worktree per run” each run gets its own branch and worktree of a git repository; otherwise the folder is used as is.",
                ))
                .into_any_element(),
        ];
        if def.workdirs.is_empty() {
            out.push(
                section(4.)
                    .border_b_0()
                    .child(help("No workdirs yet. Add one to give agents, commands and merges a folder to work in."))
                    .into_any_element(),
            );
        }
        for (i, w) in def.workdirs.iter().enumerate() {
            let open = self.wd_sel.as_deref() == Some(w.id.as_str());
            out.push(self.workdir_card(i, def, w, open, window, cx));
        }
        out
    }

    fn workdir_card(
        &self,
        i: usize,
        def: &PipelineDef,
        w: &WorkdirDef,
        open: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let users = steps_using(def, &w.id);
        let used_by = match users.len() {
            0 => "not used".to_string(),
            1 => format!("used by {}", users[0].name),
            n => format!("used by {n} steps"),
        };
        let path = if w.path.as_os_str().is_empty() {
            "no folder".to_string()
        } else {
            pipeline::def::tilde(&w.path)
        };
        let missing = w.path.as_os_str().is_empty() || !w.path.is_dir();
        let id = w.id.clone();
        let head = div()
            .id(("wd-head", i))
            .flex()
            .items_center()
            .gap(px(10.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                let next = if this.wd_sel.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) };
                this.expand_workdir(next, cx);
            }))
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(SURFACE_HOVER)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(if w.worktree { "git-branch" } else { "folder" }, 14., TEXT_SECONDARY)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(SharedString::from(w.name.clone())))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(FONT_MONO)
                                    .text_size(px(11.))
                                    .text_color(TEXT_DISABLED)
                                    .child(SharedString::from(w.id.clone())),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(if missing { WARNING } else { TEXT_MUTED })
                            .child(SharedString::from(path.clone())),
                    ),
            )
            .child(icon(if open { "chevron-down" } else { "chevron-right" }, 12., TEXT_MUTED));

        let mut card = section(12.).child(head).child(
            div()
                .flex()
                .gap(px(6.))
                .child(Badge::new(if w.worktree { "worktree per run" } else { "folder as is" }).tone(if w.worktree {
                    Tone::Accent
                } else {
                    Tone::Neutral
                }))
                .child(Badge::new(SharedString::from(used_by))),
        );
        if !open {
            return card.into_any_element();
        }

        let pick_id = w.id.clone();
        let toggle_id = w.id.clone();
        let env_rows: Vec<AnyElement> = w
            .env
            .iter()
            .enumerate()
            .map(|(j, (k, v))| {
                let (wid, key) = (w.id.clone(), k.clone());
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .px(px(8.))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE_RAISED)
                    .font_family(FONT_MONO)
                    .text_size(px(12.))
                    .child(div().flex_none().text_color(ACCENT_TEXT).child(SharedString::from(k.clone())))
                    .child(div().flex_none().text_color(TEXT_MUTED).child("="))
                    .child(div().flex_1().min_w_0().truncate().text_color(TEXT_SECONDARY).child(SharedString::from(v.clone())))
                    .child(
                        div()
                            .id(("env-x", j))
                            .size(px(18.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(RADIUS_SM))
                            .cursor_pointer()
                            .hover(|s| s.bg(SURFACE_HOVER))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let key = key.clone();
                                this.edit_workdir(&wid, cx, |w| {
                                    w.env.remove(&key);
                                })
                            }))
                            .child(icon("x", 10., TEXT_MUTED)),
                    )
                    .into_any_element()
            })
            .collect();

        card = card
            .child(div().child(field_label("Name")).child(self.text_field(Field::WdName, false, window, cx)))
            .child(
                div().child(field_label("Folder")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon("folder", 14., TEXT_MUTED))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(if missing { WARNING } else { TEXT_SECONDARY })
                                .child(SharedString::from(path)),
                        )
                        .child(
                            Button::new(("wd-pick", i), "Choose…")
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| this.pick_folder(pick_id.clone(), cx))),
                        ),
                ),
            )
            .child(self.check_row(
                "wd-worktree",
                w.worktree,
                "New worktree per run",
                Some("Each run works on its own branch; merges and pull requests start from it."),
                move |this, cx| {
                    let id = toggle_id.clone();
                    this.edit_workdir(&id, cx, |w| w.worktree = !w.worktree)
                },
                cx,
            ))
            .child(div().when(!w.worktree, |d| d.opacity(0.5)).child(two_cols(
                div().child(field_label("Base branch")).child(self.text_field(Field::WdBase, true, window, cx)),
                div().child(field_label("Branch prefix")).child(self.text_field(Field::WdPrefix, true, window, cx)),
            )))
            .child(
                div()
                    .child(field_label("Setup command"))
                    .child(self.text_field(Field::WdSetup, true, window, cx))
                    .child(help("Runs once in a new worktree before any step uses it (install, warm caches).").mt(px(6.))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(field_label("Environment").mb_0())
                    .children(env_rows)
                    .child(self.text_field(Field::EnvAdd, true, window, cx))
                    .child(help("For commands and agents' bash in this workdir.")),
            );

        let confirming = self.confirm == Some(Confirm::Workdir(w.id.clone()));
        if confirming {
            let names: Vec<&str> = users.iter().map(|s| s.name.as_str()).collect();
            let text = if names.is_empty() {
                format!("Delete the workdir “{}”? The folder itself is not touched.", w.name)
            } else {
                format!(
                    "{} {} “{}”. After deleting it they have no workdir until you pick another. The folder itself is not touched.",
                    names.join(", "),
                    if names.len() == 1 { "works in" } else { "work in" },
                    w.name
                )
            };
            let id = w.id.clone();
            card = card.child(self.confirm_strip(
                "confirm-workdir",
                text,
                "Delete workdir",
                move |this, cx| this.delete_workdir(id.clone(), cx),
                cx,
            ));
        } else {
            let id = w.id.clone();
            card = card.child(
                div().flex().child(
                    Button::new(("wd-delete", i), "Delete workdir…")
                        .small()
                        .variant(ButtonVariant::Danger)
                        .on_click(cx.listener(move |this, _, _, cx| this.ask_confirm(Confirm::Workdir(id.clone()), cx))),
                ),
            );
        }
        card.into_any_element()
    }
}
