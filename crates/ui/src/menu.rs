//! Menus: [`MenuItem`]s in groups with labels and separators, driven by the keyboard and the
//! pointer.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, IntoElement, RenderOnce, ScrollHandle,
    SharedString, Window, div, prelude::*, px,
};

use crate::components::{ClickHandler, MenuItem, PopLabel, PopSeparator};
use crate::focus::Step;
use crate::keys::{self, Confirm, MoveDown, MoveFirst, MoveLast, MoveUp};
use crate::overlay::{LayerId, current_layer, overlays};

enum Entry {
    Item(Box<MenuItem>),
    Label(PopLabel),
    Separator,
    Other(AnyElement),
}

/// A list of [`MenuItem`]s with [`PopLabel`] headings and [`PopSeparator`]s. One row is
/// selected (`surface-hover`) and follows both the pointer and the keyboard: Up and Down move
/// and wrap, Home and End jump to the ends, the key of a row's one-character key cap
/// ([`MenuItem::kbd`] "A") chooses that row, another typed letter jumps to the next row
/// starting with it, Enter chooses, Escape closes the popover. Disabled rows are skipped. Choosing a
/// row in a popover closes the popover first, then calls the row's `on_click`.
///
/// The menu takes keyboard focus when it first appears (turn that off with
/// `.autofocus(false)` for a menu that sits under a search field).
#[derive(IntoElement)]
pub struct Menu {
    id: ElementId,
    entries: Vec<Entry>,
    width: Option<f32>,
    max_height: Option<f32>,
    autofocus: bool,
    layer: Option<LayerId>,
}

impl Menu {
    /// An empty menu. `id` keeps its selection between frames.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Menu {
            id: id.into(),
            entries: Vec::new(),
            width: None,
            max_height: None,
            autofocus: true,
            layer: current_layer(),
        }
    }

    /// Add a row.
    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(Box::new(item)));
        self
    }

    /// Add rows.
    pub fn items(mut self, items: impl IntoIterator<Item = MenuItem>) -> Self {
        self.entries.extend(items.into_iter().map(|i| Entry::Item(Box::new(i))));
        self
    }

    /// Add a group heading.
    pub fn label(mut self, text: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Label(PopLabel::new(text)));
        self
    }

    /// Add a group heading with something at its right.
    pub fn label_with(mut self, label: PopLabel) -> Self {
        self.entries.push(Entry::Label(label));
        self
    }

    /// Add a rule between groups.
    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    /// Add something that is not a row (a note, a toggle line); the keyboard skips it.
    pub fn child(mut self, el: impl IntoElement) -> Self {
        self.entries.push(Entry::Other(el.into_any_element()));
        self
    }

    /// Width of the popover the menu opens in, px.
    pub fn width(mut self, px: f32) -> Self {
        self.width = Some(px);
        self
    }

    /// Scroll past this height, px.
    pub fn max_height(mut self, px: f32) -> Self {
        self.max_height = Some(px);
        self
    }

    /// Take keyboard focus when it first appears (on by default).
    pub fn autofocus(mut self, on: bool) -> Self {
        self.autofocus = on;
        self
    }

    pub(crate) fn preferred_width(&self) -> Option<f32> {
        self.width
    }
}

struct MenuState {
    selected: Option<usize>,
    focus: FocusHandle,
    /// Scroll position of a menu with a `max_height`.
    scroll: ScrollHandle,
}

struct Row {
    label: SharedString,
    enabled: bool,
    /// The one-character key cap shown on the row, lower case.
    cap: Option<char>,
    on_click: Option<ClickHandler>,
}

/// The key a row's cap stands for: a cap of one character ("A", "/"), lower case. Longer caps
/// ("⌘N") are shortcuts of the app, not of the menu.
pub(crate) fn cap_key(kbd: &str) -> Option<char> {
    let mut chars = kbd.trim().chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else { return None };
    ch.to_lowercase().next()
}

/// What a typed character does in a menu: the row whose cap it is (`Some(Ok)` when that row
/// is enabled, `Some(Err)` when it is off and the key does nothing), else `None` (the
/// character jumps by label).
pub(crate) fn cap_target(caps: &[(Option<char>, bool)], ch: char) -> Option<Result<usize, ()>> {
    let ch = ch.to_lowercase().next()?;
    let ix = caps.iter().position(|(cap, enabled)| *cap == Some(ch) && *enabled)
        .or_else(|| caps.iter().position(|(cap, _)| *cap == Some(ch)))?;
    Some(if caps[ix].1 { Ok(ix) } else { Err(()) })
}

