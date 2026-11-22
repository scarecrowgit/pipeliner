//! Composer: prompt input, agent button + forced-skill chips (see `picker`), model menu,
//! hints, Send / Stop, and the slash-command / @-mention popovers.

use gpui::{
    AnyElement, Context, ElementId, IntoElement, MouseButton, deferred, div, prelude::*, px,
};
use ui::{Button, ButtonVariant, icon, kbd, theme::*};

use crate::models::{ModelRef, ProviderModels};
use crate::widgets::spinner;
use crate::{ChatScreen, ConfigState, SEARCH_THRESHOLD, Suggest, Trigger};

/// Rows rendered in the model menu at most (search narrows the rest).
const MAX_MODEL_ROWS: usize = 300;

fn eid(name: &'static str, ix: usize) -> ElementId {
    ElementId::NamedInteger(name.into(), ix as u64)
}

/// Popover surface (`listbox` in the mockup).
fn menu_surface() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .p(px(4.))
        .border_1()
        .border_color(LINE_STRONG)
        .rounded(px(RADIUS_XL))
        .bg(SURFACE_RAISED)
        .shadow(popover_shadow())
}

/// Wrap a popover so it sits `gap` px above its relative parent, painted on top, at
/// animation frame `f` (rises out of the trigger, see `ui::motion`).
fn above(
    id: &'static str,
    gap: f32,
    width: Option<f32>,
    f: ui::motion::PopFrame,
    menu: impl IntoElement,
) -> impl IntoElement {
    let anchor = div().absolute().left_0().bottom_full();
    let anchor = match width {
        Some(w) => anchor.w(px(w)),
        None => anchor.right_0(),
    };
    let anchor = ui::motion::pop_above(anchor, gap, f);
    deferred(anchor.child(div().id(id).occlude().child(menu))).with_priority(1)
}

impl ChatScreen {
    pub(crate) fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = self.is_shell();
        let empty = self.draft.trim().is_empty();

