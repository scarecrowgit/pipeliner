use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, ease_in_out, linear_color_stop, linear_gradient, px, relative, Animation, AnimationExt as _,
    AnyElement, App, Div, ElementId, Hsla, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, Styled as _, Window,
};

use super::{icon_sized, AppIcon, Button};
use crate::theme::{self, TextStyles as _};

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

/// Bordered surface with 14px padding and a 10px gap between children (`.gr-card`).
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(14.))
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::LINE)
        .rounded(theme::RADIUS_XL)
}

/// Selected card look: accent ring + glow. Apply on top of [`card`].
pub fn card_selected(d: Div) -> Div {
    d.border_color(theme::ACCENT_RING).shadow(vec![gpui::BoxShadow {
        color: theme::ACCENT_GLOW,
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(3.),
    }])
}

/// Bordered surface without padding: tables, lists, sections with their own header row.
pub fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme::SURFACE)
        .border_1()
        .border_color(theme::LINE)
        .rounded(theme::RADIUS_XL)
        .overflow_hidden()
}

/// 1px horizontal rule.
pub fn divider() -> Div {
    div().h(px(1.)).w_full().flex_none().bg(theme::LINE)
}

/// 1px vertical rule.
pub fn vdivider() -> Div {
    div().w(px(1.)).h_full().flex_none().bg(theme::LINE)
}

/// A muted 12px label ("Work", "ALSO IN NEEDS YOU").
pub fn label(text: impl Into<SharedString>) -> Div {
    div().text_label().fg_muted().child(text.into())
}

/// Small icon tile used at the start of section headers and list rows.
pub fn icon_tile(icon: AppIcon, bg: Hsla, fg: Hsla) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(theme::RADIUS_MD)
        .bg(bg)
        .text_color(fg)
        .child(icon_sized(icon, theme::ICON_SM))
}

/// Label / value row (`Tests   14/14 passed`).
pub fn key_value(key: impl Into<SharedString>, value: impl IntoElement) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(theme::S3)
        .text_body()
        .child(div().w(px(96.)).flex_none().fg_muted().child(key.into()))
        .child(div().flex_1().min_w_0().fg().child(value))
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

/// Card / section header: optional icon tile, heading, muted meta, then a spacer so
/// anything you `.child()` afterwards sits on the right.
///
/// `SectionHeader::new("Live now").icon(AppIcon::Activity).meta("5 of 6 slots").child(link)`
#[derive(IntoElement)]
pub struct SectionHeader {
    title: SharedString,
    icon: Option<AppIcon>,
    meta: Option<SharedString>,
    right: Vec<AnyElement>,
}

impl SectionHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        SectionHeader { title: title.into(), icon: None, meta: None, right: vec![] }
    }
    pub fn icon(mut self, icon: AppIcon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }
}

impl ParentElement for SectionHeader {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.right.extend(elements)
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .min_h(px(28.))
            .when_some(self.icon, |d, i| d.child(icon_tile(i, theme::ACCENT_SOFT, theme::ACCENT_TEXT)))
            .child(div().text_heading().fg().whitespace_nowrap().child(self.title))
            .when_some(self.meta, |d, m| d.child(div().text_label().fg_muted().truncate().child(m)))
            .child(div().flex_1())
            .children(self.right)
    }
}

/// The bar at the top of every screen: title, muted subtitle, meta badges, then actions
/// on the right. 64px tall with a bottom border.
///
/// ```ignore
/// PageHeader::new("Review").meta(Badge::new("3 waiting").warning().dot())
///     .action(Button::new("ship", "Ship B").primary())
/// ```
#[derive(IntoElement)]
pub struct PageHeader {
    title: SharedString,
    subtitle: Option<SharedString>,
    meta: Vec<AnyElement>,
    actions: Vec<AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        PageHeader { title: title.into(), subtitle: None, meta: vec![], actions: vec![] }
    }
    pub fn subtitle(mut self, s: impl Into<SharedString>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    /// Left side, after the subtitle (status badges).
    pub fn meta(mut self, el: impl IntoElement) -> Self {
        self.meta.push(el.into_any_element());
        self
    }
    /// Right side, in order.
    pub fn action(mut self, el: impl IntoElement) -> Self {
        self.actions.push(el.into_any_element());
        self
    }
}

impl RenderOnce for PageHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme::S3)
            .h(theme::HEADER_H)
            .px(theme::S6)
            .border_b_1()
            .border_color(theme::LINE)
            .child(
                div()
                    .text_size(px(15.))
                    .line_height(px(20.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .fg()
                    .whitespace_nowrap()
                    .child(self.title),
            )
            .when_some(self.subtitle, |d, s| d.child(div().text_body().fg_muted().whitespace_nowrap().truncate().child(s)))
            .children(self.meta)
            .child(div().flex_1())
            .child(div().flex().items_center().gap(theme::S2).children(self.actions))
    }
}

