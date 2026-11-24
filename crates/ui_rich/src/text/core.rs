//! State and behaviour shared by the two faces of the editor: [`EditCore`] (buffer plus
//! what the element needs between frames), the [`EditorView`] trait the element and the
//! common actions are generic over, and the platform input handler glue.

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, Bounds, ClipboardItem, Context, Div, FocusHandle, InteractiveElement, Pixels, Point,
    SharedString, Subscription, Task, UTF16Selection, Window, point, px, size,
};

use super::buffer::{Buffer, LogicalLines, Motion};
use super::layout::Shaped;
use super::segment as seg;
use super::*;

/// How long the caret stays on or off while blinking.
const BLINK: Duration = Duration::from_millis(530);

/// What a mouse drag extends by, after a single, double or triple click.
#[derive(Clone, Debug)]
pub(crate) enum Drag {
    Chars,
    Words(Range<usize>),
    Lines(Range<usize>),
}

/// Everything an editable field keeps between frames.
pub(crate) struct EditCore {
    pub buffer: Buffer,
    pub focus: FocusHandle,
    pub placeholder: SharedString,
    pub multiline: bool,
    pub min_rows: usize,
    pub max_rows: usize,
    pub disabled: bool,
    /// Ranges drawn in their own style; the host keeps them in step with edits.
    pub styles: Vec<super::styles::StyledRange>,
    /// The last shaped text and where it was painted.
    pub shaped: Option<Rc<Shaped>>,
    pub bounds: Option<Bounds<Pixels>>,
    /// How far the content is scrolled: `y` for a text area, `x` for a single line.
    pub scroll: Point<Pixels>,
    /// Scroll the caret into view at the next prepaint.
    pub autoscroll: bool,
    pub drag: Option<Drag>,
    pub caret_on: bool,
    /// Keep the caret on through the next blink tick (it just moved).
    blink_hold: bool,
    blink: Option<Task<()>>,
    subscriptions: Vec<Subscription>,
}

impl EditCore {
    pub fn new(buffer: Buffer, multiline: bool, cx: &mut App) -> Self {
        Self {
            buffer,
            // A tab stop: Tab and Shift-Tab move between fields in paint order.
            focus: cx.focus_handle().tab_index(0).tab_stop(true),
            placeholder: SharedString::default(),
            multiline,
            min_rows: 1,
            max_rows: if multiline { 10 } else { 1 },
            disabled: false,
            styles: Vec::new(),
            shaped: None,
            bounds: None,
            scroll: Point::default(),
            autoscroll: true,
            drag: None,
            caret_on: true,
            blink_hold: false,
            blink: None,
            subscriptions: Vec::new(),
        }
    }

    /// The caret or selection moved: show the caret and bring it into view.
    pub fn touched(&mut self) {
        self.caret_on = true;
        self.blink_hold = true;
        self.autoscroll = true;
    }

    /// Where the text's top-left was painted, in window coordinates.
    pub fn content_origin(&self) -> Option<Point<Pixels>> {
        self.bounds.map(|b| b.origin - self.scroll)
    }

