//! Keyboard focus for the kit's controls: the focus ring shown only when focus arrives by
//! keyboard (CSS `:focus-visible`), tab stops, and roving focus inside option groups.
//!
//! - Every enabled interactive component is a tab stop in paint order; Enter and Space on a
//!   focused one fire its click (GPUI does that for any focused element with a click handler).
//! - The ring (`focus_ring()`, the `--focus-ring` token) shows while the last input was a key;
//!   a mouse press hides it again. The overlay host tracks this for its window.
//! - Groups (`Tabs`, `Segmented`, radio and option groups) are one tab stop: the selected
//!   option. Arrow keys move the selection and the focus together.

use std::rc::Rc;

use gpui::{App, Div, ElementId, FocusHandle, Global, Stateful, Window, prelude::*};

use crate::components::{ValueHandler, child_id};
use crate::keys::{MoveDown, MoveFirst, MoveLast, MoveLeft, MoveRight, MoveUp};
use crate::theme::focus_ring;

#[derive(Default)]
struct FocusVisible(bool);

impl Global for FocusVisible {}

/// The last input was a key, so a focused control shows its focus ring.
pub fn focus_visible(cx: &App) -> bool {
    cx.try_global::<FocusVisible>().is_some_and(|f| f.0)
}

/// Record whether the last input came from the keyboard (`true`) or the pointer.
pub fn set_focus_visible(on: bool, window: &mut Window, cx: &mut App) {
    let g = cx.default_global::<FocusVisible>();
    if g.0 != on {
        g.0 = on;
        window.refresh();
    }
}

/// Make `el` a tab stop with the keyboard focus ring, unless `disabled`. `el` needs an id.
pub(crate) fn tab_stop(el: Stateful<Div>, disabled: bool, cx: &App) -> Stateful<Div> {
    if disabled {
        return el;
    }
    let ring = focus_visible(cx);
    el.tab_index(0).when(ring, |d| d.focus(|s| s.shadow(focus_ring())))
}

/// Make `el` focusable through `handle` (a roving option or a component that moves focus
/// itself), with the keyboard focus ring.
pub(crate) fn track(el: Stateful<Div>, handle: &FocusHandle, cx: &App) -> Stateful<Div> {
    let ring = focus_visible(cx);
    el.track_focus(handle).when(ring, |d| d.focus(|s| s.shadow(focus_ring())))
}

/// One focus handle per option of group `id`, kept between frames. Only option `current` is
/// a tab stop, so Tab enters the group at the selected option.
pub(crate) fn roving_handles(
    id: &ElementId,
    n: usize,
    current: usize,
    window: &mut Window,
    cx: &mut App,
) -> Rc<Vec<FocusHandle>> {
    let state = window.use_keyed_state(child_id(id, "focus"), cx, |_, _| Vec::<FocusHandle>::new());
    let handles = state.update(cx, |v, cx| {
        while v.len() < n {
            v.push(cx.focus_handle());
        }
        v.truncate(n);
        v.iter()
            .enumerate()
            .map(|(i, h)| h.clone().tab_index(0).tab_stop(i == current))
            .collect::<Vec<_>>()
    });
    Rc::new(handles)
}

/// Where a move lands in a group of `n` options from `from`: one step (wrapping) or an end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Prev,
    Next,
    First,
    Last,
}

impl Step {
    pub(crate) fn from(self, from: usize, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        match self {
            Step::Prev => (from + n - 1) % n,
            Step::Next => (from + 1) % n,
            Step::First => 0,
            Step::Last => n - 1,
        }
    }
}

/// Arrow keys for a roving group: the root gets the `Choice` key context; a move selects the
/// option it lands on (through `on_select`) and focuses it. `vertical` also maps Up and Down.
pub(crate) fn roving_keys(
    root: Stateful<Div>,
    handles: Rc<Vec<FocusHandle>>,
    selected: usize,
    vertical: bool,
    on_select: Option<ValueHandler<usize>>,
) -> Stateful<Div> {
    let mover = move |step: Step| {
        let handles = handles.clone();
        let on_select = on_select.clone();
        move |window: &mut Window, cx: &mut App| {
            let n = handles.len();
            let from = handles.iter().position(|h| h.is_focused(window)).unwrap_or(selected);
            let to = step.from(from, n);
            set_focus_visible(true, window, cx);
            if let Some(h) = handles.get(to) {
                window.focus(h);
            }
            if to != selected
                && let Some(f) = &on_select
            {
                f(to, window, cx);
            }
        }
    };
    let (prev, next, first, last) = (mover(Step::Prev), mover(Step::Next), mover(Step::First), mover(Step::Last));
    let (prev2, next2) = (mover(Step::Prev), mover(Step::Next));
    root.key_context(crate::keys::CHOICE)
        .on_action(move |_: &MoveLeft, w, cx| prev(w, cx))
        .on_action(move |_: &MoveRight, w, cx| next(w, cx))
        .on_action(move |_: &MoveFirst, w, cx| first(w, cx))
        .on_action(move |_: &MoveLast, w, cx| last(w, cx))
        .when(vertical, |d| {
            d.on_action(move |_: &MoveUp, w, cx| prev2(w, cx))
                .on_action(move |_: &MoveDown, w, cx| next2(w, cx))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_wrap_and_reach_the_ends() {
        assert_eq!(Step::Next.from(2, 3), 0);
        assert_eq!(Step::Prev.from(0, 3), 2);
        assert_eq!(Step::First.from(2, 3), 0);
        assert_eq!(Step::Last.from(0, 3), 2);
        assert_eq!(Step::Next.from(0, 0), 0);
    }
}
