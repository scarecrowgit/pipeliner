//! A small window for proving the dev driver end to end with real input: a kit `Button`
//! that counts clicks, a hover card, a menu that closes on click-away, a draggable knob, a
//! wheel-scrolled list, a focusable control Enter activates, and a text field that echoes.
//!
//! By hand: `HARNESS_DEV_SOCKET=/run/user/1000/demo.sock cargo run -p devtools --example
//! driver_demo`, then `harness-dev drive --socket /run/user/1000/demo.sock click increment`.
//! Scripted: the ignored test `tests/driver_window.rs`.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, DragMoveEvent, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Pixels, Render, ScrollHandle, SharedString, UTF16Selection, Window, canvas, deferred, div,
};
use ui::dev::Probe;
use ui::prelude::*;

const TRACK: f32 = 360.;
const KNOB: f32 = 24.;

struct Demo {
    count: usize,
    menu_open: bool,
    knob_x: f32,
    activations: usize,
    text: String,
    field_focus: FocusHandle,
    control_focus: FocusHandle,
    scroll: ScrollHandle,
}

/// The value carried while the knob is dragged.
struct Knob;

/// The drag preview: nothing, the knob itself moves.
struct Ghost;

impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn label(text: impl Into<SharedString>) -> gpui::Div {
    div().text_color(TEXT_MUTED).child(text.into())
}

impl Demo {
    fn menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .child(
                div()
                    .id("open-menu")
                    .probe("open-menu")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = true;
                        cx.notify();
                    }))
                    .px(px(SPACE_3))
                    .py(px(SPACE_1))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE)
                    .child("Open menu"),
            )
            .when(self.menu_open, |el| {
                // Deferred: painted after the rest of the window, so it sits on top.
                el.child(deferred(
                    div()
                        .id("menu")
                        .probe("menu")
                        .absolute()
                        .top(px(32.))
                        .w(px(160.))
                        .p(px(SPACE_2))
                        .rounded(px(RADIUS_LG))
                        .bg(SURFACE)
                        .border_1()
                        .border_color(ACCENT)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu_open = false;
                            cx.notify();
                        }))
                        .child("Menu (click away)"),
                ))
            })
    }

    fn slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("track")
            .probe("track")
            .relative()
            .w(px(TRACK))
            .h(px(KNOB))
            .rounded(px(RADIUS_FULL))
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<Knob>, _, cx| {
                let x = f32::from(e.event.position.x - e.bounds.left()) - KNOB / 2.;
                this.knob_x = x.clamp(0., TRACK - KNOB);
                cx.notify();
            }))
            .child(
                div()
                    .id("knob")
                    .probe("knob")
                    .absolute()
                    .left(px(self.knob_x))
                    .size(px(KNOB))
                    .rounded(px(RADIUS_FULL))
                    .bg(ACCENT)
                    .on_drag(Knob, |_, _, _, cx| cx.new(|_| Ghost)),
            )
    }

    fn field(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.field_focus.is_focused(window);
        let entity = cx.entity();
        let focus = self.field_focus.clone();
        let shown = if self.text.is_empty() { "Click here, then type".into() } else { self.text.clone() };
        div()
            .id("field")
            .probe("field")
            .track_focus(&self.field_focus)
            .relative()
            .w(px(TRACK))
            .h(px(32.))
            .px(px(SPACE_3))
            .flex()
            .items_center()
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(if focused { ACCENT } else { LINE })
            .bg(SURFACE)
            .child(div().flex_1().child(shown))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        let handler = ElementInputHandler::new(bounds, entity.clone());
                        window.handle_input(&focus, handler, cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let control_focused = self.control_focus.is_focused(window);
        let offset = -f32::from(self.scroll.offset().y);
        root().child(
            div()
                .flex()
                .flex_col()
                .gap(px(SPACE_3))
                .p(px(SPACE_6))
                .child(
                    div().flex().gap(px(SPACE_4)).items_center().child(
                        div().probe("increment").child(Button::new("increment", "Increment").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.count += 1;
                                cx.notify();
                            }),
                        )),
                    )
                    .child(
                        div()
                            .probe_value("count", self.count.to_string())
                            .child(format!("Clicked {} times", self.count)),
                    ),
                )
                .child(
                    div()
                        .id("hover-card")
                        .probe("hover-card")
                        .w(px(TRACK))
                        .p(px(SPACE_3))
                        .rounded(px(RADIUS_LG))
                        .border_1()
                        .border_color(LINE)
                        .bg(SURFACE)
                        .hover(|s| s.bg(ACCENT).border_color(TEXT))
                        .child("Hover card: turns accent under the pointer"),
                )
                .child(
                    div()
                        .flex()
                        .gap(px(SPACE_4))
                        .items_center()
                        .child(self.menu(cx))
                        .child(
                            label(if self.menu_open { "Menu open" } else { "Menu closed" })
                                .probe_value("menu-state", if self.menu_open { "open" } else { "closed" }),
                        ),
                )
                .child(self.slider(cx))
                .child(label(format!("Knob at {:.0}", self.knob_x)).probe_value("knob-x", format!("{:.0}", self.knob_x)))
                .child(
                    div()
                        .id("control")
                        .probe("control")
                        .track_focus(&self.control_focus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.activations += 1;
                            cx.notify();
                        }))
                        .w(px(TRACK))
                        .px(px(SPACE_3))
                        .py(px(SPACE_2))
                        .rounded(px(RADIUS_MD))
                        .border_1()
                        .border_color(if control_focused { ACCENT } else { LINE })
                        .child(format!("Focusable control, activated {} times", self.activations)),
                )
                .child(label("").probe_value("activations", self.activations.to_string()))
                .child(self.field(window, cx))
                .child(label(format!("Echo: {}", self.text)).probe_value("echo", self.text.clone()))
                .child(
                    div()
                        .id("list")
                        .probe_value("list", format!("{offset:.0}"))
                        .track_scroll(&self.scroll)
                        .overflow_y_scroll()
                        .w(px(TRACK))
                        .h(px(100.))
                        .border_1()
                        .border_color(LINE)
                        .flex()
                        .flex_col()
                        .children((0..30).map(|i| {
                            div().flex_shrink_0().px(px(SPACE_2)).child(format!("Row {i}"))
                        })),
                ),
        )
    }
}

/// Append-only text input: enough for `type` to reach a real input handler.
impl EntityInputHandler for Demo {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.text.encode_utf16().collect();
        let range = range.start.min(units.len())..range.end.min(units.len());
        Some(String::from_utf16_lossy(&units[range]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.text.encode_utf16().count();
        Some(UTF16Selection { range: end..end, reversed: false })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.push_str(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }
}

fn main() {
    ui::run_app("Driver demo", 900., 640., |_window: &mut Window, cx: &mut App| -> Entity<Demo> {
        ui::dev::install(cx);
        cx.new(|cx| Demo {
            count: 0,
            menu_open: false,
            knob_x: 0.,
            activations: 0,
            text: String::new(),
            field_focus: cx.focus_handle(),
            control_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        })
    });
}