    /// The caret's rectangle in window coordinates, as of the last paint.
    pub fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let (shaped, origin) = (self.shaped.as_ref()?, self.content_origin()?);
        let p = shaped.caret_position(self.buffer.head());
        Some(Bounds::new(origin + p, size(px(1.), shaped.line_height())))
    }

    /// Bounds of `range` in window coordinates (its first visual line), for the IME.
    pub fn range_bounds(&self, range: Range<usize>) -> Option<Bounds<Pixels>> {
        let (shaped, origin) = (self.shaped.as_ref()?, self.content_origin()?);
        let a = shaped.caret_position(range.start);
        let b = shaped.caret_position(range.end);
        let right = if b.y == a.y {
            b.x.max(a.x + px(1.))
        } else {
            a.x + px(1.)
        };
        Some(Bounds::from_corners(
            origin + a,
            origin + point(right, a.y + shaped.line_height()),
        ))
    }

    /// The caret offset nearest a window position.
    pub fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        let (shaped, origin) = (self.shaped.as_ref()?, self.content_origin()?);
        Some(shaped.offset_at(position - origin))
    }

    fn offset_under(&self, position: Point<Pixels>) -> Option<usize> {
        let (shaped, origin) = (self.shaped.as_ref()?, self.content_origin()?);
        Some(shaped.offset_under(position - origin))
    }

    /// A press at `position` with `clicks` clicks: place the caret, extend, or select a word
    /// or line, and start a drag.
    pub fn mouse_down(&mut self, position: Point<Pixels>, clicks: usize, shift: bool) {
        let Some(at) = self.offset_at(position) else {
            return;
        };
        match clicks {
            0 | 1 => {
                if shift {
                    self.buffer.extend_to(at)
                } else {
                    self.buffer.set_cursor(at)
                }
                self.drag = Some(Drag::Chars);
            }
            2 => {
                let under = self.offset_under(position).unwrap_or(at);
                self.buffer.select_word_at(under);
                self.drag = Some(Drag::Words(self.buffer.selection()));
            }
            _ => {
                self.buffer.select_line_at(at);
                self.drag = Some(Drag::Lines(self.buffer.selection()));
            }
        }
        self.touched();
    }

    /// The mouse moved to `position` with the button held: extend the drag selection.
    pub fn mouse_drag(&mut self, position: Point<Pixels>) -> bool {
        let (Some(drag), Some(at)) = (self.drag.clone(), self.offset_at(position)) else {
            return false;
        };
        let before = (self.buffer.anchor(), self.buffer.head());
        match drag {
            Drag::Chars => self.buffer.extend_to(at),
            Drag::Words(origin) => {
                let under = self.offset_under(position).unwrap_or(at);
                let piece = seg::word_at(self.buffer.text(), under);
                self.buffer.select_spanning(origin, piece)
            }
            Drag::Lines(origin) => {
                let piece = seg::line_at(self.buffer.text(), at);
                self.buffer.select_spanning(origin, piece)
            }
        }
        let moved = before != (self.buffer.anchor(), self.buffer.head());
        if moved {
            self.touched();
        }
        moved
    }
}

/// A view that owns an [`EditCore`]: the element and the shared actions work on it.
pub(crate) trait EditorView: gpui::EntityInputHandler + Sized + 'static {
    fn core(&self) -> &EditCore;
    fn core_mut(&mut self) -> &mut EditCore;
    /// The user changed the text (typing, IME, paste, cut, delete, undo).
    fn changed(&mut self, cx: &mut Context<Self>);
    /// Focus arrived (`true`) or left.
    fn focus_changed(&mut self, focused: bool, window: &mut Window, cx: &mut Context<Self>);
}

/// Subscribe to the view's focus once a window exists: blink while focused, report focus
/// and blur to the view.
pub(crate) fn ensure_focus_subscriptions<V: EditorView>(
    view: &mut V,
    window: &mut Window,
    cx: &mut Context<V>,
) {
    if !view.core().subscriptions.is_empty() {
        return;
    }
    let focus = view.core().focus.clone();
    let on = cx.on_focus(&focus, window, |this: &mut V, window, cx| {
        start_blink(this, cx);
        this.core_mut().touched();
        this.focus_changed(true, window, cx);
        cx.notify();
    });
    let off = cx.on_blur(&focus, window, |this: &mut V, window, cx| {
        let core = this.core_mut();
        core.blink = None;
        core.drag = None;
        core.buffer.unmark();
        this.focus_changed(false, window, cx);
        cx.notify();
    });
    view.core_mut().subscriptions = vec![on, off];
}

fn start_blink<V: EditorView>(view: &mut V, cx: &mut Context<V>) {
    if ui::motion::reduced() {
        view.core_mut().caret_on = true;
        return;
    }
    let task = cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(BLINK).await;
            let alive = this.update(cx, |v: &mut V, cx| {
                let core = v.core_mut();
                if std::mem::take(&mut core.blink_hold) {
                    core.caret_on = true;
                } else {
                    core.caret_on = !core.caret_on;
                }
                cx.notify();
            });
            if alive.is_err() {
                break;
            }
        }
    });
    view.core_mut().blink = Some(task);
}

/// Move the caret (or extend) by a layout-free motion.
pub(crate) fn motion<V: EditorView>(v: &mut V, m: Motion, extend: bool, cx: &mut Context<V>) {
    let core = v.core_mut();
    core.buffer.move_to(m, extend);
    core.touched();
    cx.notify();
}

