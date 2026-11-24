//! The viewer's view: toolbar, the clipped stage with the zoomed image, and the pointer and
//! key handling that drives the [`Viewport`].

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, ImageSource, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, ScrollWheelEvent,
    SharedString, Subscription, WeakEntity, Window, canvas, div, img, prelude::*, px,
};
use ui::dev::Probe;
use ui::overlay::{LayerId, current_layer};
use ui::prelude::*;

use super::geometry::{Extent, MARGIN, MAX_SCALE, Mode, Viewport};
use super::load::{self, Loaded};
use super::{
    KEY_CONTEXT, NextImage, PanDown, PanUp, PreviousImage, VIEWER_ID, ViewerImage, ViewerSource,
    ZoomActual, ZoomFit, ZoomFitWidth, ZoomIn, ZoomOut,
};
use crate::markdown::Markdown;

/// Height of the toolbar above the stage.
pub(super) const TOOLBAR_H: f32 = 48.0;
/// Travel in px before a press on the stage becomes a drag (and no longer a click).
const DRAG_THRESHOLD: f32 = 3.0;
/// Zoom per pixel of Ctrl+wheel travel: `exp(dy × WHEEL_ZOOM)`.
const WHEEL_ZOOM: f32 = 0.005;
/// Up and Down scroll this far.
const KEY_PAN: f32 = 80.0;
/// Room under the image for the caption: its 28px pill 16px above the bottom, and a gap.
const CAPTION_ROOM: f32 = SPACE_4 + 28. + SPACE_3;
/// Width of the percent between the zoom buttons.
const PERCENT_W: f32 = 48.0;

/// A press on the stage, until it is released.
#[derive(Clone, Copy, Debug)]
struct Drag {
    start: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
    on_image: bool,
}

/// The open viewer: which image, its decoding, and its view.
pub(super) struct ImageViewer {
    pub(super) images: Vec<ViewerImage>,
    pub(super) index: usize,
    pub(super) focus: FocusHandle,
    focused: bool,
    /// Settled decodings (ready or failed), per image.
    loaded: Vec<Option<Loaded>>,
    /// The view of the current image, once its size is known.
    pub(super) viewport: Option<Viewport>,
    drag: Option<Drag>,
    /// Where the stage was laid out last frame, for pointer positions.
    stage_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    errors: HashMap<usize, Entity<Markdown>>,
    layer: Option<LayerId>,
    _release: Subscription,
}

impl ImageViewer {
    pub(super) fn new(images: Vec<ViewerImage>, start: usize, cx: &mut Context<Self>) -> Self {
        let index = start.min(images.len().saturating_sub(1));
        let release = cx.on_release(|this: &mut Self, cx| {
            // Textures and decoded copies of in-memory images are the viewer's own; file images
            // stay cached for the thumbnails that show them.
            for (image, loaded) in this.images.iter().zip(&this.loaded) {
                if let (ViewerSource::Memory(_), Some(Loaded::Ready(r))) = (&image.source, loaded) {
                    cx.drop_image(r.clone(), None);
                }
                load::release(&image.source, cx);
            }
        });
        Self {
            loaded: vec![None; images.len()],
            images,
            index,
            focus: cx.focus_handle(),
            focused: false,
            viewport: None,
            drag: None,
            stage_bounds: Rc::default(),
            errors: HashMap::new(),
            layer: None,
            _release: release,
        }
    }

    /// The failure text of the current image, when it could not be loaded.
    #[cfg(test)]
    pub(super) fn error_text(&self, cx: &App) -> Option<String> {
        self.errors.get(&self.index).map(|md| md.read(cx).text().to_string())
    }

    fn current(&mut self, window: &mut Window, cx: &mut App) -> Loaded {
        if let Some(Some(done)) = self.loaded.get(self.index) {
            return done.clone();
        }
        let Some(image) = self.images.get(self.index) else { return Loaded::Pending };
        let now = load::load(&image.source, window, cx);
        if !matches!(now, Loaded::Pending) {
            self.loaded[self.index] = Some(now.clone());
        }
        now
    }

    /// The stage: the window under the toolbar.
    fn stage(window: &Window) -> Extent {
        let s = window.viewport_size();
        Extent::new(f32::from(s.width), f32::from(s.height) - TOOLBAR_H)
    }

