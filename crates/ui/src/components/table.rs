//! The `.tbl` look: a header row and body rows laid out from column definitions.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, IntoElement, RenderOnce, SharedString, Window, div,
    prelude::*, px, relative,
};

use super::{ClickHandler, child_id};
use crate::assets::{IconName, icon};
use crate::dev::Probe;
use crate::focus;
use crate::layout::{col, row, truncate_lh};
use crate::motion::{self, Look};
use crate::theme::*;

/// Which way a sorted column runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortDirection {
    /// Smallest first: an up arrow.
    #[default]
    Ascending,
    /// Largest first: a down arrow.
    Descending,
}

impl SortDirection {
    /// The other direction.
    pub fn flip(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}

/// The sort a click on the header of `column` asks for, given the `current` one: the sorted
/// column flips its direction, another column starts ascending.
pub fn next_sort(current: Option<(usize, SortDirection)>, column: usize) -> (usize, SortDirection) {
    match current {
        Some((c, dir)) if c == column => (column, dir.flip()),
        _ => (column, SortDirection::Ascending),
    }
}

/// A sort request from a [`Table`]: the column index and the direction.
type SortHandler = Rc<dyn Fn(usize, SortDirection, &mut Window, &mut App) + 'static>;

/// Horizontal alignment of a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    /// Text starts at the cell's left edge.
    #[default]
    Left,
    /// Money and counts: flush right.
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Width {
    Fixed(f32),
    Flex(f32),
}

/// A column definition: header text, width and alignment.
#[derive(Clone, Debug)]
pub struct Column {
    header: SharedString,
    width: Width,
    align: Align,
    sortable: bool,
}

impl Column {
    /// A flexible column (ratio 1) headed `header`.
    pub fn new(header: impl Into<SharedString>) -> Self {
        Self { header: header.into(), width: Width::Flex(1.), align: Align::Left, sortable: false }
    }

    /// A click on the header asks the table's `on_sort` to sort by this column.
    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }

    /// A fixed width in px.
    pub fn fixed(mut self, width: f32) -> Self {
        self.width = Width::Fixed(width);
        self
    }

    /// A flexible column with this share of the free width.
    pub fn flex(mut self, ratio: f32) -> Self {
        self.width = Width::Flex(ratio);
        self
    }

    /// Right-align the header and the cells.
    pub fn right(mut self) -> Self {
        self.align = Align::Right;
        self
    }
}

/// One body row: a cell per column. Hover fades to `surface-raised`; `selected` sits on
/// `surface-hover` with a 2px accent bar inset on the left edge.
pub struct TableRow {
    id: ElementId,
    cells: Vec<AnyElement>,
    selected: bool,
    on_click: Option<ClickHandler>,
}

impl TableRow {
    /// An empty row.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self { id: id.into(), cells: Vec::new(), selected: false, on_click: None }
    }

    /// Add a cell with any content.
    pub fn cell(mut self, el: impl IntoElement) -> Self {
        self.cells.push(el.into_any_element());
        self
    }

    /// Add a text cell that ellipsizes.
    pub fn text(self, text: impl Into<SharedString>) -> Self {
        self.cell(truncate_lh(text, BODY.line))
    }

    /// Add the object's name: `BODY_STRONG`, ellipsized.
    pub fn strong(self, text: impl Into<SharedString>) -> Self {
        self.cell(truncate_lh(text, BODY.line).type_style(BODY_STRONG))
    }

    /// Add machine text (keys, tools, costs): mono, ellipsized.
    pub fn mono(self, text: impl Into<SharedString>) -> Self {
        self.cell(truncate_lh(text, MONO.line).type_style(MONO))
    }

    /// The selected row.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Called when the row is clicked.
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

enum Entry {
    Row(TableRow),
    Group(SharedString, Option<SharedString>),
}

/// A list of records in a card frame: `surface`, `line` border, `radius-xl`. A 36px header of
/// 12px muted labels, 48px rows divided by `line-subtle` hairlines, optional group header
/// rows. Right-align money and counts; put keys and costs in mono columns.
///
/// Sorting: headers of [`Column::sortable`] columns are buttons; the sorted one
/// ([`Table::sorted`]) is in `text` with an arrow for its direction. A click calls
/// [`Table::on_sort`] with the sort it asks for ([`next_sort`]); the caller sorts the rows.
/// Header `ix` is probed `<table id>-head-<ix>`.
#[derive(IntoElement)]
pub struct Table {
    id: ElementId,
    columns: Vec<Column>,
    entries: Vec<Entry>,
    frameless: bool,
    sorted: Option<(usize, SortDirection)>,
    on_sort: Option<SortHandler>,
}

impl Table {
    /// A table with no columns.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            columns: Vec::new(),
            entries: Vec::new(),
            frameless: false,
            sorted: None,
            on_sort: None,
        }
    }

    /// The rows are sorted by `column` (an index) in `direction`: its header shows the arrow.
    pub fn sorted(mut self, column: usize, direction: SortDirection) -> Self {
        self.sorted = Some((column, direction));
        self
    }

    /// Called when a sortable header is clicked, with the column and direction to sort by.
    pub fn on_sort(mut self, f: impl Fn(usize, SortDirection, &mut Window, &mut App) + 'static) -> Self {
        self.on_sort = Some(Rc::new(f));
        self
    }

    /// Add a column.
    pub fn column(mut self, column: Column) -> Self {
        self.columns.push(column);
        self
    }

    /// Add a body row.
    pub fn row(mut self, row: TableRow) -> Self {
        self.entries.push(Entry::Row(row));
        self
    }

    /// Add a group header row ("Today", with a count) that spans the columns.
    pub fn group(mut self, label: impl Into<SharedString>, count: Option<SharedString>) -> Self {
        self.entries.push(Entry::Group(label.into(), count));
        self
    }

    /// No surface, border or radius: the table sits inside a frame of its own.
    pub fn frameless(mut self) -> Self {
        self.frameless = true;
        self
    }
}