/// Move the caret (or extend) one visual line, using the last layout.
pub(crate) fn vertical<V: EditorView>(v: &mut V, down: bool, extend: bool, cx: &mut Context<V>) {
    let core = v.core_mut();
    match core.shaped.clone() {
        Some(shaped) => core.buffer.move_vertically(down, extend, shaped.as_ref()),
        None => {
            let text = core.buffer.text().to_string();
            core.buffer
                .move_vertically(down, extend, &LogicalLines(&text));
        }
    }
    core.touched();
    cx.notify();
}

/// Run an edit on the buffer; report a change to the view.
pub(crate) fn edit<V: EditorView>(
    v: &mut V,
    cx: &mut Context<V>,
    f: impl FnOnce(&mut Buffer) -> bool,
) -> bool {
    let core = v.core_mut();
    let changed = f(&mut core.buffer);
    core.touched();
    if changed {
        v.changed(cx);
    }
    cx.notify();
    changed
}

fn delete<V: EditorView>(v: &mut V, m: Motion, cx: &mut Context<V>) {
    if v.core().disabled {
        return;
    }
    let nothing = !v.core().buffer.has_selection() && v.core().buffer.head() == 0;
    if !edit(v, cx, |b| b.delete(m)) && nothing && m == Motion::Left {
        // Nothing to delete: an outer binding may use the key (remove a chip).
        cx.propagate();
    }
}

fn copy<V: EditorView>(v: &mut V, cx: &mut Context<V>) {
    let sel = v.core().buffer.selected_text();
    if !sel.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(sel.to_string()));
    }
}

/// The actions both faces handle the same way: motion, selection, deletion, clipboard
/// (except paste), undo. Vertical motion, Enter, Tab, Escape and paste are per face.
pub(crate) fn common_actions<V: EditorView>(el: Div, cx: &mut Context<V>) -> Div {
    macro_rules! on {
        ($el:expr, $($action:ty => $body:expr),* $(,)?) => {
            $el$(.on_action(cx.listener(|this: &mut V, _: &$action, _: &mut Window, cx: &mut Context<V>| {
                #[allow(clippy::redundant_closure_call)]
                ($body)(this, cx)
            })))*
        };
    }
    use Motion as M;
    on!(el,
        MoveLeft => |v, cx| motion(v, M::Left, false, cx),
        MoveRight => |v, cx| motion(v, M::Right, false, cx),
        WordLeft => |v, cx| motion(v, M::WordLeft, false, cx),
        WordRight => |v, cx| motion(v, M::WordRight, false, cx),
        LineStart => |v, cx| motion(v, M::LineStart, false, cx),
        LineEnd => |v, cx| motion(v, M::LineEnd, false, cx),
        DocStart => |v, cx| motion(v, M::DocStart, false, cx),
        DocEnd => |v, cx| motion(v, M::DocEnd, false, cx),
        SelectLeft => |v, cx| motion(v, M::Left, true, cx),
        SelectRight => |v, cx| motion(v, M::Right, true, cx),
        SelectWordLeft => |v, cx| motion(v, M::WordLeft, true, cx),
        SelectWordRight => |v, cx| motion(v, M::WordRight, true, cx),
        SelectLineStart => |v, cx| motion(v, M::LineStart, true, cx),
        SelectLineEnd => |v, cx| motion(v, M::LineEnd, true, cx),
        SelectDocStart => |v, cx| motion(v, M::DocStart, true, cx),
        SelectDocEnd => |v, cx| motion(v, M::DocEnd, true, cx),
        SelectAll => |v: &mut V, cx: &mut Context<V>| {
            v.core_mut().buffer.select_all();
            v.core_mut().touched();
            cx.notify();
        },
        Backspace => |v, cx| delete(v, M::Left, cx),
        Delete => |v, cx| delete(v, M::Right, cx),
        DeleteWordLeft => |v, cx| delete(v, M::WordLeft, cx),
        DeleteWordRight => |v, cx| delete(v, M::WordRight, cx),
        DeleteToLineStart => |v, cx| delete(v, M::LineStart, cx),
        DeleteToLineEnd => |v, cx| delete(v, M::LineEnd, cx),
        Copy => |v, cx| copy(v, cx),
        Cut => |v: &mut V, cx: &mut Context<V>| {
            if v.core().disabled || !v.core().buffer.has_selection() {
                return;
            }
            copy(v, cx);
            edit(v, cx, |b| b.delete(M::Left));
        },
        Undo => |v: &mut V, cx: &mut Context<V>| {
            if !v.core().disabled {
                edit(v, cx, Buffer::undo);
            }
        },
        Redo => |v: &mut V, cx: &mut Context<V>| {
            if !v.core().disabled {
                edit(v, cx, Buffer::redo);
            }
        },
    )
}