    /// A window position in px from the stage's top-left.
    fn local(&self, at: Point<Pixels>) -> (f32, f32) {
        let origin = self.stage_bounds.get().map_or(gpui::point(px(0.), px(TOOLBAR_H)), |b| b.origin);
        (f32::from(at.x - origin.x), f32::from(at.y - origin.y))
    }

    fn with_view(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Viewport)) {
        if let Some(v) = self.viewport.as_mut() {
            f(v);
            cx.notify();
        }
    }

    /// Show the image `delta` places away, if there is one.
    pub(super) fn go(&mut self, delta: isize, cx: &mut Context<Self>) {
        let next = self.index as isize + delta;
        if next < 0 || next as usize >= self.images.len() {
            return;
        }
        self.index = next as usize;
        self.viewport = None;
        self.drag = None;
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let overlays = overlays(window, cx);
        match self.layer {
            Some(layer) => overlays.close_layer(layer, window, cx),
            None => overlays.close(VIEWER_ID, window, cx),
        }
    }

    fn pointer_pressed(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let (lx, ly) = self.local(ev.position);
        let on_image = self.viewport.is_some_and(|v| v.hits_image(lx, ly));
        if ev.click_count >= 2 && on_image {
            self.drag = None;
            self.with_view(cx, |v| v.toggle_actual(lx, ly));
            return;
        }
        self.drag = Some(Drag { start: ev.position, last: ev.position, moved: false, on_image });
        cx.notify();
    }

    fn pointer_moved(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            // The button went up where we did not hear it.
            self.drag = None;
            cx.notify();
            return;
        }
        let travel = f32::from((ev.position.x - drag.start.x).abs() + (ev.position.y - drag.start.y).abs());
        if !drag.moved && travel < DRAG_THRESHOLD {
            return;
        }
        drag.moved = true;
        let (dx, dy) = (f32::from(ev.position.x - drag.last.x), f32::from(ev.position.y - drag.last.y));
        drag.last = ev.position;
        if let Some(v) = self.viewport.as_mut() {
            v.pan(dx, dy);
        }
        cx.notify();
    }

    fn pointer_released(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        cx.notify();
        if !drag.moved && !drag.on_image && ev.click_count <= 1 {
            self.close(window, cx);
        }
    }

    fn wheel(&mut self, ev: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let d = ev.delta.pixel_delta(window.line_height());
        let (dx, dy) = (f32::from(d.x), f32::from(d.y));
        let (ax, ay) = self.local(ev.position);
        if ev.modifiers.control || ev.modifiers.platform {
            self.with_view(cx, |v| v.zoom_at(ax, ay, v.scale * (dy * WHEEL_ZOOM).exp()));
        } else if ev.modifiers.shift && dx == 0. {
            self.with_view(cx, |v| v.pan(dy, 0.));
        } else {
            self.with_view(cx, |v| v.pan(dx, dy));
        }
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let image = &self.images[self.index];
        let count = self.images.len();
        let ready = self.viewport.is_some();
        let v = self.viewport;
        let percent = v.map_or_else(|| "–".to_string(), |v| format!("{}%", v.percent()));
        let mode = v.map(|v| v.mode);
        let at_max = v.is_some_and(|v| v.scale >= MAX_SCALE - 1e-4);
        let at_min = v.is_some_and(|v| v.scale <= v.min_scale() + 1e-4);
        let actual = v.is_some_and(|v| v.percent() == 100);

        let position = (count > 1).then(|| {
            let text = format!("{} of {}", self.index + 1, count);
            row()
                .flex_none()
                .gap(px(SPACE_1))
                .child(
                    div()
                        .type_style(LABEL)
                        .text_color(TEXT_MUTED)
                        .tabular()
                        .mr(px(SPACE_1))
                        .child(text.clone())
                        .probe_value("viewer-position", text),
                )
                .child(
                    Button::new("viewer-prev", "Previous")
                        .ghost()
                        .small()
                        .focus_on_click(false)
                        .disabled(self.index == 0)
                        .on_click(cx.listener(|this, _, _, cx| this.go(-1, cx)))
                        .probe("viewer-prev"),
                )
                .child(
                    Button::new("viewer-next", "Next")
                        .ghost()
                        .small()
                        .focus_on_click(false)
                        .disabled(self.index + 1 >= count)
                        .on_click(cx.listener(|this, _, _, cx| this.go(1, cx)))
                        .probe("viewer-next"),
                )
        });

        let zoom = row()
            .flex_none()
            .h(px(CONTROL_HEIGHT_SM))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(LINE_STRONG)
            .bg(SURFACE)
            .child(
                IconButton::new("viewer-zoom-out", IconName::Minus)
                    .small()
                    .tooltip("Zoom out")
                    .focus_on_click(false)
                    .disabled(!ready || at_min)
                    .on_click(cx.listener(|this, _, _, cx| this.with_view(cx, |v| v.step(false))))
                    .probe("viewer-zoom-out"),
            )
            .child(
                div()
                    .w(px(PERCENT_W))
                    .flex()
                    .justify_center()
                    .type_style(MONO_SM)
                    .tabular()
                    .text_color(TEXT_SECONDARY)
                    .child(percent.clone())
                    .probe_value("viewer-zoom", percent),
            )
            .child(
                IconButton::new("viewer-zoom-in", IconName::Plus)
                    .small()
                    .tooltip("Zoom in")
                    .focus_on_click(false)
                    .disabled(!ready || at_max)
                    .on_click(cx.listener(|this, _, _, cx| this.with_view(cx, |v| v.step(true))))
                    .probe("viewer-zoom-in"),
            );

        let preset = |id: &'static str, label: &'static str, on: bool| {
            Button::new(id, label).ghost().small().focus_on_click(false).disabled(!ready).selected(on)
        };

        row()
            .flex_none()
            .h(px(TOOLBAR_H))
            .w_full()
            .gap(px(SPACE_3))
            .px(px(SPACE_4))
            .bg(BG_SIDEBAR)
            .border_b_1()
            .border_color(LINE)
            .child(icon(IconName::Image, 14., TEXT_MUTED))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .type_style(BODY_STRONG)
                    .child(truncate(image.name.clone()))
                    .probe_value("viewer-name", image.name.clone()),
            )
            .children(position)
            .child(zoom)
            .child(
                row()
                    .flex_none()
                    .gap(px(SPACE_1))
                    .child(
                        preset("viewer-fit", "Fit", mode == Some(Mode::Fit))
                            .on_click(cx.listener(|this, _, _, cx| this.with_view(cx, Viewport::fit)))
                            .probe("viewer-fit"),
                    )
                    .child(
                        preset("viewer-fit-width", "Fit width", mode == Some(Mode::FitWidth))
                            .on_click(cx.listener(|this, _, _, cx| this.with_view(cx, Viewport::fit_width)))
                            .probe("viewer-fit-width"),
                    )
                    .child(
                        preset("viewer-actual", "100%", actual)
                            .on_click(cx.listener(|this, _, _, cx| this.with_view(cx, |v| v.zoom_to(1.0))))
                            .probe("viewer-actual"),
                    ),
            )
            .child(div().h(px(SPACE_5)).child(vr()))
            .child(
                IconButton::new("viewer-close", IconName::X)
                    .small()
                    .tooltip("Close")
                    .focus_on_click(false)
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    .probe("viewer-close"),
            )
    }

    fn picture(&self, image: Arc<RenderImage>) -> gpui::AnyElement {
        let Some(v) = self.viewport else { return div().into_any_element() };
        let d = v.drawn();
        let pannable = d.w > v.stage.w - 1. || d.h > v.stage.h - 1.;
        let cursor = match self.drag {
            Some(drag) if drag.moved => CursorStyle::ClosedHand,
            _ if pannable => CursorStyle::OpenHand,
            _ => CursorStyle::Arrow,
        };
        img(ImageSource::Render(image))
            .id("viewer-image")
            .absolute()
            .left(px(v.x))
            .top(px(v.y))
            .w(px(d.w))
            .h(px(d.h))
            .cursor(cursor)
            .probe("viewer-image")
            .into_any_element()
    }

    fn failed(&mut self, reason: SharedString, stage: Extent, cx: &mut Context<Self>) -> gpui::AnyElement {
        let image = &self.images[self.index];
        let what = match &image.source {
            ViewerSource::File(path) => path.display().to_string(),
            ViewerSource::Memory(_) => image.name.to_string(),
        };
        let text = format!("{what}\n{reason}");
        let md = self
            .errors
            .entry(self.index)
            .or_insert_with(|| cx.new(|cx| Markdown::verbatim(text, cx).type_style(MONO_SM).color(TEXT_SECONDARY)))
            .clone();
        let w = (stage.w - 4. * SPACE_8).clamp(240., 640.);
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(
                col()
                    .id("viewer-failed")
                    .debug_selector(|| "viewer-failed".into())
                    .occlude()
                    .w(px(w))
                    .gap(px(SPACE_2))
                    .p(px(SPACE_5))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE)
                    .child(
                        row()
                            .gap(px(SPACE_2))
                            .child(icon(IconName::Alert, 14., DANGER))
                            .child(div().type_style(BODY_STRONG).child("Could not load the image")),
                    )
                    .child(div().w_full().child(md))
                    .probe("viewer-failed"),
            )
            .into_any_element()
    }

    fn caption(&self, stage: Extent) -> Option<impl IntoElement> {
        let caption = self.images[self.index].caption.clone()?;
        Some(
            div().absolute().left_0().right_0().bottom(px(SPACE_4)).flex().justify_center().child(
                div()
                    .id("viewer-caption")
                    .occlude()
                    .flex()
                    .max_w(px((stage.w - 2. * SPACE_8).max(120.)))
                    .px(px(SPACE_3))
                    .py(px(SPACE_1))
                    .rounded(px(RADIUS_MD))
                    .border_1()
                    .border_color(LINE_STRONG)
                    .bg(SURFACE_RAISED)
                    .type_style(LABEL)
                    .text_color(TEXT_SECONDARY)
                    .child(truncate_lh(caption.clone(), LABEL.line))
                    .probe_value("viewer-caption", caption),
            ),
        )
    }
}

