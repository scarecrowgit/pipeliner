//! Containers: cards, stat strips, banners, panel sections, key/value grids, notes, dividers.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div,
    prelude::*, px,
};

use super::{CountBadge, Status, StatusDot};
use crate::layout::{col, grow, row};
use crate::theme::*;

/// The basic container: `surface` fill, 1px `line` border, `radius-xl`, no shadow. Children
/// form the body (14px padding, 10px gap, or `flush` for none, e.g. around a table). Add a
/// [`CardHeader`] and a [`CardFooter`] with `header` and `footer`. A `selected` card swaps its
/// border to `accent-ring` and adds the `selected-glow` halo.
#[derive(IntoElement, Default)]
pub struct Card {
    header: Option<AnyElement>,
    footer: Option<AnyElement>,
    body: Vec<AnyElement>,
    selected: bool,
    flush: bool,
}

impl Card {
    /// An empty card.
    pub fn new() -> Self {
        Self::default()
    }

    /// The header row, usually a [`CardHeader`].
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// The footer row, usually a [`CardFooter`].
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// The selected look: accent ring and glow.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// No body padding or gap: the content brings its own.
    pub fn flush(mut self) -> Self {
        self.flush = true;
        self
    }
}

impl ParentElement for Card {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for Card {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        col()
            .bg(SURFACE)
            .border_1()
            .border_color(if self.selected { ACCENT_RING } else { LINE })
            .rounded(px(RADIUS_XL))
            .when(self.selected, |d| d.shadow(selected_glow()))
            .when_some(self.header, |d, h| d.child(h))
            .child(
                col()
                    .when(!self.flush, |d| d.gap(px(SPACE_2_5)).p(px(SPACE_3_5)))
                    .children(self.body),
            )
            .when_some(self.footer, |d, f| d.child(f))
    }
}

/// A card's header row: min 44px, a hairline below, an optional leading dot or tile, the
/// title in `HEADING`, an optional count, and meta pushed to the right in `LABEL` muted.
#[derive(IntoElement)]
pub struct CardHeader {
    title: SharedString,
    lead: Option<AnyElement>,
    count: Option<SharedString>,
    meta: Option<AnyElement>,
}

impl CardHeader {
    /// A header titled `title`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), lead: None, count: None, meta: None }
    }

    /// Something before the title: a [`Tile`](super::Tile) or an icon.
    pub fn lead(mut self, el: impl IntoElement) -> Self {
        self.lead = Some(el.into_any_element());
        self
    }

    /// A status dot before the title.
    pub fn dot(self, status: Status) -> Self {
        self.lead(StatusDot::new(status))
    }

    /// An accent count after the title.
    pub fn count(mut self, count: impl Into<SharedString>) -> Self {
        self.count = Some(count.into());
        self
    }

    /// Meta text or controls at the right edge.
    pub fn meta(mut self, meta: impl IntoElement) -> Self {
        self.meta = Some(meta.into_any_element());
        self
    }
}

impl RenderOnce for CardHeader {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        row()
            .flex_none()
            .gap(px(SPACE_2))
            .min_h(px(CARD_HEAD_HEIGHT))
            .px(px(SPACE_4))
            .border_b_1()
            .border_color(LINE)
            .when_some(self.lead, |d, l| d.child(l))
            .child(div().type_style(HEADING).child(self.title))
            .when_some(self.count, |d, c| d.child(CountBadge::new(c)))
            .child(grow())
            .when_some(self.meta, |d, m| d.child(div().type_style(LABEL).text_color(TEXT_MUTED).child(m)))
    }
}

/// A card's footer row: 40px, a hairline above, `LABEL` muted. Children are its content.
#[derive(IntoElement, Default)]
pub struct CardFooter {
    children: Vec<AnyElement>,
    bare: bool,
}

impl CardFooter {
    /// An empty footer.
    pub fn new() -> Self {
        Self::default()
    }

    /// No hairline above (the content above already ends in one).
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }
}

