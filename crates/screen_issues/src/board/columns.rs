//! The board: one column per status (`board()` of the mockups), cards that drag between
//! columns, and the arrow keys that walk the cards.

use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{Issue, IssueKey, IssuePatch, IssueStatus};
use ui::dev::Probe;
use ui::prelude::*;

use super::IssuesView;
use super::card::{CARD_WIDTH, DraggedIssue};

/// The id and probe suffix of a status: `in-progress`.
pub(crate) fn status_slug(status: IssueStatus) -> &'static str {
    match status {
        IssueStatus::Backlog => "backlog",
        IssueStatus::Todo => "todo",
        IssueStatus::InProgress => "in-progress",
        IssueStatus::InReview => "in-review",
        IssueStatus::Done => "done",
    }
}

/// The dot before a column's name.
pub(crate) fn status_dot(status: IssueStatus) -> Status {
    match status {
        IssueStatus::Backlog | IssueStatus::Todo => Status::Draft,
        IssueStatus::InProgress => Status::Running,
        IssueStatus::InReview => Status::Waiting,
        IssueStatus::Done => Status::Live,
    }
}

/// "1 needs you", "2 need you".
pub(crate) fn need_label(n: usize) -> Option<String> {
    match n {
        0 => None,
        1 => Some("1 needs you".to_string()),
        n => Some(format!("{n} need you")),
    }
}

/// One column of the board as shown: its status and its cards after the filters.
pub(crate) struct Column {
    pub status: IssueStatus,
    pub cards: Vec<Arc<Issue>>,
    /// Issues of the column whose latest run waits for a person.
    pub needs_you: usize,
}

impl IssuesView {
    /// The columns, in the mockup's order, each with the issues that pass the filters.
    pub(crate) fn columns(&self, cx: &gpui::App) -> Vec<Column> {
        let issues = self.issues.read(cx);
        let runs = self.runs.read(cx);
        IssueStatus::ALL
            .into_iter()
            .map(|status| {
                let cards: Vec<Arc<Issue>> =
                    issues.column(status).into_iter().filter(|issue| self.filters.matches(issue)).collect();
                let needs_you = cards
                    .iter()
                    .filter(|issue| runs.latest_for(&issue.key).is_some_and(|run| run.state.needs_person()))
                    .count();
                Column { status, cards, needs_you }
            })
            .collect()
    }

    /// Moves `key` to `status` by sending the status patch; what that means is the engine's
    /// business.
    pub(crate) fn move_issue(&mut self, key: &IssueKey, status: IssueStatus, cx: &mut Context<Self>) {
        let known = self.issues.read(cx).issue(key).map(|issue| issue.status);
        if known.is_none_or(|from| from == status) {
            return;
        }
        let patch = IssuePatch { status: Some(status), ..IssuePatch::default() };
        drop(hub::commands(cx).update_issue(key, patch, cx));
        self.cursor = Some(key.clone());
        cx.notify();
    }

    /// The board: columns side by side in a scroll area.
    pub(crate) fn render_board(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let columns = self.columns(cx);
        let columns: Vec<AnyElement> =
            columns.into_iter().map(|column| self.render_column(column, window, cx)).collect();
        div()
            .id("issues-board")
            .size_full()
            .overflow_scroll()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .child(div().flex().flex_row().flex_none().min_h_full().items_start().gap(px(SPACE_3)).children(columns))
            .into_any_element()
    }

