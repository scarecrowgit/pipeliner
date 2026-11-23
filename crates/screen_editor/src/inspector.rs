//! Right-hand inspector: tabs for the selected step (`step_fields`), the pipeline itself
//! (name, description, steps, delete) and its workdirs (`workdirs`), plus the shared
//! controls (text fields, multi-line boxes, check rows, confirm strips).

use gpui::{
    AnyElement, Context, ElementId, Focusable, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px,
};
use pipeline::PipelineDef;
use ui::{Button, ButtonVariant, TabItem, Tabs, checkbox, field_box, icon, theme::*};

use crate::layout::{kind_look, kind_line};
use crate::snap::{Snap, model_short};
use crate::widgets::*;
use crate::{Confirm, Field, PipelineEditorScreen, Tab};

/// Inspector width.
pub(crate) const INSPECTOR_W: f32 = 340.;
/// Full content width inside the 20px side padding.
pub(crate) const FULL_W: f32 = INSPECTOR_W - 40.;

impl PipelineEditorScreen {
    pub(crate) fn render_inspector(
        &mut self,
        def: &PipelineDef,
        snap: &Snap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let tabs = [Tab::Step, Tab::Pipeline, Tab::Workdirs];
        let selected = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let on_select = cx.listener(move |this, i: &usize, _, cx| this.set_tab(tabs[*i], cx));
        let tab_bar = Tabs::new(
            "inspector-tabs",
            vec![
                TabItem::new("Step"),
                TabItem::new("Pipeline"),
                TabItem::new("Workdirs").count(def.workdirs.len().to_string()),
            ],
            selected,
        )
        .on_select(move |i, window, cx| on_select(&i, window, cx));

        let body: Vec<AnyElement> = match self.tab {
            Tab::Step => match self.sel_step(def).cloned() {
                Some(s) => self.ins_step(def, &s, snap, window, cx),
                None => self.ins_no_step(def, cx),
            },
            Tab::Pipeline => self.ins_pipeline(def, snap, window, cx),
            Tab::Workdirs => self.ins_workdirs(def, window, cx),
        };
        // The panel scrolls on its own; the inner column keeps its natural height.
        div()
            .id("inspector-scroll")
            .w(px(INSPECTOR_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(BG_SIDEBAR)
            .child(self.inspector_column(tab_bar, body, cx))
    }

    fn inspector_column(&mut self, tab_bar: Tabs, body: Vec<AnyElement>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .child(div().px(px(20.)).border_b_1().border_color(LINE).child(tab_bar))
            .when_some(self.error.clone(), |d, e| {
                d.child(
                    div()
                        .id("inspector-error")
                        .flex()
                        .items_start()
                        .gap(px(8.))
                        .px(px(20.))
                        .py(px(10.))
                        .border_b_1()
                        .border_color(LINE)
                        .bg(DANGER_SOFT)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(DANGER)
                        .child(div().flex_1().min_w_0().child(e))
                        .child(
                            div()
                                .id("dismiss-error")
                                .flex_none()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                }))
                                .child(icon("x", 12., DANGER)),
                        ),
                )
            })
            .children(body)
    }

    // ── Shared controls ─────────────────────────────────────────────────

    /// Text field box hosting the input of `f`; clicking anywhere in the box focuses it.
    pub(crate) fn text_field(&self, f: Field, mono: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let input = self.input(f);
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        let focus = input.clone();
        field_box()
            .id(("field", f as usize))
            .h(px(CONTROL_HEIGHT_SM + 4.))
            .px(px(10.))
            .cursor_text()
            .when(focused, |d| d.border_color(ACCENT_STRONG))
            .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
            .on_click(cx.listener(move |_, _, window, cx| focus.focus_handle(cx).focus(window)))
            .child(input)
            .into_any_element()
    }

    /// Multi-line value: wrapped text at rest (at least `rows` lines tall), the single-line
    /// input while editing (line breaks show as ⏎).
    pub(crate) fn long_field(&self, f: Field, rows: f32, mono: bool, value: &str, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.long_edit == Some(f);
        let line = if mono { 18. } else { LINE_BODY };
        let placeholder = f.placeholder();
        div()
            .id(("long", f as usize))
            .min_h(px(rows * line + 18.))
            .px(px(10.))
            .py(px(8.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(if editing { ACCENT_STRONG } else { LINE_STRONG })
            .rounded(px(RADIUS_LG))
            .line_height(px(line))
            .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
            .cursor_text()
            .when(editing, |d| d.child(div().flex().h(px(line)).child(self.input(f))))
            .when(!editing, |d| {
                d.child(if value.trim().is_empty() {
                    div().text_color(TEXT_MUTED).child(placeholder)
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .text_color(TEXT_SECONDARY)
                        .children(value.lines().map(|l| div().min_h(px(line)).child(SharedString::from(l.to_string()))))
                })
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.long_edit != Some(f) {
                    this.start_long_edit(f, window, cx);
                }
            }))
            .into_any_element()
    }

    /// Checkbox + label; clicking the row runs `toggle`.
    pub(crate) fn check_row(
        &self,
        key: &'static str,
        checked: bool,
        label: impl IntoElement,
        hint: Option<&'static str>,
        toggle: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(key)
            .flex()
            .items_start()
            .gap(px(10.))
            .min_h(px(24.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.commit_pending(cx);
                toggle(this, cx);
            }))
            .child(div().pt(px(2.)).child(checkbox(ElementId::Name(format!("{key}-box").into()), checked)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(label)
                    .when_some(hint, |d, h| d.child(help(h))),
            )
            .into_any_element()
    }

    /// "Delete …?" strip with Cancel and a danger button.
    pub(crate) fn confirm_strip(
        &self,
        id: &'static str,
        text: impl Into<SharedString>,
        action: &'static str,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(12.))
            .rounded(px(RADIUS_LG))
            .border_1()
            .border_color(DANGER)
            .bg(DANGER_SOFT)
            .child(div().text_size(px(12.)).line_height(px(18.)).text_color(TEXT_SECONDARY).child(text.into()))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new((id, 0usize), "Cancel")
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new((id, 1usize), action)
                            .small()
                            .variant(ButtonVariant::Danger)
                            .on_click(cx.listener(move |this, _, _, cx| run(this, cx))),
                    ),
            )
            .into_any_element()
    }

    /// Clickable list of the steps (icon, name, a detail) that selects one.
    fn step_list(&self, def: &PipelineDef, cx: &mut Context<Self>) -> Vec<AnyElement> {
        def.steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (ic, bg, fg) = kind_look(s.kind, s.delegate.is_some());
                let detail = if s.kind.uses_model() {
                    model_short(&s.model).to_string()
                } else {
                    kind_line(s)
                };
                let id = s.id.clone();
                div()
                    .id(("step-row", i))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(32.))
                    .px(px(8.))
                    .mx(px(-8.))
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer()
                    .hover(|st| st.bg(SURFACE_HOVER))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(Some(id.clone()), cx)))
                    .child(
                        div()
                            .w(px(16.))
                            .flex_none()
                            .font_family(FONT_MONO)
                            .text_size(px(11.))
                            .text_color(TEXT_DISABLED)
                            .child(SharedString::from((i + 1).to_string())),
                    )
                    .child(
                        div()
                            .size(px(22.))
                            .flex_none()
                            .rounded(px(RADIUS_SM))
                            .bg(bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(ic, 12., fg)),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(SharedString::from(s.name.clone())))
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(130.))
                            .truncate()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child(SharedString::from(detail)),
                    )
                    .into_any_element()
            })
            .collect()
    }

    // ── Bodies ──────────────────────────────────────────────────────────

    fn ins_no_step(&self, def: &PipelineDef, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            section(4.)
                .border_b_0()
                .child(strong("No step selected").mb(px(2.)))
                .child(help(if def.steps.is_empty() {
                    "This pipeline has no steps yet. Use “Add step” above the canvas."
                } else {
                    "Select a step on the canvas or below to edit it."
                }))
                .child(div().h(px(8.)))
                .children(self.step_list(def, cx))
                .into_any_element(),
        ]
    }

    fn ins_pipeline(&self, def: &PipelineDef, snap: &Snap, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let active = snap.pipes.iter().find(|p| p.id == def.id).map(|p| p.active).unwrap_or(0);
        let wd_rows: Vec<_> = def
            .workdirs
            .iter()
            .map(|w| {
                kv(
                    w.name.clone(),
                    SharedString::from(if w.path.as_os_str().is_empty() {
                        "no folder".to_string()
                    } else {
                        pipeline::def::tilde(&w.path)
                    }),
                    true,
                )
            })
            .collect();
        let mut out = vec![
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(20.))
                .py(px(16.))
                .border_b_1()
                .border_color(LINE)
                .child(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .rounded(px(RADIUS_MD))
                        .bg(ACCENT_SOFT)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("pipelines", 14., ACCENT_TEXT)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(20.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(SharedString::from(def.name.clone())),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(TEXT_MUTED)
                                .truncate()
                                .child(SharedString::from(format!(
                                    "Pipeline · {} {}",
                                    def.steps.len(),
                                    if def.steps.len() == 1 { "step" } else { "steps" }
                                ))),
                        ),
                )
                .into_any_element(),
            section(12.)
                .child(div().child(field_label("Name")).child(self.text_field(Field::PipeName, false, window, cx)))
                .child(
                    div()
                        .child(field_label("Description"))
                        .child(self.long_field(Field::PipeDesc, 3., false, &def.description, cx)),
                )
                .child(kv("File", SharedString::from(format!("pipelines/{}.toml", def.id)), true))
                .child(kv("Id", SharedString::from(def.id.clone()), true))
                .child(help("In the harness config folder. Issues pick a pipeline by its id. Changes are saved right away; runs in progress keep the version they started with."))
                .into_any_element(),
            section(2.)
                .child(strong("Steps").mb(px(6.)))
                .when(def.steps.is_empty(), |d| d.child(help("No steps yet.")))
                .children(self.step_list(def, cx))
                .into_any_element(),
            section(8.)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(strong("Workdirs"))
                        .child(
                            Button::new("manage-workdirs", "Manage")
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Workdirs, cx))),
                        ),
                )
                .when(wd_rows.is_empty(), |d| d.child(help("No workdirs: steps that work in a folder need one.")))
                .children(wd_rows)
                .into_any_element(),
        ];
        let confirming = self.confirm == Some(Confirm::Pipeline);
        out.push(
            section(10.)
                .border_b_0()
                .when(!confirming, |d| {
                    d.child(
                        div().flex().child(
                            Button::new("delete-pipeline", "Delete pipeline…")
                                .small()
                                .variant(ButtonVariant::Danger)
                                .on_click(cx.listener(|this, _, _, cx| this.ask_confirm(Confirm::Pipeline, cx))),
                        ),
                    )
                })
                .when(confirming, |d| {
                    let text = if active > 0 {
                        format!(
                            "A run of “{}” is active, so it cannot be deleted until the run stops.",
                            def.name
                        )
                    } else {
                        format!(
                            "Delete “{}”? Its file is removed; issues on it run through the default pipeline.",
                            def.name
                        )
                    };
                    d.child(self.confirm_strip("confirm-pipeline", text, "Delete pipeline", |this, cx| this.delete_pipeline(cx), cx))
                })
                .into_any_element(),
        );
        out
    }
}