impl ParentElement for CardFooter {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for CardFooter {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        row()
            .flex_none()
            .gap(px(SPACE_2))
            .h(px(CARD_FOOT_HEIGHT))
            .px(px(SPACE_4))
            .when(!self.bare, |d| d.border_t_1().border_color(LINE))
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .children(self.children)
    }
}

/// A label above a number: the figure a strip or a card is about. `large` is the 28px `STAT`.
/// `sub` carries the unit or the limit ("of $10.00"); `warning` colours the value only when
/// someone has to act; `spawn` colours the sub.
#[derive(IntoElement)]
pub struct Stat {
    label: SharedString,
    value: SharedString,
    sub: Option<SharedString>,
    large: bool,
    warning: bool,
    spawn: bool,
}

impl Stat {
    /// A stat with a `label` and a `value`.
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self { label: label.into(), value: value.into(), sub: None, large: false, warning: false, spawn: false }
    }

    /// A muted unit or limit after the value ("of $10.00").
    pub fn suffix(mut self, sub: impl Into<SharedString>) -> Self {
        self.sub = Some(sub.into());
        self
    }

    /// The 28px figure.
    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    /// The value in `warning`: something needs a human.
    pub fn warning(mut self) -> Self {
        self.warning = true;
        self
    }

    /// The sub in `spawn-text`: it counts agent-made things.
    pub fn spawn(mut self) -> Self {
        self.spawn = true;
        self
    }
}

impl RenderOnce for Stat {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        col()
            .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(self.label))
            .child(
                row()
                    .items_baseline()
                    .mt(px(SPACE_1))
                    .gap(px(SPACE_1))
                    .child(
                        div()
                            .type_style(if self.large { STAT } else { STAT_SM })
                            .tabular()
                            .text_color(if self.warning { WARNING } else { TEXT })
                            .child(self.value),
                    )
                    .when_some(self.sub, |d, s| {
                        d.child(
                            div()
                                .type_style(LABEL)
                                .text_color(if self.spawn { SPAWN_TEXT } else { TEXT_MUTED })
                                .child(s),
                        )
                    }),
            )
    }
}

/// Stats side by side in one frame, divided by hairlines (never one box per stat).
#[derive(IntoElement, Default)]
pub struct StatStrip {
    stats: Vec<Stat>,
}

impl StatStrip {
    /// An empty strip.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a cell.
    pub fn stat(mut self, stat: Stat) -> Self {
        self.stats.push(stat);
        self
    }
}

impl RenderOnce for StatStrip {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .children(self.stats.into_iter().enumerate().map(|(ix, stat)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .px(px(SPACE_4))
                    .py(px(SPACE_3_5))
                    .when(ix > 0, |d| d.border_l_1().border_color(LINE))
                    .child(stat)
            }))
    }
}

/// The colour family of a [`Banner`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BannerTone {
    /// `surface` with a `line-strong` border.
    #[default]
    Neutral,
    /// Waiting for a human.
    Warning,
    /// Failed or denied.
    Danger,
    /// A spawn request.
    Spawn,
}

/// A full-width notice: icon, text and actions in a row (12px gap, 8px radius). Children are
/// its content.
#[derive(IntoElement, Default)]
pub struct Banner {
    tone: BannerTone,
    children: Vec<AnyElement>,
}

impl Banner {
    /// A neutral banner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the colour family.
    pub fn tone(mut self, tone: BannerTone) -> Self {
        self.tone = tone;
        self
    }
}

impl ParentElement for Banner {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Banner {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (bg, border) = match self.tone {
            BannerTone::Neutral => (SURFACE, LINE_STRONG),
            BannerTone::Warning => (WARNING_SOFT, BANNER_WARNING_LINE),
            BannerTone::Danger => (DANGER_SOFT, BANNER_DANGER_LINE),
            BannerTone::Spawn => (SPAWN_SOFT, SPAWN_LINE),
        };
        row()
            .gap(px(SPACE_3))
            .px(px(SPACE_3_5))
            .py(px(SPACE_3))
            .border_1()
            .border_color(border)
            .rounded(px(RADIUS_LG))
            .bg(bg)
            .children(self.children)
    }
}

