//! [`DiffCard`]: a file's diff in a card with a header (path and counts), optional trailing
//! controls and footer, a collapsible body and a row limit behind "Show all N lines".

use std::sync::Arc;

use gpui::{
    AnyElement, App, ElementId, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};
use ui::assets::IconName;
use ui::components::{IconButton, Link};
use ui::layout::{col, grow, row};
use ui::theme::*;

use super::model::{DiffFile, visible_rows};
use super::view::DiffView;

/// A diff in a card, as the chat's edit card and the Inbox review draw it: a header row (an
/// optional leading tile, an optional label such as `edit`, the path in mono, `+18 −4`,
/// trailing controls such as "1 of 3 files" and a next-file button), the [`DiffView`], and an
/// optional footer row (the LSP line). The body can collapse, and long diffs show their
/// first rows behind "Show all N lines".
///
/// ```ignore
/// DiffCard::new("edit-3", file.clone()).label("edit").compact().collapsible().collapse_after(12)
///     .footer(row().gap(px(SPACE_2)).child(StatusDot::new(Status::Live)).child("LSP · typescript · 0 errors"))
/// DiffCard::new("variant-a", file.clone()).lead(Tile::new(..)).pager(0, 3, |_, _, _| next())
/// ```
#[derive(IntoElement)]
pub struct DiffCard {
    id: ElementId,
    file: Arc<DiffFile>,
    label: Option<SharedString>,
    lead: Option<AnyElement>,
    trailing: Vec<AnyElement>,
    footer: Option<AnyElement>,
    collapsible: bool,
    limit: Option<usize>,
    max_height: f32,
    compact: bool,
}

/// Whether the card is open and whether "Show all" was clicked, kept across frames.
struct CardState {
    open: bool,
    expanded: bool,
}

impl DiffCard {
    /// A card for `file`; `id` must be unique among its siblings.
    pub fn new(id: impl Into<ElementId>, file: impl Into<Arc<DiffFile>>) -> Self {
        Self {
            id: id.into(),
            file: file.into(),
            label: None,
            lead: None,
            trailing: Vec::new(),
            footer: None,
            collapsible: false,
            limit: None,
            max_height: 600.,
            compact: false,
        }
    }

    /// A muted mono word before the path, e.g. the tool name `edit`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Something before everything else in the header: a [`Tile`](ui::components::Tile).
    pub fn lead(mut self, el: impl IntoElement) -> Self {
        self.lead = Some(el.into_any_element());
        self
    }

    /// A control or label at the right end of the header; call again for more.
    pub fn trailing(mut self, el: impl IntoElement) -> Self {
        self.trailing.push(el.into_any_element());
        self
    }

    /// "1 of 3 files" and a next-file button (`index` counts from 0).
    pub fn pager(
        self,
        index: usize,
        total: usize,
        on_next: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        let label = format!(
            "{} of {}",
            index + 1,
            base::fmt::plural(total as u64, "file", "files")
        );
        let next = IconButton::new(
            ElementId::NamedChild(Box::new(self.id.clone()), "next".into()),
            IconName::ChevronDown,
        )
        .small()
        .tooltip("Next file")
        .disabled(index + 1 >= total)
        .on_click(on_next);
        self.trailing(div().type_style(LABEL).text_color(TEXT_MUTED).child(label))
            .trailing(next)
    }

    /// The footer row's content, e.g. the LSP status line.
    pub fn footer(mut self, el: impl IntoElement) -> Self {
        self.footer = Some(el.into_any_element());
        self
    }

    /// A chevron in the header folds the body away.
    pub fn collapsible(mut self) -> Self {
        self.collapsible = true;
        self
    }

    /// Show only the first `rows` rows until "Show all N lines" is clicked.
    pub fn collapse_after(mut self, rows: usize) -> Self {
        self.limit = Some(rows);
        self
    }