        let input_area = div()
            .h(px(76.))
            .pt(px(12.))
            .px(px(14.))
            .pb(px(4.))
            .flex()
            .items_start()
            .cursor_text()
            .font_family(if shell { FONT_MONO } else { FONT_SANS })
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(TEXT)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus_input(window, cx)),
            )
            .child(div().flex().w_full().h(px(20.)).child(self.input.clone()));

        let hints = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_x(px(10.))
            .gap_y(px(4.))
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .children(
                [("@", "files"), ("/", "commands"), ("!", "shell")].map(|(k, t)| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(kbd(k))
                        .child(t)
                }),
            );

        // Left group (agent, forced-skill chips, model) wraps on its own; hints + Send stay
        // together on the right so Send never drops to a line of its own.
        let selectors = div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(self.agent_selector(cx))
            .children(self.forced_chips(cx))
            .child(self.model_selector(cx));
        let actions = div().flex().flex_none().items_center().gap(px(10.)).child(hints);
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(8.))
            .child(selectors)
            .child(actions.child(if self.busy() {
                Button::new("stop", "Stop")
                    .small()
                    .variant(ButtonVariant::Danger)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.interrupt(cx);
                        this.focus_input(window, cx);
                    }))
            } else {
                Button::new("send", if shell { "Run" } else { "Send" })
                    .small()
                    .primary()
                    .disabled(empty)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send(window, cx);
                        this.focus_input(window, cx);
                    }))
            }));

        let prompt_box = div()
            .border_1()
            .border_color(if shell { WARNING } else { LINE_STRONG })
            .rounded(px(RADIUS_XL))
            .bg(SURFACE)
            .child(input_area)
            .child(toolbar);

        let footer = div()
            .flex()
            .flex_wrap()
            .justify_between()
            .gap(px(8.))
            .pt(px(6.))
            .px(px(4.))
            .text_size(px(12.))
            .text_color(TEXT_MUTED)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(self.busy(), |d| {
                        d.child(spinner("busy-spinner", 12., TEXT_MUTED))
                    })
                    .child(self.mode_hint()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(kbd("Tab"))
                    .child("Build ⇄ Plan ·")
                    .child(kbd("Esc"))
                    .child("interrupt"),
            );

        div().flex_none().px(px(24.)).pb(px(20.)).child(
            div()
                .relative()
                .w_full()
                .max_w(px(780.))
                .mx_auto()
                .children(self.suggest_popover(cx))
                .child(prompt_box)
                .child(footer),
        )
    }

    // ── Model menu ──────────────────────────────────────────────────────

    fn model_selector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let label = match (&self.model, &self.config) {
            (Some(m), _) => m.label(),
            (None, ConfigState::Loading) => "Loading models…".into(),
            (None, _) if !self.has_providers() => "No provider".into(),
            (None, _) => "Select a model".into(),
        };
        let trigger = div()
            .id("model-trigger")
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(28.))
            .max_w(px(320.))
            .px(px(8.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE_RAISED)
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .text_color(TEXT_SECONDARY)
            .cursor_pointer()
            .hover(|s| s.bg(SURFACE_HOVER))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                this.trigger_hover = hovered.then_some(Trigger::Model);
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                if this.model_open {
                    this.model_open = false;
                    this.focus_input(window, cx);
                    cx.notify();
                } else {
                    this.open_model_menu(window, cx);
                }
            }))
            .child(div().min_w_0().truncate().child(label))
            .child(icon("chevron-down", 12., TEXT_MUTED));

        div().relative().child(trigger).when_some(self.pops.model, |d, f| {
            d.child(above("model-menu", 8., Some(340.), f, self.model_menu(cx)))
        })
    }

    fn model_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let group = |text: String| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .pt(px(8.))
                .pb(px(4.))
                .text_size(px(12.))
                .text_color(TEXT_MUTED)
                .child(text)
        };
        let searching = self.model_count() > SEARCH_THRESHOLD;
        let q = self.model_query.trim().to_lowercase();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut shown = 0usize;
        let mut ix = 0usize;
        for (provider, state) in &self.catalog {
            match state {
                ProviderModels::Loading => rows.push(
                    group(provider.clone())
                        .child(spinner(eid("model-spin", ix), 12., TEXT_MUTED))
                        .into_any_element(),
                ),
                ProviderModels::Failed(e) => rows.push(
                    div()
                        .flex()
                        .flex_col()
                        .child(group(provider.clone()))
                        .child(
                            div()
                                .px(px(8.))
                                .pb(px(6.))
                                .text_size(px(12.))
                                .text_color(DANGER)
                                .child(e.lines().next().unwrap_or("").to_string()),
                        )
                        .into_any_element(),
                ),
                ProviderModels::Ready(list) => {
                    let matching: Vec<&String> = list
                        .iter()
                        .filter(|m| {
                            q.is_empty() || format!("{provider}/{m}").to_lowercase().contains(&q)
                        })
                        .collect();
                    if matching.is_empty() && !q.is_empty() {
                        continue;
                    }
                    rows.push(group(provider.clone()).into_any_element());
                    if list.is_empty() {
                        rows.push(
                            div()
                                .px(px(8.))
                                .pb(px(6.))
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("No models")
                                .into_any_element(),
                        );
                    }
                    for m in matching {
                        if shown >= MAX_MODEL_ROWS {
                            break;
                        }
                        shown += 1;
                        ix += 1;
                        let r = ModelRef {
                            provider: provider.clone(),
                            model: m.clone(),
                        };
                        let on = self.model.as_ref() == Some(&r);
                        rows.push(
                            ui::motion::fade_row(
                                div().id(eid("pick-model", ix)),
                                eid("pick-model", ix),
                                on,
                                SURFACE_HOVER,
                            )
                                .flex()
                                .items_center()
                                .h(px(30.))
                                .px(px(8.))
                                .rounded(px(RADIUS_MD))
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(if on { TEXT } else { TEXT_SECONDARY })
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.select_model(r.clone(), window, cx);
                                }))
                                .child(div().min_w_0().truncate().child(m.clone()))
                                .into_any_element(),
                        );
                    }
                }
            }
            ix += 1;
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .text_color(TEXT_MUTED)
                    .child(if self.catalog.is_empty() {
                        "No providers configured"
                    } else {
                        "No model matches"
                    })
                    .into_any_element(),
            );
        }

        menu_surface()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.trigger_hover != Some(Trigger::Model) {
                    this.model_open = false;
                    this.focus_input(window, cx);
                    cx.notify();
                }
            }))
            .when(searching, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(32.))
                        .mx(px(4.))
                        .mt(px(4.))
                        .mb(px(2.))
                        .px(px(8.))
                        .rounded(px(RADIUS_MD))
                        .border_1()
                        .border_color(LINE_STRONG)
                        .bg(SURFACE)
                        .text_size(px(12.))
                        .text_color(TEXT)
                        .child(icon("search", 12., TEXT_MUTED))
                        .child(div().flex_1().h(px(20.)).child(self.model_search.clone())),
                )
            })
            .child(
                div()
                    .id("model-list")
                    .flex()
                    .flex_col()
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    // ── Slash commands / mentions ───────────────────────────────────────

    fn suggest_popover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let option = |id: ElementId, ix: usize, sel: usize| {
            ui::motion::fade_row(div().id(id.clone()), id, ix == sel, SURFACE_HOVER)
                .flex()
                .items_center()
                .w_full()
                .min_h(px(34.))
                .px(px(10.))
                .py(px(4.))
                .rounded(px(RADIUS_MD))
                .text_color(TEXT)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.pick_suggestion(ix, cx);
                    this.focus_input(window, cx);
                }))
        };
        let sel = self.menu_sel;
        let rows: Vec<AnyElement> = match self.suggestions() {
            Suggest::None => return None,
            Suggest::Cmds(cmds) if cmds.is_empty() => vec![
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .px(px(10.))
                    .py(px(8.))
                    .text_color(TEXT_MUTED)
                    .child("No command matches. Try /new, /clear, /model, /plan or /build.")
                    .into_any_element(),
            ],
            Suggest::Cmds(cmds) => cmds
                .into_iter()
                .enumerate()
                .map(|(ix, c)| {
                    option(eid("cmd", ix), ix, sel)
                        .gap(px(12.))
                        .child(
                            div()
                                .w(px(88.))
                                .flex_none()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(ACCENT_TEXT)
                                .child(format!("/{}", c.name)),
                        )
                        .child(div().flex_1().text_color(TEXT_SECONDARY).child(c.desc))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(11.))
                                .text_color(TEXT_MUTED)
                                .child(c.key),
                        )
                        .into_any_element()
                })
                .collect(),
            Suggest::Mentions { items, .. } => items
                .into_iter()
                .enumerate()
                .map(|(ix, m)| {
                    option(eid("mention", ix), ix, sel)
                        .gap(px(10.))
                        .child(icon("file", 14., TEXT_MUTED))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT)
                                .child(m.label),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("file"),
                        )
                        .into_any_element()
                })
                .collect(),
        };
        Some(above(
            "suggest",
            6.,
            None,
            self.pops.suggest?,
            menu_surface()
                .id("suggest-list")
                .max_h(px(300.))
                .overflow_y_scroll()
                .children(rows),
        ))
    }
}
