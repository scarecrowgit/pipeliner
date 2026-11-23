//! Form fields: a labelled control and the static look of an input.

use gpui::{
    AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Window, div,
    prelude::*, px,
};

use super::{Kbd, child_id};
use crate::assets::{IconName, icon};
use crate::layout::{col, row, truncate_lh};
use crate::motion;
use crate::theme::*;

/// A label above a control, 6px apart. The control is the child; a `hint` goes below it.
#[derive(IntoElement)]
pub struct Field {
    label: SharedString,
    hint: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Field {
    /// A field labelled `label`.
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), hint: None, children: Vec::new() }
    }

    /// Explanatory text below the control.
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl ParentElement for Field {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Field {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        col()
            .gap(px(SPACE_1_5))
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(self.label))
            .children(self.children)
            .when_some(self.hint, |d, h| {
                d.child(div().type_style(LABEL).text_color(TEXT_MUTED).child(h))
            })
    }
}

/// The static look of `.input`: a 40px `surface-raised` box with a `line-strong` border, an
/// optional leading icon, a placeholder or value, and an optional trailing key cap. `focused`
/// shows the accent border and glow.
///
/// This is the seam for the editable text input: add the input element as a child and it
/// replaces the placeholder and value inside the same frame, so a focused field only has to
/// call `.focused(true)`.
#[derive(IntoElement)]
pub struct InputFrame {
    id: ElementId,
    placeholder: Option<SharedString>,
    value: Option<SharedString>,
    icon: Option<IconName>,
    kbd: Option<SharedString>,
    small: bool,
    multiline: bool,
    focused: bool,
    danger: bool,
    sunken: bool,
    content: Vec<AnyElement>,
}

impl InputFrame {
    /// An empty input frame.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            placeholder: None,
            value: None,
            icon: None,
            kbd: None,
            small: false,
            multiline: false,
            focused: false,
            danger: false,
            sunken: false,
            content: Vec::new(),
        }
    }

    /// Muted text shown while there is no value.
    pub fn placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// Static text standing in for a typed value.
    pub fn value(mut self, text: impl Into<SharedString>) -> Self {
        self.value = Some(text.into());
        self
    }

    /// A 14px muted icon before the content.
    pub fn icon(mut self, name: IconName) -> Self {
        self.icon = Some(name);
        self
    }

    /// A key cap after the content ("⌘K").
    pub fn kbd(mut self, keys: impl Into<SharedString>) -> Self {
        self.kbd = Some(keys.into());
        self
    }

    /// 32px tall with 10px padding.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// The `.textarea` look: at least 72px tall, text from the top, 10/12 padding.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// The accent border and glow of a focused input.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// The `danger` border and halo of a type-to-confirm field for a destructive action.
    pub fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    /// `surface` instead of `surface-raised`: a field inside a popover (the search slot).
    pub fn sunken(mut self) -> Self {
        self.sunken = true;
        self
    }
}

impl ParentElement for InputFrame {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for InputFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = motion::fade_color(
            child_id(&self.id, "border"),
            match (self.danger, self.focused) {
                (true, _) => DANGER,
                (false, true) => ACCENT,
                (false, false) => LINE_STRONG,
            },
            window,
            cx,
        );
        let selector = self.id.to_string();
        let body: AnyElement = if !self.content.is_empty() {
            div().flex_1().min_w_0().children(self.content).into_any_element()
        } else if let Some(v) = self.value {
            if self.multiline {
                div().flex_1().min_w_0().child(v).into_any_element()
            } else {
                truncate_lh(v, BODY.line).text_color(TEXT).into_any_element()
            }
        } else if let Some(p) = self.placeholder {
            if self.multiline {
                div().flex_1().min_w_0().text_color(TEXT_MUTED).child(p).into_any_element()
            } else {
                truncate_lh(p, BODY.line).text_color(TEXT_MUTED).into_any_element()
            }
        } else {
            div().flex_1().into_any_element()
        };
        let frame = row().id(self.id).debug_selector(move || selector);
        let frame = if self.multiline {
            frame.items_start().min_h(px(72.)).px(px(SPACE_3)).py(px(SPACE_2_5))
        } else if self.small {
            frame.h(px(CONTROL_HEIGHT_MD)).px(px(SPACE_2_5))
        } else {
            frame.h(px(CONTROL_HEIGHT)).px(px(SPACE_3))
        };
        frame
            .gap(px(SPACE_2))
            .w_full()
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_LG))
            .bg(if self.sunken { SURFACE } else { SURFACE_RAISED })
            .text_color(TEXT)
            .type_style(BODY)
            .when(self.danger, |d| d.shadow(danger_glow()))
            .when(self.focused && !self.danger, |d| d.shadow(input_glow()))
            .when_some(self.icon, |d, i| d.child(icon(i, 14., TEXT_MUTED)))
            .child(body)
            .when_some(self.kbd, |d, k| d.child(Kbd::new(k)))
    }
}