    /// Scroll inside past this height (default 600 px).
    pub fn max_height(mut self, px: f32) -> Self {
        self.max_height = px;
        self
    }

    /// The chat's 36 px header with 12 px padding (default: the review's 40 px and 14 px).
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl RenderOnce for DiffCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.with_id(self.id.clone(), |window| {
            window.use_keyed_state("diff-card", cx, |_, _| CardState {
                open: true,
                expanded: false,
            })
        });
        let (open, expanded) = {
            let s = state.read(cx);
            (s.open || !self.collapsible, s.expanded)
        };
        let child =
            |name: &'static str| ElementId::NamedChild(Box::new(self.id.clone()), name.into());
        let file = &self.file;
        let path = match &file.old_path {
            Some(old) if *old != file.path => format!("{old} → {}", file.path),
            _ => file.path.clone(),
        };
        let counts = (!file.binary).then(|| {
            row()
                .flex_none()
                .gap(px(SPACE_1))
                .type_style(MONO)
                .child(div().text_color(SUCCESS).child(format!("+{}", file.added)))
                .child(div().text_color(DANGER).child(format!("−{}", file.removed)))
        });
        let toggle = self.collapsible.then(|| {
            let st = state.clone();
            IconButton::new(
                child("toggle"),
                if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
            )
            .small()
            .tooltip(if open { "Collapse" } else { "Expand" })
            .on_click(move |_, _, cx| {
                st.update(cx, |s, cx| {
                    s.open = !s.open;
                    cx.notify();
                })
            })
        });
        let header = row()
            .flex_none()
            .gap(px(SPACE_2))
            .h(px(if self.compact { 36. } else { 40. }))
            .pl(px(if self.compact { SPACE_3 } else { SPACE_3_5 }))
            .pr(px(if self.compact { SPACE_1 } else { SPACE_2 }))
            .when(open, |d| d.border_b_1().border_color(LINE))
            .when_some(self.lead, |d, l| d.child(l))
            .when_some(self.label, |d, l| {
                d.child(
                    div()
                        .flex_none()
                        .type_style(MONO)
                        .text_color(TEXT_MUTED)
                        .child(l),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .flex_shrink()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .type_style(MONO)
                    .text_color(TEXT)
                    .child(path),
            )
            .children(counts)
            .child(grow())
            .children(self.trailing)
            .children(toggle);

        let lines = file.line_count();
        let (_, hidden) = visible_rows(lines, self.limit, expanded);
        let limited = self.limit.filter(|_| hidden);
        let body = open.then(|| {
            DiffView::new(child("view"), self.file.clone())
                .limit(limited)
                .max_height(self.max_height)
        });
        let show_all = (open && limited.is_some()).then(|| {
            let st = state.clone();
            row()
                .flex_none()
                .h(px(CONTROL_HEIGHT_MD))
                .px(px(SPACE_3))
                .bg(BG_SIDEBAR)
                .border_t_1()
                .border_color(LINE_SUBTLE)
                .type_style(LABEL)
                .child(
                    Link::new(
                        child("show-all"),
                        format!(
                            "Show all {}",
                            base::fmt::plural(lines as u64, "line", "lines")
                        ),
                    )
                    .on_click(move |_, _, cx| {
                        st.update(cx, |s, cx| {
                            s.expanded = true;
                            cx.notify();
                        })
                    }),
                )
        });
        let footer = self.footer.map(|f| {
            row()
                .flex_none()
                .gap(px(SPACE_2))
                .h(px(CONTROL_HEIGHT_MD))
                .px(px(SPACE_3))
                .border_t_1()
                .border_color(LINE)
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(f)
        });
        col()
            .w_full()
            .flex_none()
            .bg(SURFACE)
            .border_1()
            .border_color(LINE)
            .rounded(px(RADIUS_XL))
            .overflow_hidden()
            .child(header)
            .children(body)
            .children(show_all)
            .children(footer)
    }
}