// ---------------------------------------------------------------------------
// Empty and loading states
// ---------------------------------------------------------------------------

/// What every screen shows when it has nothing to show. Never fill a screen with
/// sample data; show this instead.
///
/// `EmptyState::new(AppIcon::Inbox, "Nothing waiting for you", "Gates, questions and approvals land here.")`
#[derive(IntoElement)]
pub struct EmptyState {
    icon: AppIcon,
    title: SharedString,
    description: SharedString,
    action: Option<AnyElement>,
    compact: bool,
    id: ElementId,
}

impl EmptyState {
    pub fn new(icon: AppIcon, title: impl Into<SharedString>, description: impl Into<SharedString>) -> Self {
        EmptyState {
            icon,
            title: title.into(),
            description: description.into(),
            action: None,
            compact: false,
            id: "empty-state".into(),
        }
    }
    /// Usually a [`Button`].
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
    /// Smaller padding, for use inside a card or column.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }
    /// "Not available yet": for features outside this release.
    pub fn not_available(what: impl Into<SharedString>) -> Self {
        Self::new(
            AppIcon::Lock,
            "Not available yet",
            format!("{} is not part of this release. Nothing here is hidden from you.", what.into()),
        )
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let pad = if self.compact { theme::S6 } else { px(64.) };
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(theme::S3)
            .py(pad)
            .px(theme::S6)
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(40.))
                    .rounded(theme::RADIUS_XL)
                    .bg(theme::SURFACE_RAISED)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .text_color(theme::TEXT_MUTED)
                    .child(icon_sized(self.icon, px(18.))),
            )
            .child(div().text_heading().fg().child(self.title))
            .child(div().max_w(px(420.)).text_body().fg_muted().text_center().child(self.description))
            .when_some(self.action, |d, a| d.child(div().pt(theme::S1).child(a)))
            .with_animation(
                self.id,
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                |d, t| d.opacity(t).mt(px(6.) * (1.0 - t)),
            )
    }
}

/// Convenience: an [`EmptyState`] whose action is a secondary button.
pub fn empty_with_button(
    icon: AppIcon,
    title: &'static str,
    description: &'static str,
    button: Button,
) -> EmptyState {
    EmptyState::new(icon, title, description).action(button)
}

/// Loading placeholder with a moving shimmer. Size it like the content it stands in for:
/// `Skeleton::new("sk-title").w(px(240.)).h(px(20.))`.
#[derive(IntoElement)]
pub struct Skeleton {
    id: ElementId,
    w: Option<Pixels>,
    h: Pixels,
    full: bool,
}

impl Skeleton {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Skeleton { id: id.into(), w: None, h: px(14.), full: true }
    }
    pub fn w(mut self, w: Pixels) -> Self {
        self.w = Some(w);
        self.full = false;
        self
    }
    pub fn h(mut self, h: Pixels) -> Self {
        self.h = h;
        self
    }
}

impl RenderOnce for Skeleton {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let hi = theme::alpha(theme::TEXT, 0.06);
        div()
            .relative()
            .flex_none()
            .h(self.h)
            .when_some(self.w, |d, w| d.w(w))
            .when(self.full, |d| d.w_full())
            .rounded(theme::RADIUS_MD)
            .bg(theme::SURFACE_RAISED)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(relative(0.6))
                    .flex()
                    .child(div().h_full().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(theme::alpha(hi, 0.0), 0.),
                        linear_color_stop(hi, 1.),
                    )))
                    .child(div().h_full().flex_1().bg(linear_gradient(
                        90.,
                        linear_color_stop(hi, 0.),
                        linear_color_stop(theme::alpha(hi, 0.0), 1.),
                    )))
                    .with_animation(
                        self.id,
                        Animation::new(Duration::from_millis(1400)).repeat().with_easing(ease_in_out),
                        |d, t| d.left(relative(-0.6 + 1.6 * t)),
                    ),
            )
    }
}

/// A few skeleton rows: a stand-in for a list while it loads.
pub fn skeleton_rows(id: &'static str, rows: usize) -> Div {
    div().flex().flex_col().gap(theme::S3).children((0..rows).map(move |i| {
        div()
            .flex()
            .flex_col()
            .gap(theme::S2)
            .child(Skeleton::new(ElementId::Name(format!("{id}-a{i}").into())).w(px(220.)).h(px(14.)))
            .child(Skeleton::new(ElementId::Name(format!("{id}-b{i}").into())).h(px(12.)))
    }))
}