/// Hear every pointer move and release in the window while a press on the stage is held,
/// wherever the pointer goes (over the toolbar, the caption, out of the window).
fn follow_pointer(this: &WeakEntity<ImageViewer>, window: &mut Window) {
    let t = this.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            t.update(cx, |v, cx| v.pointer_moved(ev, window, cx)).ok();
        }
    });
    let t = this.clone();
    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left {
            t.update(cx, |v, cx| v.pointer_released(ev, window, cx)).ok();
        }
    });
}

/// The image at 100%: its pixels on screen pixels.
fn natural_size(image: &RenderImage, window: &Window) -> Extent {
    let s = image.size(0);
    let k = window.scale_factor().max(0.1);
    Extent::new(s.width.0 as f32 / k, s.height.0 as f32 / k)
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Built inside the overlay layer: remember which, to close it.
        if let Some(layer) = current_layer() {
            self.layer = Some(layer);
        }
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        let stage = Self::stage(window);
        let loaded = self.current(window, cx);
        if let Loaded::Ready(image) = &loaded {
            match self.viewport.as_mut() {
                Some(v) => v.resize(stage),
                None => {
                    let bottom = if self.images[self.index].caption.is_some() { CAPTION_ROOM } else { MARGIN };
                    self.viewport = Some(Viewport::new(stage, natural_size(image, window), bottom));
                }
            }
        }
        let body = match loaded {
            Loaded::Pending => div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child("Loading the image…")
                .into_any_element(),
            Loaded::Ready(image) => self.picture(image),
            Loaded::Failed(reason) => self.failed(reason, stage, cx),
        };
        let bounds = self.stage_bounds.clone();
        let this = cx.entity().downgrade();
        let stage_el = div()
            .id("viewer-stage")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::pointer_pressed))
            .on_scroll_wheel(cx.listener(Self::wheel))
            .child(
                canvas(move |b, _, _| bounds.set(Some(b)), move |_, _, window, _| follow_pointer(&this, window))
                    .absolute()
                    .size_full(),
            )
            .child(body)
            .children(self.caption(stage))
            .probe("viewer-stage");

        col()
            .id("image-viewer")
            .size_full()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.with_view(cx, |v| v.step(true))))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.with_view(cx, |v| v.step(false))))
            .on_action(cx.listener(|this, _: &ZoomFit, _, cx| this.with_view(cx, Viewport::fit)))
            .on_action(cx.listener(|this, _: &ZoomFitWidth, _, cx| this.with_view(cx, Viewport::fit_width)))
            .on_action(cx.listener(|this, _: &ZoomActual, _, cx| this.with_view(cx, |v| v.zoom_to(1.0))))
            .on_action(cx.listener(|this, _: &NextImage, _, cx| this.go(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousImage, _, cx| this.go(-1, cx)))
            .on_action(cx.listener(|this, _: &PanUp, _, cx| this.with_view(cx, |v| v.pan(0., KEY_PAN))))
            .on_action(cx.listener(|this, _: &PanDown, _, cx| this.with_view(cx, |v| v.pan(0., -KEY_PAN))))
            .child(self.toolbar(cx))
            .child(stage_el)
    }
}