/// The enabled row a move lands on from `from` (`None`: nothing selected yet).
pub(crate) fn step_enabled(enabled: &[bool], from: Option<usize>, step: Step) -> Option<usize> {
    let n = enabled.len();
    if n == 0 {
        return None;
    }
    let order: Box<dyn Iterator<Item = usize>> = match (step, from) {
        (Step::First, _) => Box::new(0..n),
        (Step::Last, _) => Box::new((0..n).rev()),
        (Step::Next, None) => Box::new(0..n),
        (Step::Prev, None) => Box::new((0..n).rev()),
        (Step::Next, Some(f)) => Box::new((1..=n).map(move |k| (f + k) % n)),
        (Step::Prev, Some(f)) => Box::new((1..=n).map(move |k| (f + n * 2 - k) % n)),
    };
    order.into_iter().find(|&i| enabled[i])
}

/// The next enabled row after `from` (wrapping) whose label starts with `ch`.
pub(crate) fn jump_to(labels: &[(SharedString, bool)], from: Option<usize>, ch: char) -> Option<usize> {
    let n = labels.len();
    let start = from.map_or(0, |f| f + 1);
    let ch = ch.to_lowercase().next()?;
    (0..n).map(|k| (start + k) % n).find(|&i| {
        let (label, enabled) = &labels[i];
        *enabled && label.chars().next().and_then(|c| c.to_lowercase().next()) == Some(ch)
    })
}

