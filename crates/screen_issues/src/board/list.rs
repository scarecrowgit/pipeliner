//! The list view: the same issues as the board, in a table, in the order the sort menu picks.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui::{AnyElement, App, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::Issue;
use ui::dev::Probe;
use ui::prelude::*;

use super::IssuesView;
use super::card::{initials, priority_color, runner_kind, strip};
use super::columns::status_dot;

/// Probe and anchor of the sort select.
pub(crate) const SORT: &str = "issues-sort";

/// What the list is ordered by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SortBy {
    /// Status in board order, then higher priority, then most recently changed.
    #[default]
    Status,
    Priority,
    Key,
    Title,
    Owner,
    Updated,
}

impl SortBy {
    /// Every order, as the menu lists them.
    pub(crate) const ALL: [SortBy; 6] =
        [SortBy::Status, SortBy::Priority, SortBy::Updated, SortBy::Key, SortBy::Title, SortBy::Owner];

    /// The menu's word.
    pub(crate) fn label(self) -> &'static str {
        match self {
            SortBy::Status => "Status",
            SortBy::Priority => "Priority",
            SortBy::Key => "Key",
            SortBy::Title => "Title",
            SortBy::Owner => "Owner",
            SortBy::Updated => "Updated",
        }
    }

    /// Orders two issues; ties fall back to the board's order (priority, then newest).
    pub(crate) fn compare(self, a: &Issue, b: &Issue) -> Ordering {
        let board = || b.priority.cmp(&a.priority).then_with(|| b.updated.cmp(&a.updated));
        let first = match self {
            SortBy::Status => a.status.cmp(&b.status),
            SortBy::Priority => b.priority.cmp(&a.priority),
            SortBy::Key => key_number(&a.key.to_string()).cmp(&key_number(&b.key.to_string())),
            SortBy::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortBy::Owner => a.owner.to_lowercase().cmp(&b.owner.to_lowercase()),
            SortBy::Updated => b.updated.cmp(&a.updated),
        };
        first.then_with(board).then_with(|| a.key.cmp(&b.key))
    }
}

/// `PAY-1423` sorts as ("PAY", 1423), so 99 comes before 100.
fn key_number(key: &str) -> (String, u64) {
    match key.rsplit_once('-') {
        Some((prefix, n)) => (prefix.to_string(), n.parse().unwrap_or(0)),
        None => (key.to_string(), 0),
    }
}

impl IssuesView {
    /// The issues that pass the filters, in the list's order.
    pub(crate) fn rows(&self, cx: &App) -> Vec<Arc<Issue>> {
        let sort = self.sort(cx);
        let mut rows: Vec<Arc<Issue>> =
            self.issues.read(cx).iter().filter(|issue| self.filters.matches(issue)).cloned().collect();
        rows.sort_by(|a, b| sort.compare(a, b));
        rows
    }

    /// The list view.
    pub(crate) fn render_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = base::time::now_ms();
        let offset = base::time::utc_offset(now);
        // Beside the open panel there is room for the essentials only.
        let wide = !self.has_panel();
        let mut table = Table::new("issues-table")
            .column(Column::new("Key").fixed(104.))
            .column(Column::new("Title").flex(3.))
            .column(Column::new("Status").fixed(128.))
            .column(Column::new("Priority").fixed(88.));
        if wide {
            table = table.column(Column::new("Type").fixed(80.));
        }
        table = table.column(Column::new("Owner").fixed(if wide { 150. } else { 120. }));
        if wide {
            table = table.column(Column::new("Runs with").flex(2.));
        }
        table = table.column(Column::new("Updated").fixed(84.).right());
        for issue in self.rows(cx) {
            table = table.row(self.list_row(&issue, wide, now, offset, cx));
        }
        div()
            .id("issues-list")
            .size_full()
            .overflow_y_scroll()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .child(div().flex_none().w_full().child(table))
            .into_any_element()
    }

    fn list_row(&self, issue: &Arc<Issue>, wide: bool, now: i64, offset: i32, cx: &mut Context<Self>) -> TableRow {
        let key = issue.key.clone();
        let probe = format!("issues-row-{key}");
        let run = self.runs.read(cx).latest_for(&key).cloned();
        let line = strip(issue, run.as_deref());
        let runner = issue.runner.as_ref().map(|runner| {
            let (glyph, _) = runner_kind(runner);
            row()
                .w_full()
                .gap(px(SPACE_2))
                .child(icon(glyph, 14., TEXT_MUTED))
                .child(truncate(self.runner_name(runner, cx)))
                .when_some(line, |d, line| {
                    d.child(div().flex_none().child(StatusDot::new(line.tone.status()).small()))
                })
        });
        let owner = row()
            .w_full()
            .gap(px(SPACE_2))
            .when(!issue.owner.is_empty(), |d| d.child(Avatar::new(initials(&issue.owner))))
            .child(truncate(if issue.owner.is_empty() { "No owner".to_string() } else { issue.owner.clone() }));
        let tr = TableRow::new(SharedString::from(probe.clone()))
            .selected(self.cursor.as_ref() == Some(&key))
            .cell(div().type_style(MONO).text_color(TEXT_MUTED).child(key.to_string()).probe(probe))
            .strong(issue.title.clone())
            .cell(
                row()
                    .gap(px(SPACE_2))
                    .child(StatusDot::new(status_dot(issue.status)).small())
                    .child(div().flex_none().child(issue.status.label())),
            )
            .cell(div().text_color(priority_color(issue.priority)).child(issue.priority.label()));
        let tr = if wide { tr.cell(Tag::new(issue.kind.label())) } else { tr };
        let tr = tr.cell(owner);
        let tr = if wide {
            tr.cell(runner.map_or_else(
                || div().text_color(TEXT_MUTED).child("Nothing attached").into_any_element(),
                IntoElement::into_any_element,
            ))
        } else {
            tr
        };
        tr.cell(
                div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(base::time::ago(now, issue.updated, offset)),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.open_issue(&key, window, cx)))
    }
}

/// The sort menu, opened from the sort select.
pub(crate) fn sort_menu(view: &gpui::Entity<IssuesView>, cx: &App) -> Menu {
    let current = view.read(cx).sort(cx);
    let items = SortBy::ALL.into_iter().map(|sort| {
        let v = view.clone();
        MenuItem::new(("issues-sort-by", sort as usize), sort.label())
            .check_column()
            .checked(sort == current)
            .on_click(move |_, _, cx| v.update(cx, |this, cx| this.set_sort(sort, cx)))
    });
    Menu::new("issues-sort-menu").width(200.).label("Sort by").items(items)
}
