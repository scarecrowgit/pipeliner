//! A thumbnail that opens the image viewer when clicked.

use gpui::{
    AnyElement, App, ElementId, Hsla, Interactivity, IntoElement, MouseButton, RenderOnce, SharedString, Stateful,
    StyleRefinement, Window, div, prelude::*,
};
use ui::prelude::*;

use super::{ViewerImage, open_image_viewer};

/// A clickable frame around an image preview: a click opens the viewer on `images` at
/// `start`. It draws a 1px border that brightens on hover (150ms fade, `line-strong` to
/// `text-disabled`; change the resting colour with [`ImageThumb::border`]), a pointer cursor,
/// and a tooltip when given one. The click does not reach the elements under the thumbnail
/// (a card or row that opens something else).
///
/// Style it like a `div` (size, radius, background, `overflow_hidden`) and put the picture in
/// it, usually `img(path).size_full().object_fit(ObjectFit::Cover)`:
///
/// ```ignore
/// ImageThumb::new(("comment-image", ix), images.clone(), ix)
///     .w(px(160.)).h(px(100.)).rounded(px(RADIUS_MD)).overflow_hidden()
///     .child(img(path).size_full().object_fit(ObjectFit::Cover))
/// ```
///
/// Put a control that sits on top of the thumbnail (a remove button) after it with
/// `.occlude()`, so a press on the control does not also open the viewer.
#[derive(IntoElement)]
pub struct ImageThumb {
    el: Stateful<gpui::Div>,
    key: ElementId,
    images: Vec<ViewerImage>,
    start: usize,
    rest: Hsla,
    tooltip: Option<SharedString>,
}

impl ImageThumb {
    /// A thumbnail that opens `images` at `start`. `id` must be unique among its siblings.
    pub fn new(id: impl Into<ElementId>, images: Vec<ViewerImage>, start: usize) -> Self {
        let key: ElementId = id.into();
        let selector = key.to_string();
        Self {
            el: div().id(key.clone()).debug_selector(move || selector),
            key,
            images,
            start,
            rest: LINE_STRONG,
            tooltip: None,
        }
    }

    /// A thumbnail of one image.
    pub fn single(id: impl Into<ElementId>, image: ViewerImage) -> Self {
        Self::new(id, vec![image], 0)
    }

    /// The border colour at rest (`line-strong` by default; `line` on a quiet card).
    pub fn border(mut self, color: Hsla) -> Self {
        self.rest = color;
        self
    }

    /// A tooltip shown on hover ("Open image").
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
}

impl ParentElement for ImageThumb {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.el.extend(elements)
    }
}

impl Styled for ImageThumb {
    fn style(&mut self) -> &mut StyleRefinement {
        self.el.style()
    }
}

impl InteractiveElement for ImageThumb {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.el.interactivity()
    }
}

impl StatefulInteractiveElement for ImageThumb {}

impl RenderOnce for ImageThumb {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hovered = window.use_keyed_state(
            ElementId::Name(format!("{}-thumb-hover", self.key).into()),
            cx,
            |_, _| false,
        );
        let target = if *hovered.read(cx) { TEXT_DISABLED } else { self.rest };
        let border = motion::fade_color(
            ElementId::Name(format!("{}-thumb-border", self.key).into()),
            target,
            window,
            cx,
        );
        let (images, start) = (self.images, self.start);
        self.el
            .border_1()
            .border_color(border)
            .cursor_pointer()
            // Leave focus where it is, so it comes back there when the viewer closes.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_hover(move |h, _, cx| {
                hovered.update(cx, |v, cx| {
                    if *v != *h {
                        *v = *h;
                        cx.notify();
                    }
                })
            })
            .when_some(self.tooltip, |d, t| d.tooltip(ui::components::Tooltip::text(t)))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                open_image_viewer(images.clone(), start, window, cx);
            })
    }
}
