//! The popover surface (`.popover`): the panel menus, pickers and small forms float in.

use gpui::{AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use crate::components::KeyHints;
use crate::layout::{col, grow, row};
use crate::theme::*;

/// A floating panel: 4px padding, a `line-strong` border, `radius-xl`, `surface-raised`.
/// Optional parts, top to bottom: a header (`.pop-h`: title, sub-line, something at the
/// right), a search slot, the children (usually a [`Menu`](crate::menu::Menu) or
/// [`MenuItem`](crate::components::MenuItem)s with [`PopLabel`](crate::components::PopLabel)s
/// and [`PopSeparator`](crate::components::PopSeparator)s), and a footer of key hints
/// (`.pop-foot`). Open it with [`Overlays::open_popover`](crate::overlay::Overlays::open_popover).
#[derive(IntoElement, Default)]
pub struct Popover {
    width: Option<f32>,
    title: Option<SharedString>,
    sub: Option<SharedString>,
    trailing: Option<AnyElement>,
    search: Option<AnyElement>,
    footer: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl Popover {
    /// An empty popover that sizes to its content.
    pub fn new() -> Self {
        Self::default()
    }

    /// A fixed width in px (the mockups use 280 to 420).
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }

    pub(crate) fn width_opt(mut self, px: Option<f32>) -> Self {
        self.width = px.or(self.width);
        self
    }

    /// A header: title in 13/20 medium.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// A muted line under the title.
    pub fn subtitle(mut self, sub: impl Into<SharedString>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// Something at the right of the title ("2 on · 1 paused").
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing = Some(el.into_any_element());
        self
    }

    /// A search field above the content, e.g. a small
    /// [`InputFrame`](crate::components::InputFrame) with `.sunken()`.
    pub fn search(mut self, el: impl IntoElement) -> Self {
        self.search = Some(el.into_any_element());
        self
    }

    /// A footer of key hints under a `line-strong` rule.
    pub fn footer(mut self, hints: KeyHints) -> Self {
        self.footer = Some(hints.into_any_element());
        self
    }
}

impl ParentElement for Popover {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Popover {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let header = self.title.map(|title| {
            col()
                .px(px(SPACE_2_5))
                .pt(px(SPACE_2_5))
                .pb(px(SPACE_2))
                .child(
                    row()
                        .gap(px(SPACE_2))
                        .child(div().type_style(BODY_STRONG).child(title))
                        .when_some(self.trailing, |d, t| {
                            d.child(grow()).child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(t))
                        }),
                )
                .when_some(self.sub, |d, s| {
                    d.child(div().mt(px(SPACE_0_5)).type_style(LABEL).text_color(TEXT_MUTED).child(s))
                })
        });
        let footer = self.footer.map(|hints| {
            row()
                .mt(px(SPACE_1))
                .mx(px(-SPACE_1))
                .px(px(SPACE_3_5))
                .pt(px(SPACE_2))
                .pb(px(SPACE_1))
                .border_t_1()
                .border_color(LINE_STRONG)
                .child(hints)
        });
        col()
            .flex_none()
            .when_some(self.width, |d, w| d.w(px(w)))
            .p(px(SPACE_1))
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_XL))
            .bg(SURFACE_RAISED)
            .text_color(TEXT)
            .type_style(BODY)
            .children(header)
            .when_some(self.search, |d, s| d.child(div().mt(px(SPACE_0_5)).mb(px(SPACE_1)).child(s)))
            .children(self.children)
            .children(footer)
    }
}