impl RenderOnce for Menu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let rows: Vec<Row> = self
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(it) => Some(Row {
                    label: it.label.clone(),
                    enabled: !it.disabled,
                    cap: it.kbd.as_ref().and_then(|k| cap_key(k)),
                    on_click: it.on_click.clone(),
                }),
                _ => None,
            })
            .collect();
        let initial = self
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(it) => Some(it),
                _ => None,
            })
            .position(|it| it.checked && !it.disabled);
        let autofocus = self.autofocus;
        let state = window.use_keyed_state(self.id.clone(), cx, |window, cx| {
            let focus = cx.focus_handle();
            if autofocus {
                window.focus(&focus);
            }
            MenuState { selected: initial, focus, scroll: ScrollHandle::new() }
        });
        let (selected, focus, scroll) = {
            let s = state.read(cx);
            (s.selected, s.focus.clone(), s.scroll.clone())
        };
        // Child index of each row in the list (labels and rules sit between rows), so the
        // keyboard can scroll the selected row into view.
        let child_ix: Rc<Vec<usize>> = Rc::new(
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(e, Entry::Item(_)))
                .map(|(i, _)| i)
                .collect(),
        );
        let rows = Rc::new(rows);
        let enabled: Rc<Vec<bool>> = Rc::new(rows.iter().map(|r| r.enabled).collect());
        let labels: Rc<Vec<(SharedString, bool)>> =
            Rc::new(rows.iter().map(|r| (r.label.clone(), r.enabled)).collect());
        let caps: Vec<(Option<char>, bool)> = rows.iter().map(|r| (r.cap, r.enabled)).collect();

        let layer = self.layer;
        let activate = {
            let rows = rows.clone();
            Rc::new(move |ix: usize, window: &mut Window, cx: &mut App| {
                let Some(row) = rows.get(ix).filter(|r| r.enabled) else { return };
                if let Some(layer) = layer {
                    let o = overlays(window, cx);
                    if o.is_popover_layer(layer, cx) {
                        o.close_layer(layer, window, cx);
                    }
                }
                if let Some(f) = &row.on_click {
                    f(&ClickEvent::default(), window, cx);
                }
            })
        };
        let select = {
            let state = state.clone();
            move |ix: Option<usize>, cx: &mut App| {
                state.update(cx, |s, cx| {
                    if s.selected != ix {
                        s.selected = ix;
                        cx.notify();
                    }
                })
            }
        };
        let reveal = {
            let (scroll, child_ix) = (scroll.clone(), child_ix.clone());
            move |row: usize| {
                if let Some(ix) = child_ix.get(row) {
                    scroll.scroll_to_item(*ix);
                }
            }
        };
        let mover = |step: Step| {
            let (state, enabled, select, reveal) =
                (state.clone(), enabled.clone(), select.clone(), reveal.clone());
            move |window: &mut Window, cx: &mut App| {
                crate::focus::set_focus_visible(true, window, cx);
                let from = state.read(cx).selected;
                if let Some(to) = step_enabled(&enabled, from, step) {
                    select(Some(to), cx);
                    reveal(to);
                }
            }
        };
        let (up, down, first, last) = (mover(Step::Prev), mover(Step::Next), mover(Step::First), mover(Step::Last));

        let mut ix = 0usize;
        let children: Vec<AnyElement> = self
            .entries
            .into_iter()
            .map(|entry| match entry {
                Entry::Item(mut item) => {
                    let i = ix;
                    ix += 1;
                    item.selected = selected == Some(i);
                    if !item.disabled {
                        let select = select.clone();
                        item.on_move = Some(Rc::new(move |_, cx| select(Some(i), cx)));
                        let activate = activate.clone();
                        item.on_click = Some(Rc::new(move |_, w, cx| activate(i, w, cx)));
                    }
                    // A dev-driver probe named after the row's id (unless the row names one),
                    // so scripts can click it.
                    if item.probe.is_none() {
                        item.probe = Some(item.id.to_string().into());
                    }
                    (*item).into_any_element()
                }
                Entry::Label(l) => l.into_any_element(),
                Entry::Separator => PopSeparator.into_any_element(),
                Entry::Other(el) => el,
            })
            .collect();

        let confirm = activate.clone();
        let by_cap = activate.clone();
        let jump = {
            let (state, select, reveal) = (state.clone(), select.clone(), reveal.clone());
            move |ev: &gpui::KeyDownEvent, window: &mut Window, cx: &mut App| {
                let k = &ev.keystroke;
                if k.modifiers.control || k.modifiers.alt || k.modifiers.platform || k.modifiers.function {
                    return;
                }
                let mut chars = k.key.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else { return };
                // A row's key cap activates that row; a cap of a row that is off does nothing.
                match cap_target(&caps, ch) {
                    Some(Ok(ix)) => {
                        cx.stop_propagation();
                        by_cap(ix, window, cx);
                        return;
                    }
                    Some(Err(())) => {
                        cx.stop_propagation();
                        return;
                    }
                    None => {}
                }
                if !ch.is_alphanumeric() {
                    return;
                }
                crate::focus::set_focus_visible(true, window, cx);
                let from = state.read(cx).selected;
                if let Some(to) = jump_to(&labels, from, ch) {
                    select(Some(to), cx);
                    reveal(to);
                }
                cx.stop_propagation();
            }
        };
        let selector = self.id.to_string();
        div()
            .id(self.id)
            .debug_selector(move || selector)
            .flex()
            .flex_col()
            .track_focus(&focus)
            .key_context(keys::MENU)
            .on_action(move |_: &MoveUp, w, cx| up(w, cx))
            .on_action(move |_: &MoveDown, w, cx| down(w, cx))
            .on_action(move |_: &MoveFirst, w, cx| first(w, cx))
            .on_action(move |_: &MoveLast, w, cx| last(w, cx))
            .on_action(move |_: &Confirm, w, cx| {
                if let Some(ix) = state.read(cx).selected {
                    confirm(ix, w, cx);
                }
            })
            .on_key_down(jump)
            .when_some(self.max_height, |d, h| d.max_h(px(h)).overflow_y_scroll().track_scroll(&scroll))
            .children(children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_wrap_and_skip_disabled_rows() {
        let en = [true, false, true, true, false];
        assert_eq!(step_enabled(&en, None, Step::Next), Some(0));
        assert_eq!(step_enabled(&en, None, Step::Prev), Some(3));
        assert_eq!(step_enabled(&en, Some(0), Step::Next), Some(2));
        assert_eq!(step_enabled(&en, Some(3), Step::Next), Some(0));
        assert_eq!(step_enabled(&en, Some(0), Step::Prev), Some(3));
        assert_eq!(step_enabled(&en, Some(2), Step::Prev), Some(0));
        assert_eq!(step_enabled(&en, Some(2), Step::Last), Some(3));
        assert_eq!(step_enabled(&[false, false], None, Step::Next), None);
    }

    #[test]
    fn a_key_cap_names_its_row_and_only_single_characters_are_caps() {
        assert_eq!(cap_key("A"), Some('a'));
        assert_eq!(cap_key(" / "), Some('/'));
        assert_eq!(cap_key("⌘N"), None);
        assert_eq!(cap_key(""), None);
        let caps = [(Some('a'), true), (None, true), (Some('p'), false), (Some('t'), true)];
        assert_eq!(cap_target(&caps, 'A'), Some(Ok(0)));
        assert_eq!(cap_target(&caps, 't'), Some(Ok(3)));
        assert_eq!(cap_target(&caps, 'p'), Some(Err(())), "the cap of a row that is off does nothing");
        assert_eq!(cap_target(&caps, 'x'), None, "a key that is no cap jumps by label");
    }

    #[test]
    fn a_letter_jumps_to_the_next_matching_enabled_row() {
        let rows: Vec<(SharedString, bool)> = [("Agent", true), ("Archive", false), ("Autopilot", true), ("Pipeline", true)]
            .into_iter()
            .map(|(l, e)| (l.into(), e))
            .collect();
        assert_eq!(jump_to(&rows, None, 'a'), Some(0));
        assert_eq!(jump_to(&rows, Some(0), 'a'), Some(2));
        assert_eq!(jump_to(&rows, Some(2), 'A'), Some(0));
        assert_eq!(jump_to(&rows, Some(0), 'p'), Some(3));
        assert_eq!(jump_to(&rows, Some(0), 'z'), None);
    }
}
