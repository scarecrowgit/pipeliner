//! Mouse selection and copy in a [`DiffView`](super::DiffView): the selection state kept
//! with the view, [`CodeLine`] (the code of one line row, shaped per frame through GPUI's
//! line cache and painted over its selection), and the tracker that owns the mouse.
//!
//! A selection runs over rows: a [`TextPoint`]'s `unit` is a row index, its offset a byte of
//! the row's shown text. A copy gives the code of the selected lines joined by newlines:
//! the source text (tabs as tabs, a line cut for display whole when selected to its end),
//! without line numbers or `+`/`−` markers. Hunk separators and the no-newline note give
//! nothing, not even a blank line.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle, GlobalElementId,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ShapedLine, SharedString, Style, Styled, TextRun,
    TextStyle, Window, canvas, fill, point, px, size,
};

use super::model::{DiffFile, DiffRow, LineRef, TAB_WIDTH};
use super::view::{GUTTER_WIDTH, SIGN_WIDTH};
use crate::markdown::element::selection_tint;
use crate::markdown::selection::{Selection, TextPoint, word_range};

/// A line row as laid out this frame.
pub(crate) struct FrameRow {
    pub row: usize,
    pub bounds: Bounds<Pixels>,
    pub line: ShapedLine,
}

/// The selection of one diff and what it needs to hit-test and copy, shared by the view,
/// its line elements and its mouse handlers.
#[derive(Default)]
pub(crate) struct Shared {
    pub sel: Selection,
    pub dragging: bool,
    /// Line rows laid out this frame (only those on screen).
    pub frame: Vec<FrameRow>,
    pub file: Arc<DiffFile>,
    pub rows: Rc<Vec<DiffRow>>,
    /// Row for row, the source line behind a line row.
    pub sources: Rc<Vec<Option<LineRef>>>,
    /// Rows shown (a limit hides the rest).
    pub shown: usize,
}

/// Re-render the view holding the selection.
pub(crate) type Notify = Rc<dyn Fn(&mut App)>;

impl Shared {
    /// Show `file` with these rows; a different file drops the selection.
    pub fn set_file(
        &mut self,
        file: Arc<DiffFile>,
        rows: Vec<DiffRow>,
        sources: Vec<Option<LineRef>>,
    ) {
        self.file = file;
        self.rows = Rc::new(rows);
        self.sources = Rc::new(sources);
        self.sel.clear();
        self.dragging = false;
    }

    /// The shown text of row `row` (empty for a separator or the no-newline note).
    fn shown_text(&self, row: usize) -> &str {
        match self.rows.get(row) {
            Some(DiffRow::Line { text, .. }) => text,
            _ => "",
        }
    }

    /// The source line behind row `row`, without a trailing `\r`.
    fn source(&self, row: usize) -> Option<&str> {
        let r = (*self.sources.get(row)?)?;
        let text = &self.file.hunks.get(r.hunk)?.lines.get(r.line)?.text;
        Some(text.strip_suffix('\r').unwrap_or(text))
    }

    /// The code selected, as a copy gives it; `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.sel.ordered()?;
        let mut out = String::new();
        let mut first = true;
        for (i, row) in self.rows.iter().enumerate().take(e.unit + 1).skip(s.unit) {
            let DiffRow::Line { text, .. } = row else {
                continue;
            };
            let (Some(r), Some(src)) = (self.sel.range_in(i, text.len()), self.source(i)) else {
                continue;
            };
            if !first {
                out.push('\n');
            }
            out.push_str(&source_part(src, text.len(), r));
            first = false;
        }
        (!out.is_empty()).then_some(out)
    }

    /// Select every row shown.
    pub fn select_all(&mut self) {
        let Some(last) = self.shown.min(self.rows.len()).checked_sub(1) else {
            return;
        };
        self.sel.anchor = Some(TextPoint { unit: 0, offset: 0 });
        self.sel.head = Some(TextPoint {
            unit: last,
            offset: self.shown_text(last).len(),
        });
    }

    /// Where the mouse at `pos` puts a selection point: the line row level with it (left of
    /// `text_left`, the gutter's right edge, counts as the line's start), else the end of
    /// the last line row above it, else the start of the first one shown.
    pub fn point_at(&self, pos: Point<Pixels>, text_left: Pixels) -> Option<TextPoint> {
        let level = self
            .frame
            .iter()
            .find(|f| f.bounds.top() <= pos.y && pos.y < f.bounds.bottom());
        if let Some(f) = level {
            let x = pos.x.max(text_left) - f.bounds.origin.x;
            return Some(TextPoint {
                unit: f.row,
                offset: index_at(&f.line, x),
            });
        }
        let above = self
            .frame
            .iter()
            .filter(|f| f.bounds.bottom() <= pos.y)
            .max_by_key(|f| f.row);
        match above {
            Some(f) => Some(TextPoint {
                unit: f.row,
                offset: f.line.len(),
            }),
            None => self.frame.iter().min_by_key(|f| f.row).map(|f| TextPoint {
                unit: f.row,
                offset: 0,
            }),
        }
    }
}

