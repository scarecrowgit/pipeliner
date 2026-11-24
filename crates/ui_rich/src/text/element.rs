//! The element that shapes, wraps, scrolls and paints an editor's buffer, registers the
//! platform input handler and handles the mouse.

use std::rc::Rc;

use gpui::{
    App, AvailableSpace, Bounds, ContentMask, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, SharedString, Style, Window, fill, point, px, relative, size,
};
use ui::theme::{ACCENT, TEXT_DISABLED, TEXT_MUTED, with_alpha};

use super::core::EditorView;
use super::layout::{Shaped, TextLook};

/// Paints the buffer of the view `V`. One per editor, inside its focus-tracking div.
pub(crate) struct TextElement<V: EditorView> {
    view: Entity<V>,
}

impl<V: EditorView> TextElement<V> {
    pub fn new(view: Entity<V>) -> Self {
        Self { view }
    }
}

pub(crate) struct Prepaint {
    shaped: Rc<Shaped>,
    placeholder: Option<Shaped>,
    hitbox: Hitbox,
}

fn look(window: &Window) -> TextLook {
    let style = window.text_style();
    let rem = window.rem_size();
    TextLook {
        font: style.font(),
        font_size: style.font_size.to_pixels(rem),
        line_height: style.line_height_in_pixels(rem),
        color: style.color,
    }
}

impl<V: EditorView> IntoElement for TextElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<V: EditorView> Element for TextElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let look = look(window);
        let core = self.view.read(cx).core();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        if !core.multiline {
            style.size.height = look.line_height.into();
            return (window.request_layout(style, [], cx), ());
        }
        let text: SharedString = core.buffer.text().to_string().into();
        let placeholder = core.placeholder.clone();
        let styles = core.styles.clone();
        let (min, max) = (
            core.min_rows.max(1),
            core.max_rows.max(core.min_rows).max(1),
        );
        let id = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let (shown, styles) = if text.is_empty() {
                (placeholder.clone(), &[][..])
            } else {
                (text.clone(), &styles[..])
            };
            let shaped = Shaped::new(shown, &look, None, styles, width, window);
            let rows = shaped.rows().clamp(min, max);
            size(
                known.width.unwrap_or_else(|| shaped.width()),
                known.height.unwrap_or(look.line_height * rows as f32),
            )
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let mut look = look(window);
        let core = self.view.read(cx).core();
        if core.disabled {
            look.color = TEXT_DISABLED;
        }
        let wrap = core.multiline.then_some(bounds.size.width);
        let text: SharedString = core.buffer.text().to_string().into();
        let shaped = Rc::new(Shaped::new(text, &look, core.buffer.marked(), &core.styles, wrap, window));
        let placeholder = (core.buffer.is_empty() && !core.placeholder.is_empty()).then(|| {
            let muted = TextLook {
                color: TEXT_MUTED,
                ..look.clone()
            };
            Shaped::new(core.placeholder.clone(), &muted, None, &[], wrap, window)
        });
        let s = shaped.clone();
        self.view.update(cx, |v, _| {
            let core = v.core_mut();
            let caret = s.caret_position(core.buffer.head());
            let lh = f32::from(s.line_height());
            if core.multiline {
                let view_h = f32::from(bounds.size.height);
                let max = (f32::from(s.height()) - view_h).max(0.);
                let mut y = f32::from(core.scroll.y);
                if core.autoscroll {
                    let cy = f32::from(caret.y);
                    if cy < y {
                        y = cy;
                    } else if cy + lh > y + view_h {
                        y = cy + lh - view_h;
                    }
                }
                core.scroll = point(px(0.), px(y.clamp(0., max)));
            } else {
                let view_w = f32::from(bounds.size.width);
                let max = (f32::from(s.width()) + 1. - view_w).max(0.);
                let mut x = f32::from(core.scroll.x);
                if core.autoscroll {
                    let cx_ = f32::from(caret.x);
                    if cx_ < x {
                        x = cx_;
                    } else if cx_ + 1. > x + view_w {
                        x = cx_ + 1. - view_w;
                    }
                }
                core.scroll = point(px(x.clamp(0., max)), px(0.));
            }
            core.autoscroll = false;
            core.shaped = Some(s);
            core.bounds = Some(bounds);
        });
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Prepaint {
            shaped,
            placeholder,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let core = self.view.read(cx).core();
        let focus = core.focus.clone();
        let disabled = core.disabled;
        let focused = focus.is_focused(window);
        let origin = bounds.origin - core.scroll;
        let selection = core.buffer.selection();
        let show_caret = focused && core.caret_on && !disabled && selection.is_empty();
        let caret = state.shaped.caret_position(core.buffer.head());
        let multiline = core.multiline;

        if !disabled {
            window.handle_input(
                &focus,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
            window.set_cursor_style(CursorStyle::IBeam, &state.hitbox);
        }

        let shaped = state.shaped.clone();
        let lh = shaped.line_height();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if state.placeholder.is_none() {
                shaped.paint_background(origin, bounds, window, cx);
            }
            let tint = with_alpha(ACCENT, if focused { 0.32 } else { 0.16 });
            for r in shaped.selection_rects(selection.clone()) {
                window.paint_quad(fill(Bounds::new(origin + r.origin, r.size), tint));
            }
            match &state.placeholder {
                Some(p) => p.paint(origin, bounds, window, cx),
                None => shaped.paint(origin, bounds, window, cx),
            }
            if show_caret {
                let rect = Bounds::new(origin + caret, size(px(1.), lh));
                window.paint_quad(fill(rect, ACCENT));
            }
        });

        if disabled {
            return;
        }
        let (view, hitbox) = (self.view.clone(), state.hitbox.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || e.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            window.focus(&focus);
            view.update(cx, |v, cx| {
                v.core_mut()
                    .mouse_down(e.position, e.click_count, e.modifiers.shift);
                cx.notify();
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble
                || e.pressed_button != Some(MouseButton::Left)
                || view.read(cx).core().drag.is_none()
            {
                return;
            }
            view.update(cx, |v, cx| {
                if v.core_mut().mouse_drag(e.position) {
                    cx.notify();
                }
            });
        });

        let view = self.view.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && view.read(cx).core().drag.is_some() {
                view.update(cx, |v, _| v.core_mut().drag = None);
            }
        });

        let (view, hitbox) = (self.view.clone(), state.hitbox.clone());
        let extent = if multiline {
            (f32::from(shaped.height()) - f32::from(bounds.size.height)).max(0.)
        } else {
            (f32::from(shaped.width()) + 1. - f32::from(bounds.size.width)).max(0.)
        };
        window.on_mouse_event(move |e: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) || extent <= 0. {
                return;
            }
            let delta = e.delta.pixel_delta(lh);
            let scrolled = view.update(cx, |v, cx| {
                let core = v.core_mut();
                let (now, d) = if multiline {
                    (f32::from(core.scroll.y), f32::from(delta.y))
                } else {
                    (
                        f32::from(core.scroll.x),
                        f32::from(delta.x) + f32::from(delta.y),
                    )
                };
                let next = (now - d).clamp(0., extent);
                if (next - now).abs() < 0.01 {
                    return false;
                }
                core.scroll = if multiline {
                    point(px(0.), px(next))
                } else {
                    point(px(next), px(0.))
                };
                cx.notify();
                true
            });
            if scrolled {
                cx.stop_propagation();
            }
        });
    }
}