fn cell_box(column: &Column, ix: usize, count: usize, content: AnyElement) -> gpui::Div {
    let mut cell = div()
        .flex()
        .items_center()
        .h_full()
        .min_w_0()
        .overflow_hidden()
        .pl(px(if ix == 0 { SPACE_4 } else { SPACE_3 }))
        .pr(px(if ix + 1 == count { SPACE_4 } else { SPACE_3 }));
    match column.width {
        Width::Fixed(w) => cell = cell.w(px(w)).flex_none(),
        Width::Flex(r) => {
            let style = gpui::Styled::style(&mut cell);
            style.flex_grow = Some(r);
            style.flex_shrink = Some(1.);
            style.flex_basis = Some(relative(0.).into());
        }
    }
    if column.align == Align::Right {
        cell = cell.justify_end();
    }
    cell.child(content)
}

impl RenderOnce for Table {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let count = self.columns.len();
        let heads: Vec<AnyElement> = self
            .columns
            .iter()
            .enumerate()
            .map(|(ix, c)| head_cell(&self.id, c, ix, count, self.sorted, self.on_sort.clone(), window, cx))
            .collect();
        let head = row()
            .flex_none()
            .h(px(TABLE_HEAD_HEIGHT))
            .border_b_1()
            .border_color(LINE)
            .type_style(LABEL)
            .text_color(TEXT_MUTED)
            .whitespace_nowrap()
            .children(heads);
        let last = self.entries.len().saturating_sub(1);
        let body: Vec<AnyElement> = self
            .entries
            .into_iter()
            .enumerate()
            .map(|(ix, entry)| match entry {
                Entry::Group(label, n) => row()
                    .flex_none()
                    .gap(px(SPACE_1_5))
                    .h(px(TABLE_GROUP_HEIGHT))
                    .px(px(SPACE_4))
                    .bg(BG_SIDEBAR)
                    .border_b_1()
                    .border_color(LINE)
                    .child(div().type_style(BODY_STRONG).child(label))
                    .when_some(n, |d, n| {
                        d.child(div().type_style(LABEL).text_color(TEXT_MUTED).tabular().child(n))
                    })
                    .into_any_element(),
                Entry::Row(r) => {
                    let key = child_id(&self.id, &r.id);
                    render_row(r, key, &self.columns, ix == last, window, cx)
                }
            })
            .collect();
        col()
            .id(self.id)
            .w_full()
            .when(!self.frameless, |d| {
                d.bg(SURFACE).border_1().border_color(LINE).rounded(px(RADIUS_XL)).overflow_hidden()
            })
            .type_style(BODY)
            .child(head)
            .children(body)
    }
}

/// A header cell: a muted label, or for a sortable column a button with the sort arrow.
#[allow(clippy::too_many_arguments)]
fn head_cell(
    table: &ElementId,
    column: &Column,
    ix: usize,
    count: usize,
    sorted: Option<(usize, SortDirection)>,
    on_sort: Option<SortHandler>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let label = div().child(column.header.clone());
    if !column.sortable {
        return cell_box(column, ix, count, label.into_any_element()).into_any_element();
    }
    let direction = sorted.filter(|(c, _)| *c == ix).map(|(_, d)| d);
    let id = child_id(table, format!("head-{ix}"));
    let selector = id.to_string();
    let rest = if direction.is_some() { TEXT } else { TEXT_MUTED };
    let faded = motion::hover_fade_state(id.clone(), Look::bg(TRANSPARENT).fg(rest), Look::bg(TRANSPARENT).fg(TEXT), window, cx);
    let color = faded.fg.unwrap_or(rest);
    let arrow = direction.map(|d| {
        let name = match d {
            SortDirection::Ascending => IconName::ArrowUp,
            SortDirection::Descending => IconName::ArrowDown,
        };
        icon(name, 12., color)
    });
    let content = row().gap(px(SPACE_1)).child(label).children(arrow).into_any_element();
    let el = cell_box(column, ix, count, content).id(id).debug_selector(move || selector);
    let el = faded.apply(focus::tab_stop(el, false, cx)).cursor_pointer().when_some(on_sort, |d, f| {
        d.on_click(move |_, window, cx| {
            let (column, direction) = next_sort(sorted, ix);
            f(column, direction, window, cx)
        })
    });
    el.probe(format!("{table}-head-{ix}")).into_any_element()
}

fn render_row(
    r: TableRow,
    key: ElementId,
    columns: &[Column],
    last: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let count = columns.len();
    let rest = Look::bg(if r.selected { SURFACE_HOVER } else { TRANSPARENT });
    let hover = Look::bg(if r.selected { SURFACE_HOVER } else { SURFACE_RAISED });
    let selector = r.id.to_string();
    let faded = motion::hover_fade_state(key, rest, hover, window, cx);
    let el = row()
        .id(r.id)
        .debug_selector(move || selector)
        .relative()
        .flex_none()
        .h(px(TABLE_ROW_HEIGHT))
        .whitespace_nowrap()
        .when(!last, |d| d.border_b_1().border_color(LINE_SUBTLE))
        .when(r.on_click.is_some(), |d| d.cursor_pointer());
    faded
        .apply(el)
        .when(r.selected, |d| {
            d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(ACCENT))
        })
        .children(r.cells.into_iter().enumerate().map(|(ix, content)| match columns.get(ix) {
            Some(c) => cell_box(c, ix, count, content),
            None => div().child(content),
        }))
        .when_some(r.on_click, |d, f| d.on_click(move |e, w, cx| f(e, w, cx)))
        .into_any_element()
}
