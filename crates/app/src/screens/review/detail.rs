//! The main panel: what the selected item is, what led to it, and how to decide.

use std::sync::Arc;

use gpui::{img, Animation, AnimationExt as _, ObjectFit, StyledImage as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::Sizable as _;

use super::data::{self, Item, ItemKind, Pill, PillState, RunInfo, VariantView};
use super::ReviewScreen;
use crate::screens::prelude::*;
use crate::ui::select::select;
use crate::ui::{DiffFile, DiffView, ParsedDiff};

/// Height of the rendered-screen comparison area.
const IMAGE_H: f32 = 380.;

impl ReviewScreen {
    pub(super) fn render_detail(
        &self,
        item: &Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = item.task.id.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .gap(theme::S4)
            .p(theme::S6)
            .max_w(px(1180.));
        col = col
            .child(self.breadcrumb(item))
            .child(self.title_row(item, cx));
        if let Some(run) = &item.run {
            col = col.child(input_summary(run));
            col = col.child(chain(run));
        }
        if let Some(err) = &item.last_failure {
            col = col.child(last_failure(item.kind, err));
        }
        match item.kind {
            ItemKind::Pick => {
                col = col.child(self.variant_cards(item, cx));
                if item.has_images {
                    col = col.child(self.image_compare(item, cx));
                }
                col = col.child(self.summary_diff(item, window, cx));
                col = col.child(
                    div()
                        .flex()
                        .gap(theme::S4)
                        .items_start()
                        .child(self.note_panel(item, cx))
                        .when_some(item.run.as_ref(), |d, r| d.child(self.after_panel(item, r))),
                );
            }
            ItemKind::ApprovePr => {
                col = col.child(self.pr_card(item, cx));
                col = col.child(self.summary_diff(item, window, cx));
                col = col.child(
                    div()
                        .flex()
                        .gap(theme::S4)
                        .items_start()
                        .child(self.note_panel(item, cx))
                        .when_some(item.run.as_ref(), |d, r| d.child(self.after_panel(item, r))),
                );
            }
            ItemKind::Plan => {
                col = col
                    .child(self.plan_card(item, cx))
                    .child(self.note_panel(item, cx));
            }
            ItemKind::Other => {
                if !item.task.summary.is_empty() {
                    col = col.child(card().child(markdown_of(
                        format!("review-task-{id}"),
                        &item.task.summary,
                        window,
                        cx,
                    )));
                }
                col = col.child(self.note_panel(item, cx));
            }
        }
        div()
            .id(SharedString::from(format!("review-detail-{id}")))
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .child(col)
            .with_animation(
                SharedString::from(format!("review-panel-in-{id}")),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t),
            )
    }

    /// `Run #128 · Bug triage v14 › Jira › Router › You`.
    fn breadcrumb(&self, item: &Item) -> impl IntoElement {
        let sep =
            || ui::icon_sized(AppIcon::ChevronRight, px(12.)).text_color(theme::TEXT_DISABLED);
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .text_label()
            .fg_muted();
        match &item.run {
            Some(r) => {
                let head = if r.pipeline.is_empty() {
                    format!("Run #{}", r.number)
                } else {
                    format!("Run #{} · {} v{}", r.number, r.pipeline, r.version)
                };
                row = row.child(div().child(head));
                for c in &r.crumbs {
                    row = row.child(sep()).child(div().child(c.clone()));
                }
            }
            None => row = row.child(div().child(item.task.kind.label())),
        }
        row.child(sep()).child(
            div()
                .fg_accent()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child("You"),
        )
    }

    /// Title, reference chip, status and links to the chat and the thread.
    fn title_row(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &item.task;
        let (thread_id, chat) = (
            t.id.clone(),
            item.run
                .as_ref()
                .and_then(|r| r.chat_id.clone())
                .or_else(|| item.chat_id.clone()),
        );
        div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(theme::S3)
            .child(div().text_title().fg().child(t.title.clone()))
            .when_some(
                item.run.as_ref().and_then(|r| r.external_ref.clone()),
                |d, r| d.child(Badge::new(r).mono()),
            )
            .child(
                StatusPill::new(t.status)
                    .id("review-status")
                    .suffix(ui::fmt_ago(t.created_at)),
            )
            .child(div().flex_1())
            .when_some(chat, |d, chat| {
                d.child(
                    Button::new("review-open-chat", "Chat")
                        .ghost()
                        .small()
                        .icon(AppIcon::Chat)
                        .on_click(move |_, _, cx| {
                            state::navigate(
                                Route::Chat {
                                    chat: Some(chat.clone()),
                                },
                                cx,
                            )
                        }),
                )
            })
            .child(
                Button::new("review-open-thread", "Open thread")
                    .ghost()
                    .small()
                    .icon_right(AppIcon::ChevronRight)
                    .on_click(
                        cx.listener(move |_, _, _, cx| state::open_task(thread_id.clone(), cx)),
                    ),
            )
    }

    // ---- Pick gate -----------------------------------------------------------------

    /// Variant cards side by side with Pick buttons.
    fn variant_cards(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        if item.variants.is_empty() {
            return EmptyState::new(
                AppIcon::Inbox,
                "No variants to choose from",
                "The step before this gate produced no output.",
            )
            .compact()
            .id("review-no-variants")
            .into_any_element();
        }
        let picked = self.pick_of(item);
        let viewing = self.viewing_of(item);
        let multi = item.variants.len() > 1;
        div()
            .flex()
            .flex_wrap()
            .gap(theme::S4)
            .children(item.variants.iter().enumerate().map(|(pos, v)| {
                let is_picked = picked == Some(v.v.index) && multi;
                let is_viewing = viewing == Some(pos);
                let letter = harness_core::variant_letter(v.v.index);
                let (task_id, index) = (item.task.id.clone(), v.v.index);
                let (view_id, pick_id) = (task_id.clone(), task_id.clone());
                let files = v.files.len();
                div()
                    .id(SharedString::from(format!("review-variant-{}", v.v.id)))
                    .relative()
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w(px(260.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .p(px(14.))
                    .bg(theme::SURFACE)
                    .border_1()
                    .border_color(if is_viewing && multi {
                        theme::LINE_STRONG
                    } else {
                        theme::LINE
                    })
                    .rounded(theme::RADIUS_XL)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.viewing.insert(view_id.clone(), index);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(VariantChip::variant(v.v.index).selected(is_picked))
                            .child(
                                div()
                                    .text_body_strong()
                                    .fg()
                                    .child(format!("Variant {letter}")),
                            )
                            .when(!v.v.model.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_label()
                                        .fg_muted()
                                        .ellipsis()
                                        .child(Config::short_model(&v.v.model)),
                                )
                            })
                            .child(div().flex_1())
                            .when(multi && pos < 9, |d| d.child(kbd((pos + 1).to_string()))),
                    )
                    .child(
                        div()
                            .text_body_strong()
                            .fg()
                            .clamp_lines(3)
                            .child(v.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .when(!v.v.diff.is_empty(), |d| {
                                d.child(ui::key_value(
                                    "Diff",
                                    div()
                                        .flex()
                                        .gap(theme::S2)
                                        .text_mono()
                                        .child(
                                            div()
                                                .text_color(theme::DIFF_ADD_TEXT)
                                                .child(format!("+{}", v.added)),
                                        )
                                        .child(
                                            div()
                                                .text_color(theme::DIFF_DEL_TEXT)
                                                .child(format!("-{}", v.removed)),
                                        )
                                        .child(div().fg_muted().child(format!(
                                            "{files} file{}",
                                            if files == 1 { "" } else { "s" }
                                        ))),
                                ))
                            })
                            .when(v.v.cost_usd > 0.0, |d| {
                                d.child(ui::key_value(
                                    "Cost",
                                    div().text_mono().child(ui::fmt_usd(v.v.cost_usd)),
                                ))
                            })
                            .when(v.v.duration_secs >= 1.0, |d| {
                                d.child(ui::key_value(
                                    "Time",
                                    div()
                                        .text_mono()
                                        .child(ui::fmt_duration(v.v.duration_secs as i64)),
                                ))
                            })
                            .when_some(v.v.branch.clone().filter(|b| !b.is_empty()), |d, b| {
                                d.child(ui::key_value(
                                    "Branch",
                                    div().text_mono().ellipsis().child(b),
                                ))
                            }),
                    )
                    .when(!v.body.is_empty(), |d| {
                        // A plain-text teaser; the full Markdown is in the Summary tab below.
                        d.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .overflow_hidden()
                                .text_label()
                                .fg_muted()
                                .clamp_lines(4)
                                .child(plain_teaser(&v.body)),
                        )
                    })
                    .child(div().flex_1())
                    .when(multi, |d| {
                        d.child(
                            Button::new(
                                SharedString::from(format!("review-pick-{}", v.v.id)),
                                if is_picked {
                                    format!("Picked · ships with Ship {letter}")
                                } else {
                                    format!("Pick {letter}")
                                },
                            )
                            .secondary()
                            .full_width()
                            .selected(is_picked)
                            .icon(if is_picked {
                                AppIcon::Check
                            } else {
                                AppIcon::Plus
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.pick(pick_id.clone(), index, cx),
                            )),
                        )
                    })
                    // The picked ring fades in over the card.
                    .when(is_picked, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(theme::RADIUS_XL)
                                .border_1()
                                .border_color(theme::ACCENT_RING)
                                .shadow(vec![gpui::BoxShadow {
                                    color: theme::ACCENT_GLOW,
                                    offset: gpui::point(px(0.), px(0.)),
                                    blur_radius: px(0.),
                                    spread_radius: px(3.),
                                }])
                                .with_animation(
                                    SharedString::from(format!("review-ring-{}", v.v.id)),
                                    Animation::new(theme::DUR_STATE).with_easing(theme::ease_out),
                                    |d, t| d.opacity(t),
                                ),
                        )
                    })
            }))
            .into_any_element()
    }

    /// Large comparison of rendered screens: a tab per file, one image per variant.
    fn image_compare(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let names = data::image_names(&item.variants);
        let tab = self.image_tab.min(names.len().saturating_sub(1));
        let name = names.get(tab).cloned().unwrap_or_default();
        let mut tabs = Tabs::new("review-image-tabs").px(theme::S4).selected(tab);
        for n in &names {
            tabs = tabs.tab(n.clone());
        }
        let tabs = tabs.on_select(cx.listener(|this, ix: &usize, _, cx| {
            this.image_tab = *ix;
            cx.notify();
        }));
        panel()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(theme::S4)
                    .pt(theme::S3)
                    .gap(theme::S2)
                    .child(
                        SectionHeader::new("Rendered screens")
                            .icon(AppIcon::Sparkles)
                            .meta("click an image to open it full size"),
                    ),
            )
            .child(tabs)
            .child(
                div()
                    .flex()
                    .gap(theme::S4)
                    .p(theme::S4)
                    .children(item.variants.iter().map(|v| {
                        let path =
                            v.v.images
                                .iter()
                                .find(|p| data::image_name(p) == name)
                                .cloned();
                        let letter = harness_core::variant_letter(v.v.index);
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme::S2)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme::S2)
                                    .child(VariantChip::variant(v.v.index).size(px(20.)))
                                    .child(
                                        div()
                                            .text_label()
                                            .fg_secondary()
                                            .ellipsis()
                                            .child(v.title.clone()),
                                    ),
                            )
                            .child(match path {
                                Some(p) => {
                                    let open = p.clone();
                                    div()
                                        .id(SharedString::from(format!(
                                            "review-img-{}-{tab}",
                                            v.v.id
                                        )))
                                        .h(px(IMAGE_H))
                                        .w_full()
                                        .rounded(theme::RADIUS_LG)
                                        .border_1()
                                        .border_color(theme::LINE)
                                        .bg(theme::BG)
                                        .overflow_hidden()
                                        .cursor_pointer()
                                        .hover(|s| s.border_color(theme::LINE_STRONG))
                                        .child(
                                            img(std::path::PathBuf::from(p))
                                                .w_full()
                                                .h_full()
                                                .object_fit(ObjectFit::Contain),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_zoom(open.clone(), cx)
                                        }))
                                        .into_any_element()
                                }
                                None => div()
                                    .h(px(IMAGE_H))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::RADIUS_LG)
                                    .border_1()
                                    .border_color(theme::LINE)
                                    .text_label()
                                    .fg_muted()
                                    .child(format!("Variant {letter} has no render of this screen"))
                                    .into_any_element(),
                            })
                    }))
                    .into_any_element(),
            )
    }

    // ---- Approve PR ------------------------------------------------------------------

    /// The pull request: link, stats and the three outcomes.
    fn pr_card(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let v = item.variants.first();
        let url = item.pr_url.clone();
        card()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .child(ui::icon_tile(
                        AppIcon::GitPullRequest,
                        theme::ACCENT_SOFT,
                        theme::ACCENT_TEXT,
                    ))
                    .child(match url {
                        Some(u) => {
                            let open = u.clone();
                            div()
                                .id("review-pr-link")
                                .flex_1()
                                .min_w_0()
                                .ellipsis()
                                .text_mono()
                                .fg_accent()
                                .cursor_pointer()
                                .hover(|s| s.underline())
                                .child(u)
                                .on_click(move |_, _, cx| cx.open_url(&open))
                                .into_any_element()
                        }
                        None => div()
                            .flex_1()
                            .text_body()
                            .fg_muted()
                            .child("This run has no pull request yet.")
                            .into_any_element(),
                    })
                    .when_some(v, |d, v| {
                        d.when(!v.v.diff.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .gap(theme::S2)
                                    .text_mono()
                                    .child(
                                        div()
                                            .text_color(theme::DIFF_ADD_TEXT)
                                            .child(format!("+{}", v.added)),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme::DIFF_DEL_TEXT)
                                            .child(format!("-{}", v.removed)),
                                    )
                                    .child(
                                        div().fg_muted().child(format!("{} files", v.files.len())),
                                    ),
                            )
                        })
                        .when(v.v.cost_usd > 0.0, |d| {
                            d.child(
                                div()
                                    .text_mono()
                                    .fg_muted()
                                    .child(ui::fmt_usd(v.v.cost_usd)),
                            )
                        })
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(theme::S2)
                    .child(
                        Button::new("review-approve-pr", "Approve & merge")
                            .primary()
                            .icon(AppIcon::Check)
                            .on_click(cx.listener(|this, _, window, cx| this.ship(window, cx))),
                    )
                    .child(
                        Button::new("review-reject-pr", "Reject (keep PR open)")
                            .danger()
                            .on_click(cx.listener(|this, _, window, cx| this.reject(window, cx))),
                    ),
            )
    }

    // ---- Summary / Diff ----------------------------------------------------------------

    /// Tabs over the open variant's summary (markdown) and diff (with Viewed checkboxes).
    fn summary_diff(
        &self,
        item: &Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(vv) = self.viewing_of(item).and_then(|ix| item.variants.get(ix)) else {
            return div().into_any_element();
        };
        let letter = harness_core::variant_letter(vv.v.index);
        let files = vv.files.len();
        let tab = self.detail_tab;
        let meta = if vv.diff.is_empty() {
            format!("Variant {letter}")
        } else {
            format!(
                "Variant {letter} · {files} file{} · +{} -{}",
                if files == 1 { "" } else { "s" },
                vv.added,
                vv.removed
            )
        };
        let body = if tab == 0 {
            if vv.v.summary.trim().is_empty() {
                div()
                    .p(theme::S4)
                    .text_label()
                    .fg_muted()
                    .child("This variant has no summary.")
                    .into_any_element()
            } else {
                div()
                    .p(theme::S4)
                    .child(markdown_of(
                        format!("review-summary-{}", vv.v.id),
                        &vv.v.summary,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
        } else if vv.diff.is_empty() {
            div()
                .p(theme::S4)
                .text_label()
                .fg_muted()
                .child("This variant has no diff.")
                .into_any_element()
        } else {
            self.diff_files(vv, cx).into_any_element()
        };
        panel()
            .child(
                div()
                    .flex()
                    .items_end()
                    .child(
                        Tabs::new("review-detail-tabs")
                            .px(theme::S4)
                            .tab("Summary")
                            .tab_count("Diff", files)
                            .selected(tab)
                            .on_select(cx.listener(|this, ix: &usize, _, cx| {
                                this.detail_tab = *ix;
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h(theme::TAB_H)
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr(theme::S4)
                            .border_b_1()
                            .border_color(theme::LINE)
                            .text_label()
                            .fg_muted()
                            .child(meta),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// The diff, one block per file. A file marked Viewed collapses to its header; files
    /// past the first few hundred lines (and very big ones) start folded behind a
    /// "Show diff" row, so a large PR doesn't redraw thousands of rows every frame.
    fn diff_files(&self, vv: &VariantView, cx: &mut Context<Self>) -> impl IntoElement {
        let sizes: Vec<usize> = vv.files.iter().map(|f| f.files[0].lines.len()).collect();
        let auto = data::open_by_default(&sizes);
        div()
            .flex()
            .flex_col()
            .children(vv.files.iter().enumerate().map(|(ix, f)| {
                let file = &f.files[0];
                let key = (vv.v.id.clone(), file.path.clone());
                let viewed = self.viewed.contains(&key);
                let open = !viewed && (auto[ix] || self.opened.contains(&key));
                let folded = !viewed && !open;
                let shown: Arc<ParsedDiff> = if open {
                    f.clone()
                } else {
                    Arc::new(ParsedDiff {
                        files: vec![DiffFile {
                            path: file.path.clone(),
                            status: file.status,
                            added: file.added,
                            removed: file.removed,
                            lines: vec![],
                        }],
                    })
                };
                let open_key = key.clone();
                div()
                    .relative()
                    .child(DiffView::new(shown).max_lines(1500))
                    .when(folded, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::S3)
                                .px(theme::S4)
                                .py(theme::S2)
                                .border_b_1()
                                .border_color(theme::LINE)
                                .text_label()
                                .fg_muted()
                                .child(format!("{} lines folded", file.lines.len()))
                                .child(
                                    Button::new(
                                        SharedString::from(format!(
                                            "review-open-file-{}-{ix}",
                                            vv.v.id
                                        )),
                                        "Show diff",
                                    )
                                    .ghost()
                                    .small()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.opened.insert(open_key.clone());
                                        cx.notify();
                                    })),
                                ),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right(theme::S4)
                            .h(px(40.))
                            .flex()
                            .items_center()
                            .child(
                                Checkbox::new(SharedString::from(format!(
                                    "review-viewed-{}-{ix}",
                                    vv.v.id
                                )))
                                .label("Viewed")
                                .small()
                                .checked(viewed)
                                .on_click(cx.listener(
                                    move |this, checked: &bool, _, cx| {
                                        if *checked {
                                            this.viewed.insert(key.clone());
                                        } else {
                                            this.viewed.remove(&key);
                                        }
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
            }))
    }

    // ---- Plan approval -----------------------------------------------------------------

    fn plan_card(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = item.chat_id.clone();
        panel()
            .child(
                div().p(theme::S4).child(
                    SectionHeader::new("Proposed plan")
                        .icon(AppIcon::Sparkles)
                        .meta(if item.plan_steps.is_empty() {
                            String::new()
                        } else {
                            format!("{} steps", item.plan_steps.len())
                        }),
                ),
            )
            .child(divider())
            .child(if item.plan_steps.is_empty() {
                div()
                    .p(theme::S4)
                    .text_label()
                    .fg_muted()
                    .child("The plan text is in the chat.")
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .children(item.plan_steps.iter().enumerate().map(|(i, s)| {
                        div()
                            .flex()
                            .items_start()
                            .gap(theme::S3)
                            .px(theme::S4)
                            .py(theme::S3)
                            .when(i > 0, |d| d.border_t_1().border_color(theme::LINE))
                            .child(step_number(i + 1))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_body()
                                    .fg_secondary()
                                    .child(s.clone()),
                            )
                    }))
                    .into_any_element()
            })
            .child(divider())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .p(theme::S4)
                    .child(
                        Button::new("review-approve-plan", "Approve plan")
                            .primary()
                            .icon(AppIcon::Check)
                            .on_click(cx.listener(|this, _, window, cx| this.ship(window, cx))),
                    )
                    .child(
                        Button::new("review-reject-plan", "Reject")
                            .danger()
                            .on_click(cx.listener(|this, _, window, cx| this.reject(window, cx))),
                    )
                    .child(div().flex_1())
                    .when_some(chat, |d, chat| {
                        d.child(
                            Button::new("review-plan-chat", "Open the chat")
                                .ghost()
                                .icon_right(AppIcon::ChevronRight)
                                .on_click(move |_, _, cx| {
                                    state::navigate(
                                        Route::Chat {
                                            chat: Some(chat.clone()),
                                        },
                                        cx,
                                    )
                                }),
                        )
                    }),
            )
    }

    // ---- Send back / feedback ----------------------------------------------------------

    /// The note box: "Send back with a note" for gates, feedback for plans and questions.
    fn note_panel(&self, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let gate = matches!(item.kind, ItemKind::Pick | ItemKind::ApprovePr);
        let (title, meta, button) = if gate {
            (
                "Send back with a note",
                "becomes prompt layer 6 on the next try",
                "Send back",
            )
        } else if item.kind == ItemKind::Plan {
            (
                "Feedback on the plan",
                "the orchestrator revises and proposes again",
                "Send feedback",
            )
        } else {
            ("Reply", "sent to the waiting step", "Send reply")
        };
        let reasons = self
            .data
            .as_ref()
            .map(|d| d.reasons.as_slice())
            .unwrap_or(&[]);
        let has_target = gate
            && item
                .run
                .as_ref()
                .is_some_and(|r| !r.back_options.is_empty());
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme::S3)
            .p(theme::S4)
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::LINE)
            .rounded(theme::RADIUS_XL)
            .child(
                SectionHeader::new(title)
                    .icon(AppIcon::RotateCcw)
                    .meta(meta),
            )
            .child(ui::input::composer_field(&self.note))
            .when(gate && !reasons.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(theme::S2)
                        .child(div().text_label().fg_muted().child("Common reasons"))
                        .children(reasons.iter().enumerate().map(|(ix, (text, n))| {
                            let text = text.clone();
                            div()
                                .id(SharedString::from(format!("review-reason-{ix}")))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(26.))
                                .px(theme::S3)
                                .rounded(theme::RADIUS_FULL)
                                .border_1()
                                .border_color(theme::LINE_STRONG)
                                .text_label()
                                .fg_secondary()
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::SURFACE_HOVER))
                                .child(data::truncate(&text, 40))
                                .child(div().fg_muted().child(n.to_string()))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let text = text.clone();
                                    this.note.update(cx, |s, cx| s.set_value(text, window, cx));
                                    this.focus_note(window, cx);
                                }))
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .when(has_target, |d| {
                        d.child(div().text_label().fg_muted().child("Send to"))
                            .child(div().w(px(200.)).child(select(&self.target)))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("review-send-note", button)
                            .secondary()
                            .icon(AppIcon::Send)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.submit_note(window, cx)),
                            ),
                    ),
            )
    }

    // ---- After you ship ------------------------------------------------------------------

    /// The nodes that run after this gate, from the run's graph.
    fn after_panel(&self, item: &Item, run: &RunInfo) -> impl IntoElement {
        let title = match (item.kind, self.pick_of(item)) {
            (ItemKind::Pick, Some(i)) if item.variants.len() > 1 => {
                format!("After you ship {}", harness_core::variant_letter(i))
            }
            (ItemKind::Pick, _) if item.variants.len() > 1 => "After you ship".to_string(),
            _ => "After you approve".to_string(),
        };
        div()
            .w(px(380.))
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme::S3)
            .p(theme::S4)
            .bg(theme::SURFACE)
            .border_1()
            .border_color(theme::LINE)
            .rounded(theme::RADIUS_XL)
            .child(SectionHeader::new(title).icon(AppIcon::GitFork))
            .when(run.next.is_empty(), |d| {
                d.child(
                    div()
                        .text_label()
                        .fg_muted()
                        .child("This is the last step of the pipeline."),
                )
            })
            .children(run.next.iter().enumerate().map(|(i, n)| {
                div()
                    .flex()
                    .items_start()
                    .gap(theme::S3)
                    .when(i > 0, |d| {
                        d.pt(theme::S3).border_t_1().border_color(theme::LINE)
                    })
                    .child(step_number(i + 1))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_baseline()
                                    .gap(theme::S2)
                                    .child(div().text_body_strong().fg().child(n.name.clone()))
                                    .child(div().text_label().fg_muted().child(
                                        if n.model.is_empty() {
                                            n.kind.to_string()
                                        } else {
                                            format!("{} · {}", n.kind, n.model)
                                        },
                                    )),
                            )
                            .when(!n.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_label()
                                        .fg_muted()
                                        .clamp_lines(2)
                                        .child(n.detail.clone()),
                                )
                            }),
                    )
            }))
    }
}