/// The source behind shown bytes `range` of a line shown `shown_len` bytes long: a tab is
/// shown as [`TAB_WIDTH`] spaces and comes back whole when the range touches it; a range
/// reaching the end of the shown line takes the rest of the source (a line cut for display
/// is copied whole).
fn source_part(src: &str, shown_len: usize, range: Range<usize>) -> String {
    let to_end = range.end >= shown_len;
    let mut out = String::new();
    let mut at = 0;
    for c in src.chars() {
        if at >= range.end && !to_end {
            break;
        }
        let w = if c == '\t' { TAB_WIDTH } else { c.len_utf8() };
        if at + w > range.start {
            out.push(c);
        }
        at += w;
    }
    out
}

/// The character boundary of `line` nearest to `x` (relative to the line's origin).
fn index_at(line: &ShapedLine, x: Pixels) -> usize {
    line.runs
        .iter()
        .flat_map(|r| r.glyphs.iter().map(|g| (g.index, g.position.x)))
        .chain(std::iter::once((line.len(), line.width)))
        .min_by(|a, b| f32::from((a.1 - x).abs()).total_cmp(&f32::from((b.1 - x).abs())))
        .map_or(0, |(i, _)| i)
}

/// The code of one line row: shaped in the inherited text style with the changed words
/// tinted, the selection painted behind it, registered for hit-testing.
pub(crate) struct CodeLine {
    row: usize,
    text: SharedString,
    marks: Vec<Range<usize>>,
    mark: Hsla,
    shared: Rc<RefCell<Shared>>,
    focused: bool,
}

impl CodeLine {
    /// Row `row` showing `text`, with `marks` (sorted, disjoint byte ranges on character
    /// boundaries) on a `mark` background.
    pub fn new(
        row: usize,
        text: SharedString,
        marks: Vec<Range<usize>>,
        mark: Hsla,
        shared: Rc<RefCell<Shared>>,
        focused: bool,
    ) -> Self {
        Self {
            row,
            text,
            marks,
            mark,
            shared,
            focused,
        }
    }

    fn runs(&self, style: &TextStyle) -> Vec<TextRun> {
        let len = self.text.len();
        let mut out = Vec::with_capacity(self.marks.len() * 2 + 1);
        let mut at = 0;
        for m in &self.marks {
            if m.start < at || m.end > len || m.start >= m.end {
                continue;
            }
            if m.start > at {
                out.push(style.to_run(m.start - at));
            }
            let mut run = style.to_run(m.end - m.start);
            run.background_color = Some(self.mark);
            out.push(run);
            at = m.end;
        }
        if len > at {
            out.push(style.to_run(len - at));
        }
        out
    }
}

