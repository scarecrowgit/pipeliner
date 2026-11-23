//! Page header (breadcrumb, pipeline switcher, file badge, run summary, "New pipeline",
//! "Open board") and the canvas toolbar ("Add step", zoom).

use gpui::{Context, FontWeight, IntoElement, SharedString, div, prelude::*, px};
use pipeline::{PipelineDef, StepKind};
use ui::{Badge, Button, Status, Tone, icon, theme::*};

use crate::layout::kind_look;
use crate::menu::Item;
use crate::snap::Snap;
use crate::{Menu, PipelineEditorScreen};

impl PipelineEditorScreen {
    pub(crate) fn render_header(&self, snap: &Snap, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let def = snap.def.as_ref();
        let badge = match (def, &self.error) {
            (None, _) => None,
            (Some(_), Some(_)) => Some(Badge::new("Not saved").tone(Tone::Danger)),
            (Some(d), None) => {
                let n = d.problems().len();
                Some(if n > 0 {
                    Badge::new(SharedString::from(format!(
                        "{n} {}",
                        if n == 1 { "problem" } else { "problems" }
                    )))
                    .dot(Status::Waiting)
                } else {
                    Badge::new(SharedString::from(format!("{}.toml", d.id))).dot(Status::Live)
                })
            }
        };
        let active = snap.active();
        let summary = match (snap.runs.len(), active) {
            (0, _) => "No runs yet".to_string(),
            (n, 0) => format!("{n} {}", if n == 1 { "run" } else { "runs" }),
            (n, a) => format!("{n} {} · {a} active", if n == 1 { "run" } else { "runs" }),
        };

        // Pipeline switcher.
        let switch_open = self.menu == Some(Menu::Switch);
        let current = def.map(|d| d.id.clone()).unwrap_or_default();
        let switch_items: Vec<Item> = snap
            .pipes
            .iter()
            .map(|p| {
                let id = p.id.clone();
                let trailing = match p.active {
                    0 => format!("{} {}", p.steps, if p.steps == 1 { "step" } else { "steps" }),
                    a => format!("{a} active"),
                };
                Item::new(p.name.clone(), move |this, _, cx| this.switch_pipeline(id.clone(), cx))
                    .icon("pipelines", ACCENT_SOFT, ACCENT_TEXT)
                    .trailing(trailing)
                    .checked(p.id == current)
            })
            .collect();
        let switcher = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(
                div()
                    .id("pipeline-switch")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(8.))
                    .mx(px(-8.))
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer()
                    .when(switch_open, |d| d.bg(SURFACE_HOVER))
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .when(!snap.pipes.is_empty(), |d| {
                        d.on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Switch, cx)))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(def.map(|d| d.name.clone()).unwrap_or_else(|| "No pipeline".into()))),
                    )
                    .when(snap.pipes.len() > 1, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .font_weight(FontWeight::NORMAL)
                                .text_color(TEXT_MUTED)
                                .child(SharedString::from(format!("{} pipelines", snap.pipes.len()))),
                        )
                    })
                    .when(!snap.pipes.is_empty(), |d| d.child(icon("chevron-down", 12., TEXT_MUTED))),
            )
            .when(switch_open, |d| d.child(self.popup(Menu::Switch, 320., switch_items, cx)));

        // "New pipeline" menu.
        let new_open = self.menu == Some(Menu::New);
        let mut new_items = vec![
            Item::new("Blank", |this, window, cx| this.new_pipeline(None, window, cx))
                .icon("plus", SURFACE_HOVER, TEXT_SECONDARY)
                .detail("No steps; the default pipeline's workdirs"),
        ];
        if let Some(d) = def {
            let (id, name) = (d.id.clone(), d.name.clone());
            new_items.push(
                Item::new(format!("Copy of {name}"), move |this, window, cx| {
                    this.new_pipeline(Some(id.clone()), window, cx)
                })
                .icon("replay", ACCENT_SOFT, ACCENT_TEXT)
                .detail("Same steps and workdirs, to change"),
            );
        }
        let others: Vec<_> = snap.pipes.iter().filter(|p| p.id != current).collect();
        if !others.is_empty() {
            new_items.push(Item::heading("From a template"));
            for p in others {
                let id = p.id.clone();
                new_items.push(
                    Item::new(p.name.clone(), move |this, window, cx| this.new_pipeline(Some(id.clone()), window, cx))
                        .icon("pipelines", SURFACE_HOVER, TEXT_SECONDARY)
                        .trailing(format!("{} {}", p.steps, if p.steps == 1 { "step" } else { "steps" })),
                );
            }
        }
        let new_btn = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                Button::new("new-pipeline", "New pipeline")
                    .icon("plus")
                    .selected(new_open)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::New, cx))),
            )
            .when(new_open, |d| d.child(self.popup(Menu::New, 300., new_items, cx)));

        div()
            .flex()
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
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .id("crumb-pipelines")
                            .flex_none()
                            .text_color(TEXT_MUTED)
                            .cursor_pointer()
                            .hover(|s| s.text_color(TEXT))
                            .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Agents, cx))
                            .child("Pipelines"),
                    )
                    .child(icon("chevron-right", 12., TEXT_DISABLED))
                    .child(switcher),
            )
            .children(badge)
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(SharedString::from(summary)),
            )
            .child(new_btn)
            .child(
                Button::new("open-board", "Open board")
                    .primary()
                    .icon("arrow-right")
                    .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Issues, cx)),
            )
    }

    pub(crate) fn render_toolbar(&self, def: &PipelineDef, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let zoom_pct = format!("{}%", (self.zoom * 100.).round() as i32);
        let where_to = match self.sel.as_deref().and_then(|id| def.step(id)) {
            Some(s) => format!("after {}", s.name),
            None => "at the end".into(),
        };
        let add_open = self.menu == Some(Menu::Add);
        let add_items: Vec<Item> = std::iter::once(Item::heading(format!("Add a step {where_to}")))
            .chain(StepKind::ALL.into_iter().map(|k| {
                let (ic, bg, fg) = kind_look(k, false);
                Item::new(k.label(), move |this, _, cx| this.add_step(k, cx))
                    .icon(ic, bg, fg)
                    .detail(k.about())
            }))
            .collect();
        let steps = def.steps.len();
        let wds = def.workdirs.len();
        let summary = format!(
            "{steps} {} · {wds} {} · runs left to right, top to bottom",
            if steps == 1 { "step" } else { "steps" },
            if wds == 1 { "workdir" } else { "workdirs" },
        );
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(8.))
            .border_b_1()
            .border_color(LINE)
            .bg(BG)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(
                        Button::new("add-step", "Add step")
                            .primary()
                            .small()
                            .icon("plus")
                            .selected(add_open)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(Menu::Add, cx))),
                    )
                    .when(add_open, |d| d.child(self.popup(Menu::Add, 400., add_items, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .pl(px(4.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(SharedString::from(summary)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(2.))
                    .child(
                        Button::new("zoom-out", "−")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.step_zoom(-0.1, cx))),
                    )
                    .child(
                        div()
                            .min_w(px(44.))
                            .flex()
                            .justify_center()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_SECONDARY)
                            .child(zoom_pct),
                    )
                    .child(
                        Button::new("zoom-in", "+")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.step_zoom(0.1, cx))),
                    )
                    .child(
                        Button::new("fit", "Fit")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
                    ),
            )
    }
}