    fn render_column(&mut self, column: Column, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let status = column.status;
        let slug = status_slug(status);
        let name = status.label();
        let cards: Vec<AnyElement> = column.cards.iter().map(|issue| self.render_card(issue, window, cx)).collect();
        let head = row()
            .h(px(28.))
            .px(px(SPACE_1))
            .gap(px(SPACE_2))
            .child(StatusDot::new(status_dot(status)).small())
            .child(div().flex_none().type_style(BODY_STRONG).child(name))
            .child(div().flex_none().text_color(TEXT_MUTED).tabular().child(column.cards.len().to_string()))
            .when_some(need_label(column.needs_you), |d, need| {
                d.child(div().flex_none().type_style(LABEL).text_color(WARNING).child(need))
            })
            .child(grow())
            .child(
                IconButton::new(SharedString::from(format!("issues-add-{slug}")), IconName::Plus)
                    .small()
                    .tooltip(format!("Add issue to {name}"))
                    .on_click(cx.listener(move |this, _, window, cx| this.new_issue(Some(status), window, cx))),
            );
        let add = div().flex().child(
            Button::new(SharedString::from(format!("issues-add-bottom-{slug}")), "Add issue")
                .ghost()
                .small()
                .icon(IconName::Plus)
                .on_click(cx.listener(move |this, _, window, cx| this.new_issue(Some(status), window, cx))),
        );
        let probe = format!("issues-col-{slug}");
        let selector = probe.clone();
        col()
            .id(SharedString::from(probe.clone()))
            .debug_selector(move || selector)
            .flex_none()
            .w(px(CARD_WIDTH))
            .gap(px(SPACE_2))
            .p(px(SPACE_1))
            .m(px(-SPACE_1))
            .pb(px(SPACE_8))
            .border_1()
            .border_color(TRANSPARENT)
            .rounded(px(RADIUS_XL))
            .drag_over::<DraggedIssue>(move |style, dragged, _, _| {
                if dragged.status == status { style } else { style.bg(SURFACE).border_color(ACCENT_RING) }
            })
            .on_drop(cx.listener(move |this, dragged: &DraggedIssue, _, cx| this.move_issue(&dragged.key, status, cx)))
            .child(head)
            .children(cards)
            .child(add)
            .probe(probe)
            .into_any_element()
    }

    // ------------------------------------------------------------ keyboard

    /// The keys of the cards in walking order: one list per board column, or the list's rows
    /// as one column.
    pub(crate) fn walk(&self, cx: &gpui::App) -> Vec<Vec<IssueKey>> {
        if self.layout(cx) == super::Layout::List {
            return vec![self.rows(cx).into_iter().map(|issue| issue.key.clone()).collect()];
        }
        self.columns(cx).into_iter().map(|column| column.cards.into_iter().map(|issue| issue.key.clone()).collect()).collect()
    }

    /// Moves the selection `dx` columns and `dy` cards; the first card when nothing is
    /// selected.
    pub(crate) fn move_cursor(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let walk = self.walk(cx);
        let at = self.cursor.as_ref().and_then(|key| {
            walk.iter().enumerate().find_map(|(c, keys)| keys.iter().position(|k| k == key).map(|r| (c, r)))
        });
        let next = match at {
            None => walk.iter().find_map(|keys| keys.first()).cloned(),
            Some((c, r)) if dx != 0 => {
                let mut target = c as isize + dx;
                while target >= 0 && (target as usize) < walk.len() && walk[target as usize].is_empty() {
                    target += dx.signum();
                }
                if target < 0 || target as usize >= walk.len() {
                    None
                } else {
                    let keys = &walk[target as usize];
                    keys.get(r.min(keys.len() - 1)).cloned()
                }
            }
            Some((c, r)) => {
                let keys = &walk[c];
                let row = (r as isize + dy).clamp(0, keys.len() as isize - 1) as usize;
                keys.get(row).cloned()
            }
        };
        if let Some(key) = next {
            self.cursor = Some(key);
            cx.notify();
        }
    }

    /// Moves the selection to the first or last card of its column.
    pub(crate) fn cursor_to_end(&mut self, last: bool, cx: &mut Context<Self>) {
        let walk = self.walk(cx);
        let column = self
            .cursor
            .as_ref()
            .and_then(|key| walk.iter().find(|keys| keys.contains(key)))
            .or_else(|| walk.iter().find(|keys| !keys.is_empty()));
        let key = column.and_then(|keys| if last { keys.last() } else { keys.first() }).cloned();
        if key.is_some() {
            self.cursor = key;
            cx.notify();
        }
    }
}