/// Markdown flattened to one wrapping paragraph: markers dropped, lines joined.
/// "Your last approve didn't go through": the previous attempt's error and what to do.
fn last_failure(kind: ItemKind, err: &str) -> impl IntoElement {
    let hint = match kind {
        ItemKind::ApprovePr => {
            "The PR is still open. Resolve it on GitHub (or send it back) and approve again."
        }
        _ => "This step is asking again.",
    };
    div()
        .flex()
        .flex_col()
        .gap(theme::S2)
        .p(theme::S4)
        .rounded(theme::RADIUS_XL)
        .border_1()
        .border_color(theme::alpha(theme::DANGER, 0.35))
        .bg(theme::DANGER_SOFT)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .text_body_strong()
                .text_color(theme::DANGER)
                .child(ui::icon_sized(AppIcon::Alert, theme::ICON))
                .child("Your last decision didn't go through"),
        )
        .child(
            div()
                .text_mono()
                .fg_secondary()
                .child(data::truncate(err.trim(), 600)),
        )
        .child(div().text_label().fg_muted().child(hint))
}

fn plain_teaser(md: &str) -> String {
    md.lines()
        .map(|l| l.trim().trim_start_matches(['#', '-', '*', '>']).trim())
        .filter(|l| !l.is_empty())
        .map(|l| clip_words(&l.replace("**", "").replace('`', ""), TEASER_WORD_MAX))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Longest word a teaser shows whole. Text only wraps at spaces, so one longer word (a
/// path, URL or hash) made the clamped teaser render as nothing.
const TEASER_WORD_MAX: usize = 32;

fn clip_words(s: &str, max: usize) -> String {
    s.split(' ')
        .map(|w| {
            if w.chars().count() > max {
                let head: String = w.chars().take(max - 1).collect();
                format!("{head}…")
            } else {
                w.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}


/// A markdown block with a stable id.
fn markdown_of(id: String, text: &str, window: &mut Window, cx: &mut App) -> impl IntoElement {
    ui::markdown(SharedString::from(id), text.to_string(), window, cx)
}

/// A 22px numbered circle ("1", "2").
fn step_number(n: usize) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(theme::RADIUS_FULL)
        .border_1()
        .border_color(theme::LINE_STRONG)
        .text_mono_sm()
        .fg_muted()
        .child(n.to_string())
}

/// Upstream step outputs: the input first, then one row per finished step.
fn input_summary(run: &RunInfo) -> impl IntoElement {
    let input = data::first_line(&run.input);
    let row = |name: String, text: String, cost: Option<f64>, first: bool| {
        div()
            .flex()
            .items_baseline()
            .gap(theme::S4)
            .px(theme::S4)
            .py(px(10.))
            .when(!first, |d| d.border_t_1().border_color(theme::LINE))
            .child(
                div()
                    .w(px(110.))
                    .flex_none()
                    .text_label()
                    .fg_muted()
                    .ellipsis()
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_body()
                    .fg_secondary()
                    .ellipsis()
                    .child(text),
            )
            .when_some(cost, |d, c| {
                d.child(
                    div()
                        .flex_none()
                        .text_mono()
                        .fg_muted()
                        .child(ui::fmt_usd(c)),
                )
            })
    };
    let mut rows: Vec<Div> = vec![];
    if !input.is_empty() {
        rows.push(row("Input".into(), input, None, true));
    }
    for r in &run.rows {
        let first = rows.is_empty();
        rows.push(row(r.name.clone(), r.summary.clone(), r.cost, first));
    }
    if rows.is_empty() {
        return div().into_any_element();
    }
    panel().children(rows).into_any_element()
}

/// The step chain: finished steps with costs, the gate (you), what comes next.
fn chain(run: &RunInfo) -> impl IntoElement {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme::S2)
        .children(run.chain.iter().enumerate().flat_map(|(i, p)| {
            let mut els: Vec<AnyElement> = vec![];
            if i > 0 {
                els.push(
                    ui::icon_sized(AppIcon::ChevronRight, px(12.))
                        .text_color(theme::TEXT_DISABLED)
                        .into_any_element(),
                );
            }
            els.push(pill(p).into_any_element());
            els
        }))
        .child(div().flex_1())
        .child(
            div()
                .text_label()
                .fg_muted()
                .child(format!("run {}", ui::fmt_usd(run.cost))),
        )
}

fn pill(p: &Pill) -> Div {
    let (border, bg, fg) = match p.state {
        PillState::Done => (theme::LINE_STRONG, theme::SURFACE, theme::TEXT_SECONDARY),
        PillState::Current => (theme::WARNING, theme::WARNING_SOFT, theme::WARNING),
        PillState::Pending => (theme::LINE, theme::TRANSPARENT, theme::TEXT_MUTED),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .h(px(28.))
        .px(theme::S3)
        .rounded(theme::RADIUS_MD)
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_label()
        .when(p.state == PillState::Done, |d| {
            d.child(ui::icon_sized(AppIcon::Check, px(12.)).text_color(theme::SUCCESS))
        })
        .when(p.state == PillState::Current, |d| {
            d.child(ui::dot_el(theme::WARNING, px(6.)))
        })
        .child(p.name.clone())
        .when(!p.detail.is_empty(), |d| {
            d.child(div().fg_muted().child(p.detail.clone()))
        })
}

#[cfg(test)]
mod teaser_tests {
    use super::*;

    #[test]
    fn long_words_are_clipped() {
        let t = plain_teaser(&format!("**Swimlanes** see {}", "a".repeat(100)));
        assert!(t.starts_with("Swimlanes see aaaa"));
        assert!(t.split(' ').all(|w| w.chars().count() <= TEASER_WORD_MAX));
        assert!(t.ends_with('…'));
    }
}