/// A section of a side panel (`.psec`): 16/20 padding, a hairline below, a header row with
/// the title and trailing meta, then the content. `last` drops the hairline.
#[derive(IntoElement)]
pub struct Section {
    title: SharedString,
    meta: Option<AnyElement>,
    last: bool,
    children: Vec<AnyElement>,
}

impl Section {
    /// A section titled `title`.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), meta: None, last: false, children: Vec::new() }
    }

    /// Meta text or a link at the right edge of the header.
    pub fn meta(mut self, meta: impl IntoElement) -> Self {
        self.meta = Some(meta.into_any_element());
        self
    }

    /// The last section of a panel: no hairline below.
    pub fn last(mut self) -> Self {
        self.last = true;
        self
    }
}

impl ParentElement for Section {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Section {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        col()
            .flex_none()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .when(!self.last, |d| d.border_b_1().border_color(LINE))
            .child(
                row()
                    .gap(px(SPACE_2))
                    .min_h(px(20.))
                    .mb(px(SPACE_2_5))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .child(div().type_style(BODY_STRONG).text_color(TEXT).child(self.title))
                    .child(grow())
                    .when_some(self.meta, |d, m| d.child(m)),
            )
            .children(self.children)
    }
}

/// A two-column list of facts: a muted 88px key, a value (`.kv`). Rows are 6px apart, columns
/// 12px.
#[derive(IntoElement, Default)]
pub struct KeyValue {
    rows: Vec<(SharedString, AnyElement, bool)>,
    key_width: Option<f32>,
}

impl KeyValue {
    /// An empty list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a row: the key and its value.
    pub fn row(mut self, key: impl Into<SharedString>, value: impl IntoElement) -> Self {
        self.rows.push((key.into(), value.into_any_element(), false));
        self
    }

    /// Add a row whose key is machine text (a tool or linter name): mono.
    pub fn mono_row(mut self, key: impl Into<SharedString>, value: impl IntoElement) -> Self {
        self.rows.push((key.into(), value.into_any_element(), true));
        self
    }

    /// Width of the key column in px (default 88).
    pub fn key_width(mut self, px: f32) -> Self {
        self.key_width = Some(px);
        self
    }
}

impl RenderOnce for KeyValue {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let key_width = self.key_width.unwrap_or(88.);
        col().gap(px(SPACE_1_5)).children(self.rows.into_iter().map(move |(key, value, mono)| {
            row()
                .gap(px(SPACE_3))
                .child(
                    div()
                        .w(px(key_width))
                        .flex_none()
                        .type_style(if mono { MONO } else { LABEL })
                        .text_color(TEXT_MUTED)
                        .child(key),
                )
                .child(div().flex_1().min_w_0().child(value))
        }))
    }
}

/// Small explanatory text under a control or section: `LABEL` in `text-muted`.
#[derive(IntoElement)]
pub struct Note {
    text: SharedString,
}

impl Note {
    /// A note reading `text`.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for Note {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().type_style(LABEL).text_color(TEXT_MUTED).child(self.text)
    }
}

/// A 1px rule. Horizontal across its parent by default; `vertical` down it.
#[derive(IntoElement, Default)]
pub struct Divider {
    vertical: bool,
    subtle: bool,
    strong: bool,
}

impl Divider {
    /// A horizontal `line` rule.
    pub fn new() -> Self {
        Self::default()
    }

    /// A vertical rule.
    pub fn vertical(mut self) -> Self {
        self.vertical = true;
        self
    }

    /// `line-subtle`: inside a card.
    pub fn subtle(mut self) -> Self {
        self.subtle = true;
        self
    }

    /// `line-strong`: inside a popover.
    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

impl RenderOnce for Divider {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let color = if self.strong {
            LINE_STRONG
        } else if self.subtle {
            LINE_SUBTLE
        } else {
            LINE
        };
        let rule = div().flex_none().bg(color);
        if self.vertical { rule.w(px(1.)).h_full() } else { rule.h(px(1.)).w_full() }
    }
}