/// Implements `gpui::EntityInputHandler` for a view that implements `EditorView`, so IME
/// composition, dead keys and typed text reach the buffer.
macro_rules! impl_input_handler {
    ($ty:ty) => {
        impl gpui::EntityInputHandler for $ty {
            fn text_for_range(
                &mut self,
                range: std::ops::Range<usize>,
                adjusted: &mut Option<std::ops::Range<usize>>,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> Option<String> {
                $crate::text::core::text_for_range(self.core(), range, adjusted)
            }

            fn selected_text_range(
                &mut self,
                _ignore_disabled_input: bool,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> Option<gpui::UTF16Selection> {
                Some($crate::text::core::selected_range(self.core()))
            }

            fn marked_text_range(
                &self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> Option<std::ops::Range<usize>> {
                let b = &self.core().buffer;
                b.marked().map(|r| b.range_to_utf16(&r))
            }

            fn unmark_text(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
                self.core_mut().buffer.unmark();
                cx.notify();
            }

            fn replace_text_in_range(
                &mut self,
                range: Option<std::ops::Range<usize>>,
                text: &str,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) {
                $crate::text::core::ime_commit(self, range, text, cx);
            }

            fn replace_and_mark_text_in_range(
                &mut self,
                range: Option<std::ops::Range<usize>>,
                text: &str,
                selected: Option<std::ops::Range<usize>>,
                _: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) {
                $crate::text::core::ime_mark(self, range, text, selected, cx);
            }

            fn bounds_for_range(
                &mut self,
                range: std::ops::Range<usize>,
                _element_bounds: gpui::Bounds<gpui::Pixels>,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> Option<gpui::Bounds<gpui::Pixels>> {
                let core = self.core();
                core.range_bounds(core.buffer.range_from_utf16(&range))
            }

            fn character_index_for_point(
                &mut self,
                point: gpui::Point<gpui::Pixels>,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> Option<usize> {
                let core = self.core();
                core.offset_at(point).map(|o| core.buffer.to_utf16(o))
            }
        }
    };
}
pub(crate) use impl_input_handler;

pub(crate) fn text_for_range(
    core: &EditCore,
    range: Range<usize>,
    adjusted: &mut Option<Range<usize>>,
) -> Option<String> {
    let b = &core.buffer;
    let r = b.range_from_utf16(&range);
    *adjusted = Some(b.range_to_utf16(&r));
    b.text().get(r).map(str::to_string)
}

pub(crate) fn selected_range(core: &EditCore) -> UTF16Selection {
    let b = &core.buffer;
    UTF16Selection {
        range: b.range_to_utf16(&b.selection()),
        reversed: b.is_reversed(),
    }
}

pub(crate) fn ime_commit<V: EditorView>(
    v: &mut V,
    range: Option<Range<usize>>,
    text: &str,
    cx: &mut Context<V>,
) {
    if v.core().disabled {
        return;
    }
    let range = range.map(|r| v.core().buffer.range_from_utf16(&r));
    edit(v, cx, |b| b.commit_text(range, text));
}

pub(crate) fn ime_mark<V: EditorView>(
    v: &mut V,
    range: Option<Range<usize>>,
    text: &str,
    selected: Option<Range<usize>>,
    cx: &mut Context<V>,
) {
    if v.core().disabled {
        return;
    }
    let range = range.map(|r| v.core().buffer.range_from_utf16(&r));
    let selected = selected.map(|r| seg::range_from_utf16(text, &r));
    edit(v, cx, |b| b.mark_text(range, text, selected));
}
