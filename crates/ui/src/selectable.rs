//! Mouse text selection across many text blocks (e.g. a chat transcript).
//!
//! GPUI's `StyledText` can't be selected. A screen owns a [`Selection`] and, while it builds
//! the selectable part of its tree, wraps the build in [`scope`]; inside the scope [`text`]
//! turns a `StyledText` into a [`SelectableText`] element (outside a scope it returns the
//! plain text, so shared render helpers work everywhere).
//!
//! - drag selects across blocks (in paint order); double click = word, triple click = block;
//! - I-beam cursor over text; a press outside any block clears the selection (call
//!   [`SelState::press_outside`] from the container's mouse-down handler);
//! - [`SelState::selected_text`] joins the pieces with each block's `joiner`.
//!
//! ```ignore
//! self.selection.borrow_mut().begin_frame();
//! let items = selectable::scope(&self.selection, || build_items());
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyledText,
    TextLayout, Window, fill, point, px,
};

use crate::theme::{ACCENT, alpha};

pub type Selection = Rc<RefCell<SelState>>;

pub fn new_selection() -> Selection {
    Rc::new(RefCell::new(SelState::default()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pos {
    key: SharedString,
    ix: usize,
}

struct Block {
    key: SharedString,
    joiner: &'static str,
    text: SharedString,
}

#[derive(Default)]
pub struct SelState {
    anchor: Option<Pos>,
    head: Option<Pos>,
    dragging: bool,
    /// Blocks registered this frame, in paint order.
    blocks: Vec<Block>,
    /// A block handled the current mouse-down (so the container must not clear).
    hit: bool,
    /// Focused when a block is pressed, so a key context can handle copy.
    focus: Option<FocusHandle>,
    /// Select everything at the next paint (blocks are only known after prepaint).
    select_all_pending: bool,
}

impl SelState {
    /// Call at the start of every render of the selectable area.
    pub fn begin_frame(&mut self) {
        self.blocks.clear();
    }

    /// Handle that receives focus when text is pressed (for a `ctrl-c` binding).
    pub fn set_focus(&mut self, focus: FocusHandle) {
        self.focus = Some(focus);
    }

    pub fn is_empty(&self) -> bool {
        match (&self.anchor, &self.head) {
            (Some(a), Some(h)) => a == h,
            _ => true,
        }
    }

    /// Select every block of the area (applied at the next paint).
    pub fn select_all(&mut self) {
        self.select_all_pending = true;
    }

    fn apply_select_all(&mut self) {
        if !self.select_all_pending || self.blocks.is_empty() {
            return;
        }
        self.select_all_pending = false;
        if let (Some(first), Some(last)) = (self.blocks.first(), self.blocks.last()) {
            self.anchor = Some(Pos { key: first.key.clone(), ix: 0 });
            self.head = Some(Pos { key: last.key.clone(), ix: last.text.len() });
        }
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.dragging = false;
    }

    /// For the container's mouse-down handler (runs after the blocks' handlers): clears the
    /// selection unless a block took this press. Returns true if something changed.
    pub fn press_outside(&mut self) -> bool {
        if std::mem::take(&mut self.hit) {
            return false;
        }
        let had = !self.is_empty();
        self.clear();
        had
    }

    fn order(&self, key: &SharedString) -> Option<usize> {
        self.blocks.iter().position(|b| &b.key == key)
    }

    /// `(start, end)` in document order, if both ends are on screen this frame.
    fn span(&self) -> Option<((usize, usize), (usize, usize))> {
        let (a, h) = (self.anchor.as_ref()?, self.head.as_ref()?);
        let a = (self.order(&a.key)?, a.ix);
        let h = (self.order(&h.key)?, h.ix);
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    /// Byte range selected inside the block with registration index `order`.
    fn range_in(&self, order: usize, len: usize) -> Option<(usize, usize)> {
        let ((so, si), (eo, ei)) = self.span()?;
        if order < so || order > eo {
            return None;
        }
        let start = if order == so { si.min(len) } else { 0 };
        let end = if order == eo { ei.min(len) } else { len };
        (start < end).then_some((start, end))
    }

    /// The selected text, pieces joined by each block's `joiner`.
    pub fn selected_text(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::new();
        let mut first = true;
        for (i, b) in self.blocks.iter().enumerate() {
            let Some((s, e)) = self.range_in(i, b.text.len()) else {
                continue;
            };
            let (s, e) = (floor_char(&b.text, s), floor_char(&b.text, e));
            if !first {
                out.push_str(b.joiner);
            }
            out.push_str(&b.text[s..e]);
            first = false;
        }
        (!out.is_empty()).then_some(out)
    }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Word around byte `ix` (alphanumerics and `_`), or the single character there.
fn word_at(text: &str, ix: usize) -> (usize, usize) {
    let ix = floor_char(text, ix);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let Some(c) = text[ix..].chars().next() else {
        return (ix, ix);
    };
    if !is_word(c) {
        return (ix, ix + c.len_utf8());
    }
    let start = text[..ix]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(ix, |(i, _)| i);
    let end = text[ix..]
        .char_indices()
        .find(|&(_, c)| !is_word(c))
        .map_or(text.len(), |(i, _)| ix + i);
    (start, end)
}

// ── Scope ───────────────────────────────────────────────────────────────

struct Scope {
    sel: Selection,
    prefix: SharedString,
    n: usize,
}

thread_local! {
    static SCOPE: RefCell<Option<Scope>> = const { RefCell::new(None) };
}

/// Run `build` with selection enabled for [`text`] calls inside it.
pub fn scope<R>(sel: &Selection, build: impl FnOnce() -> R) -> R {
    let prev = SCOPE.with(|s| {
        s.borrow_mut().replace(Scope { sel: sel.clone(), prefix: "".into(), n: 0 })
    });
    let out = build();
    SCOPE.with(|s| *s.borrow_mut() = prev);
    out
}

/// Namespace for the following blocks' keys (e.g. the transcript item index), so keys of
/// later items stay stable when earlier items change shape.
pub fn set_prefix(prefix: impl Into<SharedString>) {
    SCOPE.with(|s| {
        if let Some(scope) = s.borrow_mut().as_mut() {
            scope.prefix = prefix.into();
            scope.n = 0;
        }
    });
}

/// Selectable text inside a [`scope`], plain `StyledText` outside. `joiner` is inserted
/// before this block's text when copying a selection that also covers earlier blocks
/// (`"\n\n"` paragraphs, `"\n"` lines, `""` pieces of one line, `"\t"` table cells).
pub fn text(styled: StyledText, joiner: &'static str) -> AnyElement {
    let scoped = SCOPE.with(|s| {
        s.borrow_mut().as_mut().map(|scope| {
            scope.n += 1;
            (scope.sel.clone(), SharedString::from(format!("{}:{}", scope.prefix, scope.n)))
        })
    });
    match scoped {
        Some((sel, key)) => SelectableText { sel, key, joiner, text: styled }.into_any_element(),
        None => styled.into_any_element(),
    }
}

/// [`text`] for a plain string.
pub fn plain(s: impl Into<SharedString>, joiner: &'static str) -> AnyElement {
    text(StyledText::new(s.into()), joiner)
}

// ── Element ─────────────────────────────────────────────────────────────

pub struct SelectableText {
    sel: Selection,
    key: SharedString,
    joiner: &'static str,
    text: StyledText,
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        self.text.prepaint(id, inspector_id, bounds, state, window, cx);
        let layout = self.text.layout();
        self.sel.borrow_mut().blocks.push(Block {
            key: self.key.clone(),
            joiner: self.joiner,
            text: SharedString::from(layout.text()),
        });
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.text.layout().clone();
        let len = layout.len();
        let range = {
            self.sel.borrow_mut().apply_select_all();
            let sel = self.sel.borrow();
            sel.order(&self.key).and_then(|o| sel.range_in(o, len))
        };
        if let Some((start, end)) = range {
            paint_selection(&layout, bounds, start, end, window);
        }
        self.text.paint(id, inspector_id, bounds, state, &mut (), window, cx);
        window.set_cursor_style(CursorStyle::IBeam, hitbox);

        let index = |layout: &TextLayout, p: gpui::Point<Pixels>| match layout.index_for_position(p)
        {
            Ok(i) | Err(i) => i,
        };

        let (sel, key, hb) = (self.sel.clone(), self.key.clone(), hitbox.clone());
        let lay = layout.clone();
        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left {
                return;
            }
            if !hb.is_hovered(window) {
                return;
            }
            let ix = index(&lay, ev.position);
            let text = lay.text();
            let mut s = sel.borrow_mut();
            let (a, h) = match ev.click_count {
                2 => word_at(&text, ix),
                n if n >= 3 => (0, text.len()),
                _ => (ix, ix),
            };
            s.anchor = Some(Pos { key: key.clone(), ix: a });
            s.head = Some(Pos { key: key.clone(), ix: h });
            s.dragging = ev.click_count <= 1;
            s.hit = true;
            if let Some(f) = s.focus.clone() {
                drop(s);
                window.focus(&f);
            }
            window.refresh();
        });

        let (sel, key) = (self.sel.clone(), self.key.clone());
        let lay = layout.clone();
        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || !bounds.contains(&ev.position) {
                return;
            }
            let mut s = sel.borrow_mut();
            if !s.dragging || ev.pressed_button != Some(MouseButton::Left) {
                return;
            }
            let head = Pos { key: key.clone(), ix: index(&lay, ev.position) };
            if s.head.as_ref() != Some(&head) {
                s.head = Some(head);
                window.refresh();
            }
        });

        let sel = self.sel.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, _cx| {
            if phase == DispatchPhase::Bubble {
                sel.borrow_mut().dragging = false;
            }
        });
    }
}

