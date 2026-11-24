//! The list view: the same issues as the board, in a table sorted by a click on a header.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui::{AnyElement, App, Context, IntoElement, SharedString, Window, canvas, div, prelude::*, px};
use hub::api::Issue;
use ui::dev::Probe;
use ui::prelude::*;

use super::IssuesView;
use super::card::{initials, priority_color, runner_kind, strip};
use super::columns::status_dot;

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
    /// Orders two issues in `direction` (ascending is each order's natural one: board order,
    /// higher priority first, newest first); ties fall back to the board's order.
    pub(crate) fn compare_in(self, direction: SortDirection, a: &Issue, b: &Issue) -> Ordering {
        let first = self.compare_first(a, b);
        let first = if direction == SortDirection::Descending { first.reverse() } else { first };
        first.then_with(|| self.compare(a, b))
    }

    fn compare_first(self, a: &Issue, b: &Issue) -> Ordering {
        match self {
            SortBy::Status => a.status.cmp(&b.status),
            SortBy::Priority => b.priority.cmp(&a.priority),
            SortBy::Key => key_number(&a.key.to_string()).cmp(&key_number(&b.key.to_string())),
            SortBy::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortBy::Owner => a.owner.to_lowercase().cmp(&b.owner.to_lowercase()),
            SortBy::Updated => b.updated.cmp(&a.updated),
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

/// Which columns the list has room for, fewest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Fit {
    /// Key and title.
    Title,
    /// Key, title and status.
    Status,
    /// Key, title, status, priority, owner and updated.
    Compact,
    /// Every column, type and "Runs with" too.
    Full,
}

impl Fit {
    /// The columns for a table `width` px wide; before it is measured (0), by whether the
    /// panel takes room beside it.
    pub(crate) fn of(width: f32, panel: bool) -> Fit {
        match width {
            w if w <= 0. => {
                if panel {
                    Fit::Compact
                } else {
                    Fit::Full
                }
            }
            w if w >= 960. => Fit::Full,
            w if w >= 640. => Fit::Compact,
            w if w >= 400. => Fit::Status,
            _ => Fit::Title,
        }
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
        let (sort, direction) = self.sort(cx);
        let mut rows: Vec<Arc<Issue>> =
            self.issues.read(cx).iter().filter(|issue| self.filters.matches(issue)).cloned().collect();
        rows.sort_by(|a, b| sort.compare_in(direction, a, b));
        rows
    }

    /// The list view.
    pub(crate) fn render_list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = base::time::now_ms();
        let offset = base::time::utc_offset(now);
        let fit = Fit::of(self.list_width.get(), self.has_panel());
        let mut columns: Vec<(Column, Option<SortBy>)> = vec![
            (Column::new("Key").fixed(104.), Some(SortBy::Key)),
            (Column::new("Title").flex(3.), Some(SortBy::Title)),
        ];
        if fit >= Fit::Status {
            columns.push((Column::new("Status").fixed(128.), Some(SortBy::Status)));
        }
        if fit >= Fit::Compact {
            columns.push((Column::new("Priority").fixed(88.), Some(SortBy::Priority)));
        }
        if fit == Fit::Full {
            columns.push((Column::new("Type").fixed(80.), None));
        }
        if fit >= Fit::Compact {
            columns.push((Column::new("Owner").fixed(if fit == Fit::Full { 150. } else { 120. }), Some(SortBy::Owner)));
        }
        if fit == Fit::Full {
            columns.push((Column::new("Runs with").flex(2.), None));
        }
        if fit >= Fit::Compact {
            columns.push((Column::new("Updated").fixed(84.).right(), Some(SortBy::Updated)));
        }
        let (sort, direction) = self.sort(cx);
        let by_column: Vec<Option<SortBy>> = columns.iter().map(|(_, by)| *by).collect();
        let sorted = by_column.iter().position(|by| *by == Some(sort));
        let mut table = Table::new("issues-table");
        for (column, by) in columns {
            table = table.column(if by.is_some() { column.sortable() } else { column });
        }
        if let Some(ix) = sorted {
            table = table.sorted(ix, direction);
        }
        let view = cx.entity().downgrade();
        table = table.on_sort(move |ix, direction, _, cx| {
            let Some(Some(by)) = by_column.get(ix).copied() else { return };
            view.update(cx, |this, cx| this.set_sort(by, direction, cx)).ok();
        });
        for issue in self.rows(cx) {
            table = table.row(self.list_row(&issue, fit, now, offset, cx));
        }
        let (width, panel) = (self.list_width.clone(), self.has_panel());
        // Measures the room the table has; another set of columns is drawn at once.
        let measure = canvas(
            move |bounds, window, cx| {
                let w = f32::from(bounds.size.width);
                if w > 0. && (w - width.get()).abs() > 0.5 {
                    let before = Fit::of(width.get(), panel);
                    width.set(w);
                    if Fit::of(w, panel) != before {
                        window.defer(cx, |window, _| window.refresh());
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id("issues-list")
            .relative()
            .size_full()
            .overflow_y_scroll()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .child(div().relative().flex_none().w_full().child(measure).child(table))
            .into_any_element()
    }

    fn list_row(&self, issue: &Arc<Issue>, fit: Fit, now: i64, offset: i32, cx: &mut Context<Self>) -> TableRow {
        let wide = fit == Fit::Full;
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
            .cell(truncate_lh(key.to_string(), MONO.line).type_style(MONO).text_color(TEXT_MUTED).probe(probe))
            .strong(issue.title.clone());
        let tr = if fit >= Fit::Status {
            tr.cell(
                row()
                    .gap(px(SPACE_2))
                    .child(StatusDot::new(status_dot(issue.status)).small())
                    .child(div().flex_none().child(issue.status.label())),
            )
        } else {
            tr
        };
        if fit < Fit::Compact {
            return tr.on_click(cx.listener(move |this, _, window, cx| this.open_issue(&key, window, cx)));
        }
        let tr = tr.cell(div().text_color(priority_color(issue.priority)).child(issue.priority.label()));
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