impl IntoElement for CodeLine {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CodeLine {
    type RequestLayoutState = ShapedLine;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ShapedLine) {
        let style = window.text_style();
        let rem = window.rem_size();
        let runs = self.runs(&style);
        let line = window.text_system().shape_line(
            self.text.clone(),
            style.font_size.to_pixels(rem),
            &runs,
            None,
        );
        let layout = Style {
            flex_shrink: 0.,
            size: size(
                px(f32::from(line.width).ceil()).into(),
                style.line_height_in_pixels(rem).into(),
            ),
            ..Default::default()
        };
        (window.request_layout(layout, [], cx), line)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        line: &mut ShapedLine,
        _: &mut Window,
        _: &mut App,
    ) {
        let mut s = self.shared.borrow_mut();
        let entry = FrameRow {
            row: self.row,
            bounds,
            line: line.clone(),
        };
        match s.frame.iter_mut().find(|f| f.row == self.row) {
            Some(f) => *f = entry,
            None => s.frame.push(entry),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        line: &mut ShapedLine,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let height = bounds.size.height;
        let origin = bounds.origin;
        let _ = line.paint_background(origin, height, window, cx);
        let (range, runs_on) = {
            let s = self.shared.borrow();
            let range = s.sel.range_in(self.row, self.text.len());
            let runs_on = s.sel.ordered().is_some_and(|(_, e)| e.unit > self.row);
            (range, runs_on)
        };
        if let Some(r) = range {
            // A selection that goes on to the next line covers the newline too.
            let newline = if runs_on { height * 0.3 } else { px(0.) };
            let (x0, x1) = (line.x_for_index(r.start), line.x_for_index(r.end) + newline);
            if x1 > x0 {
                window.paint_quad(fill(
                    Bounds::new(point(origin.x + x0, origin.y), size(x1 - x0, height)),
                    selection_tint(self.focused),
                ));
            }
        }
        let _ = line.paint(origin, height, window, cx);
    }
}

/// A zero-layout element covering the diff (put it first, absolutely placed, over the whole
/// view): it forgets last frame's rows before the rows register, shows the I-beam, and
/// owns the mouse. A press inside selects (twice a word, three times the line, with shift
/// it extends); a press anywhere else in the window drops the selection.
pub(crate) fn tracker(
    shared: Rc<RefCell<Shared>>,
    focus: FocusHandle,
    notify: Notify,
) -> impl IntoElement {
    let first = shared.clone();
    canvas(
        move |bounds, window, _| {
            first.borrow_mut().frame.clear();
            window.insert_hitbox(bounds, HitboxBehavior::Normal)
        },
        move |bounds, hitbox, window, _| {
            window.set_cursor_style(CursorStyle::IBeam, &hitbox);
            let text_left = bounds.left() + px(GUTTER_WIDTH + SIGN_WIDTH);

            let (s, n) = (shared.clone(), notify.clone());
            window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                    return;
                }
                let mut st = s.borrow_mut();
                let at = hitbox
                    .is_hovered(window)
                    .then(|| st.point_at(e.position, text_left))
                    .flatten();
                let Some(p) = at else {
                    let had = !st.sel.is_empty();
                    st.sel.clear();
                    st.dragging = false;
                    drop(st);
                    if had {
                        n(cx);
                    }
                    return;
                };
                let text = st.shown_text(p.unit).to_string();
                match e.click_count {
                    2 => st.sel.set(p.unit, word_range(&text, p.offset)),
                    c if c >= 3 => st.sel.set(p.unit, 0..text.len()),
                    _ if e.modifiers.shift && st.sel.anchor.is_some() => st.sel.head = Some(p),
                    _ => st.sel.collapse(p),
                }
                st.dragging = e.click_count <= 1;
                drop(st);
                window.focus(&focus);
                n(cx);
            });

            let (s, n) = (shared.clone(), notify.clone());
            window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let mut st = s.borrow_mut();
                if !st.dragging || e.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let head = st.point_at(e.position, text_left);
                if head.is_some() && head != st.sel.head {
                    st.sel.head = head;
                    drop(st);
                    n(cx);
                }
            });

            let s = shared.clone();
            window.on_mouse_event(move |e: &MouseUpEvent, phase, _, _| {
                if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                    s.borrow_mut().dragging = false;
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
thread_local! {
    /// Every diff's selection state by the view's id, so tests can look inside.
    static VIEWS: RefCell<Vec<(ElementId, std::rc::Weak<RefCell<Shared>>)>> =
        const { RefCell::new(Vec::new()) };
}

/// Remember the state of the view `id` for tests (no-op outside tests).
pub(crate) fn register(_id: &ElementId, _shared: &Rc<RefCell<Shared>>) {
    #[cfg(test)]
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.retain(|(id, s)| id != _id && s.strong_count() > 0);
        v.push((_id.clone(), Rc::downgrade(_shared)));
    });
}

/// The selection state of the view `id`, if it is alive.
#[cfg(test)]
pub(crate) fn shared_of(id: &ElementId) -> Option<Rc<RefCell<Shared>>> {
    VIEWS.with(|v| {
        v.borrow()
            .iter()
            .find(|(i, _)| i == id)
            .and_then(|(_, s)| s.upgrade())
    })
}

/// The window position of byte `offset` of row `row`, if it was laid out this frame.
#[cfg(test)]
pub(crate) fn row_point(shared: &Shared, row: usize, offset: usize) -> Option<Point<Pixels>> {
    let f = shared.frame.iter().find(|f| f.row == row)?;
    let x = f.line.x_for_index(offset);
    Some(f.bounds.origin + point(x + px(1.), f.bounds.size.height / 2.))
}