/// Highlight `start..end` of a wrapped text layout: first line from the start x, full lines
/// in between, last line up to the end x.
fn paint_selection(
    layout: &TextLayout,
    bounds: Bounds<Pixels>,
    start: usize,
    end: usize,
    window: &mut Window,
) {
    let (Some(a), Some(b)) = (layout.position_for_index(start), layout.position_for_index(end))
    else {
        return;
    };
    let lh = layout.line_height();
    let color = alpha(ACCENT, 0.32);
    let right = bounds.right();
    let left = bounds.left();
    let rect = |x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels| {
        Bounds::from_corners(point(x0, y0), point(x1.max(x0 + px(2.)), y1))
    };
    if (a.y - b.y).abs() < px(1.) {
        window.paint_quad(fill(rect(a.x, a.y, b.x, a.y + lh), color));
    } else {
        window.paint_quad(fill(rect(a.x, a.y, right, a.y + lh), color));
        if b.y > a.y + lh {
            window.paint_quad(fill(rect(left, a.y + lh, right, b.y), color));
        }
        window.paint_quad(fill(rect(left, b.y, b.x, b.y + lh), color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundaries() {
        let t = "foo bar_baz, qux";
        assert_eq!(word_at(t, 5), (4, 11));
        assert_eq!(word_at(t, 0), (0, 3));
        assert_eq!(word_at(t, 11), (11, 12));
        assert_eq!(word_at("привет мир", 2), (0, 12));
    }

    fn st(blocks: &[(&str, &str)]) -> SelState {
        SelState {
            blocks: blocks
                .iter()
                .map(|(k, t)| Block { key: k.to_string().into(), joiner: "\n\n", text: t.to_string().into() })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn selection_across_blocks_in_either_direction() {
        let mut s = st(&[("a", "hello world"), ("b", "second"), ("c", "third one")]);
        s.anchor = Some(Pos { key: "c".into(), ix: 5 });
        s.head = Some(Pos { key: "a".into(), ix: 6 });
        assert_eq!(s.selected_text().unwrap(), "world\n\nsecond\n\nthird");
        assert_eq!(s.range_in(1, 6), Some((0, 6)));
        s.head = s.anchor.clone();
        assert!(s.is_empty() && s.selected_text().is_none());
    }

    #[test]
    fn press_outside_clears_unless_hit() {
        let mut s = st(&[("a", "x")]);
        s.anchor = Some(Pos { key: "a".into(), ix: 0 });
        s.head = Some(Pos { key: "a".into(), ix: 1 });
        s.hit = true;
        assert!(!s.press_outside());
        assert!(!s.is_empty());
        assert!(s.press_outside());
        assert!(s.is_empty());
    }
}
