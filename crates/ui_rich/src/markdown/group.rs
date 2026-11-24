//! [`SelectionGroup`]: one mouse selection across several [`Markdown`](super::Markdown) views
//! (log lines, rows of problems), each drawn as its own view.
//!
//! The group holds the selection as two points, each a member key and a point inside that
//! member's text, and writes the part each member covers into the member's own selection, so
//! every member paints its part with the usual highlight. Members join when they are drawn
//! (and again every frame), so a view made anew for a row that scrolled back in shows its part
//! of the selection at once.
//!
//! Finding where a drag is: a member only knows where its own text was laid out in the frame
//! it was last drawn in. Each member drawn in the current frame listens to the mouse; in the
//! capture phase of an event each puts forward the point of its own text nearest the pointer,
//! and the first one in the bubble phase picks the best and moves the selection. A member
//! not drawn this frame has no listener and so never answers with stale bounds.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::{Rc, Weak};

use gpui::{App, EntityId, FocusHandle, Pixels, Point, SharedString, Window};

use super::element::{FrameUnit, Shared};
use super::rich::Unit;
use super::selection::{Selection, TextPoint};

/// The end of a member's text, whatever its length.
const END: TextPoint = TextPoint { unit: usize::MAX, offset: usize::MAX };

/// A point in a group: member `key`, point `at` of its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GroupPoint {
    pub key: usize,
    pub at: TextPoint,
}

/// How near a member's text is to the pointer; smaller is nearer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Rank {
    /// The pointer is on a unit's text box.
    Inside,
    /// Level with a unit, this far to its side.
    Level(f32),
    /// Below a unit whose bottom is at this y: the selection runs to its end.
    Below(f32),
    /// Above everything: the start of the first unit.
    Above,
}

impl Rank {
    /// Whether `self` (of member point `a`) beats `other` (of `b`).
    fn beats(self, a: GroupPoint, other: Rank, b: GroupPoint) -> bool {
        use Rank::*;
        match (self, other) {
            (Inside, Inside) => a < b,
            (Inside, _) => true,
            (_, Inside) => false,
            (Level(x), Level(y)) => x < y || (x == y && a < b),
            (Level(_), _) => true,
            (_, Level(_)) => false,
            (Below(x), Below(y)) => x > y || (x == y && a > b),
            (Below(_), _) => true,
            (_, Below(_)) => false,
            (Above, Above) => a < b,
        }
    }
}

/// The point of `frame` (one member's units as laid out this frame) a drag at `pos` puts the
/// selection head on, and how near it is: the rules of a single view, ranked so the members
/// can be compared.
pub(crate) fn nearest(frame: &[FrameUnit], pos: Point<Pixels>) -> Option<(Rank, TextPoint)> {
    let at = |f: &FrameUnit| TextPoint { unit: f.unit, offset: f.layout.offset_at(pos - f.bounds.origin) };
    if let Some(f) = frame.iter().find(|f| f.bounds.contains(&pos)) {
        return Some((Rank::Inside, at(f)));
    }
    let h_dist = |f: &FrameUnit| {
        let (x, l, r) = (f32::from(pos.x), f32::from(f.bounds.left()), f32::from(f.bounds.right()));
        if x < l { l - x } else if x > r { x - r } else { 0. }
    };
    let level = frame
        .iter()
        .filter(|f| f.bounds.top() <= pos.y && pos.y < f.bounds.bottom())
        .min_by(|a, b| h_dist(a).total_cmp(&h_dist(b)));
    if let Some(f) = level {
        return Some((Rank::Level(h_dist(f)), at(f)));
    }
    let above = frame.iter().filter(|f| f.bounds.bottom() <= pos.y).max_by(|a, b| {
        f32::from(a.bounds.bottom()).total_cmp(&f32::from(b.bounds.bottom())).then(a.unit.cmp(&b.unit))
    });
    match above {
        Some(f) => Some((Rank::Below(f32::from(f.bounds.bottom())), TextPoint { unit: f.unit, offset: f.layout.rich.text.len() })),
        None => frame.iter().min_by_key(|f| f.unit).map(|f| (Rank::Above, TextPoint { unit: f.unit, offset: 0 })),
    }
}

/// A view in the group.
struct Member {
    shared: Weak<RefCell<Shared>>,
    units: Rc<Vec<Unit>>,
    view: EntityId,
    focus: FocusHandle,
}

/// What the members drawn this frame said about the event being dispatched.
#[derive(Default)]
struct Poll {
    /// Collecting answers for an event at this position.
    open: Option<Point<Pixels>>,
    /// The best point put forward so far.
    best: Option<(Rank, GroupPoint)>,
    /// Some member's text is under the pointer.
    hovered: bool,
}

#[derive(Default)]
struct State {
    members: BTreeMap<usize, Member>,
    anchor: Option<GroupPoint>,
    head: Option<GroupPoint>,
    dragging: bool,
    /// The texts of members by key, for copying members that are not views right now.
    texts: BTreeMap<usize, SharedString>,
    /// The selected part of each member in the selection, as last seen while it was a view.
    parts: BTreeMap<usize, String>,
    poll: Poll,
}

impl State {
    fn ordered(&self) -> Option<(GroupPoint, GroupPoint)> {
        let (a, h) = (self.anchor?, self.head?);
        Some(if a <= h { (a, h) } else { (h, a) })
    }

    /// The selection member `key` shows of its own text.
    fn part_of(&self, key: usize) -> Selection {
        let mut sel = Selection::default();
        let Some((s, e)) = self.ordered() else { return sel };
        if s == e || key < s.key || key > e.key {
            return sel;
        }
        sel.anchor = Some(if key == s.key { s.at } else { TextPoint::default() });
        sel.head = Some(if key == e.key { e.at } else { END });
        sel
    }
}

/// One selection over several text views: a drag that starts in one member and ends in
/// another selects from where it started, through the members in between, to where it ends;
/// every member shows its part of the highlight; Cmd/Ctrl+C in any member copies the parts
/// joined by newlines, in key order; a press anywhere else clears it.
///
/// Make one per list (keep it in the screen's state), and give each view of the list the
/// group and a key that orders it, with [`Markdown::group`](super::Markdown::group) or
/// [`Markdown::set_group`](super::Markdown::set_group). Keys must be unique in the group and
/// stable for a row (a log line's number, not its place on screen), so a selection keeps its
/// rows while the list scrolls or grows.
///
/// A virtual list only has views for the rows on screen. The highlight of a row that comes
/// back is restored when its view is drawn. For the copy, rows without a view are taken
/// from the texts given to [`set_texts`](Self::set_texts) (whole, since they lie inside the
/// selection); without texts such rows are left out.
///
/// ```ignore
/// // In the screen's state:
/// let group = SelectionGroup::new();
/// // Each frame, the texts of every row (cheap: shared strings):
/// group.set_texts(lines.iter().map(|l| (l.seq as usize, l.text.clone().into())));
/// // A row's view, made once:
/// let view = cx.new(|cx| Markdown::verbatim(text, cx).group(&group, seq as usize));
/// ```
#[derive(Clone, Default)]
pub struct SelectionGroup(Rc<RefCell<State>>);

impl SelectionGroup {
    /// An empty group.
    pub fn new() -> Self {
        Self::default()
    }

    /// Both handles are the same group.
    pub fn ptr_eq(&self, other: &SelectionGroup) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    /// The text of every row by key, for copying rows that have no view at the moment.
    pub fn set_texts(&self, texts: impl IntoIterator<Item = (usize, SharedString)>) {
        self.0.borrow_mut().texts = texts.into_iter().collect();
    }

    /// The selected text as a copy gives it: each member's part, in key order, joined by
    /// newlines. `None` when nothing is selected.
    pub fn selected_text(&self) -> Option<String> {
        let st = self.0.borrow();
        let (s, e) = st.ordered()?;
        if s == e {
            return None;
        }
        let mut keys: BTreeSet<usize> = st.members.range(s.key..=e.key).map(|(k, _)| *k).collect();
        keys.extend(st.texts.range(s.key..=e.key).map(|(k, _)| *k));
        keys.extend(st.parts.range(s.key..=e.key).map(|(k, _)| *k));
        let parts: Vec<String> = keys
            .into_iter()
            .filter_map(|key| {
                let live = st.members.get(&key).filter(|m| m.shared.strong_count() > 0);
                if let Some(m) = live {
                    return Some(st.part_of(key).text(&m.units).unwrap_or_default());
                }
                if let Some(part) = st.parts.get(&key) {
                    return Some(part.clone());
                }
                // Not seen since the selection was made: inside it, so whole.
                st.texts.get(&key).map(|t| t.to_string())
            })
            .collect();
        let text = parts.join("\n");
        (!text.is_empty()).then_some(text)
    }

    /// Nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.0.borrow().ordered().is_none_or(|(s, e)| s == e)
    }

    /// Drop the selection.
    pub fn clear(&self, cx: &mut App) {
        {
            let mut st = self.0.borrow_mut();
            st.anchor = None;
            st.head = None;
            st.dragging = false;
        }
        self.apply(cx);
    }

    /// Select everything the group knows: from the start of the first member to the end of
    /// the last one, by key (views and given texts).
    pub fn select_all(&self, cx: &mut App) {
        {
            let mut st = self.0.borrow_mut();
            let first = st.members.keys().next().copied().into_iter().chain(st.texts.keys().next().copied()).min();
            let last = st.members.keys().next_back().copied().into_iter().chain(st.texts.keys().next_back().copied()).max();
            let (Some(first), Some(last)) = (first, last) else { return };
            st.anchor = Some(GroupPoint { key: first, at: TextPoint::default() });
            st.head = Some(GroupPoint { key: last, at: END });
        }
        self.apply(cx);
    }

    /// Member `key` is drawn: remember its view, and give it its part of the selection.
    pub(crate) fn join(&self, key: usize, shared: &Rc<RefCell<Shared>>, units: &Rc<Vec<Unit>>, view: EntityId, focus: &FocusHandle) {
        let mut st = self.0.borrow_mut();
        let part = st.part_of(key);
        st.members.insert(key, Member { shared: Rc::downgrade(shared), units: units.clone(), view, focus: focus.clone() });
        if st.members.len().is_multiple_of(64) {
            st.members.retain(|_, m| m.shared.strong_count() > 0);
        }
        shared.borrow_mut().sel = part;
    }

    /// Some member has keyboard focus: the highlight shows in its focused tint everywhere.
    pub(crate) fn focused(&self, window: &Window) -> bool {
        self.0.borrow().members.values().any(|m| m.shared.strong_count() > 0 && m.focus.is_focused(window))
    }

    /// A drag that started in the group is going on.
    pub(crate) fn dragging(&self) -> bool {
        self.0.borrow().dragging
    }

    /// Start a selection at `p`; `drag` when the mouse stays down to extend it.
    pub(crate) fn press(&self, p: GroupPoint, drag: bool, cx: &mut App) {
        {
            let mut st = self.0.borrow_mut();
            st.anchor = Some(p);
            st.head = Some(p);
            st.dragging = drag;
            st.parts.clear();
        }
        self.apply(cx);
    }

    /// Select from `from` to `to` (a word, a line) without a drag.
    pub(crate) fn select(&self, from: GroupPoint, to: GroupPoint, cx: &mut App) {
        {
            let mut st = self.0.borrow_mut();
            st.anchor = Some(from);
            st.head = Some(to);
            st.dragging = false;
            st.parts.clear();
        }
        self.apply(cx);
    }

    /// Move the head to `p` (a shift-click), keeping the anchor; `false` without an anchor.
    pub(crate) fn extend(&self, p: GroupPoint, cx: &mut App) -> bool {
        {
            let mut st = self.0.borrow_mut();
            if st.anchor.is_none() {
                return false;
            }
            st.head = Some(p);
            st.dragging = false;
        }
        self.apply(cx);
        true
    }

    /// The mouse went up.
    pub(crate) fn release(&self) {
        self.0.borrow_mut().dragging = false;
    }

    /// Capture phase of an event at `pos`: member `key` drawn this frame puts forward its
    /// nearest point, and whether its text is under the pointer.
    pub(crate) fn offer(&self, pos: Point<Pixels>, key: usize, near: Option<(Rank, TextPoint)>, hovered: bool) {
        let mut st = self.0.borrow_mut();
        if st.poll.open != Some(pos) {
            st.poll = Poll { open: Some(pos), ..Poll::default() };
        }
        st.poll.hovered |= hovered;
        if let Some((rank, at)) = near {
            let p = GroupPoint { key, at };
            let better = match st.poll.best {
                None => true,
                Some((r, b)) => rank.beats(p, r, b),
            };
            if better {
                st.poll.best = Some((rank, p));
            }
        }
    }

    /// Bubble phase: the first member to ask gets what the members said (the best point, and
    /// whether any text was under the pointer); later ones get `None`.
    pub(crate) fn take_poll(&self) -> Option<(Option<GroupPoint>, bool)> {
        let mut st = self.0.borrow_mut();
        st.poll.open.take()?;
        let best = st.poll.best.take().map(|(_, p)| p);
        Some((best, std::mem::take(&mut st.poll.hovered)))
    }

    /// Move the head of a drag to `p`.
    pub(crate) fn drag_to(&self, p: GroupPoint, cx: &mut App) {
        {
            let mut st = self.0.borrow_mut();
            if st.head == Some(p) {
                return;
            }
            st.head = Some(p);
        }
        self.apply(cx);
    }

    /// Write each member's part into its view, remember the parts for the copy, redraw the
    /// views whose part changed.
    fn apply(&self, cx: &mut App) {
        let mut changed = Vec::new();
        {
            let mut st = self.0.borrow_mut();
            let mut parts = BTreeMap::new();
            for (key, m) in &st.members {
                let Some(shared) = m.shared.upgrade() else { continue };
                let part = st.part_of(*key);
                if !part.is_empty() {
                    parts.insert(*key, part.text(&m.units).unwrap_or_default());
                }
                let mut sh = shared.borrow_mut();
                if sh.sel != part {
                    sh.sel = part;
                    changed.push(m.view);
                }
            }
            // Parts seen earlier stay for members that are no longer views.
            let range = st.ordered().filter(|(s, e)| s != e);
            st.parts.retain(|k, _| range.is_some_and(|(s, e)| s.key <= *k && *k <= e.key));
            st.parts.extend(parts);
        }
        for view in changed {
            cx.notify(view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(key: usize, unit: usize, offset: usize) -> GroupPoint {
        GroupPoint { key, at: TextPoint { unit, offset } }
    }

    #[test]
    fn a_member_shows_the_part_of_the_selection_over_it() {
        let mut st = State { anchor: Some(p(7, 0, 4)), head: Some(p(3, 0, 2)), ..State::default() };
        assert!(st.part_of(2).is_empty() && st.part_of(8).is_empty());
        assert_eq!(st.part_of(3).ordered(), Some((TextPoint { unit: 0, offset: 2 }, END)), "the start runs to the end");
        assert_eq!(st.part_of(5).ordered(), Some((TextPoint::default(), END)), "a member between is whole");
        assert_eq!(st.part_of(7).ordered(), Some((TextPoint::default(), TextPoint { unit: 0, offset: 4 })));
        st.head = Some(p(7, 0, 1));
        assert_eq!(st.part_of(7).ordered(), Some((TextPoint { unit: 0, offset: 1 }, TextPoint { unit: 0, offset: 4 })));
        st.head = st.anchor;
        assert!(st.part_of(7).is_empty(), "a collapsed selection shows nothing");
    }

    #[test]
    fn the_point_on_text_beats_one_beside_it_and_the_lowest_line_above_wins() {
        let (a, b) = (p(1, 0, 0), p(2, 0, 0));
        assert!(Rank::Inside.beats(b, Rank::Level(0.), a));
        assert!(Rank::Level(2.).beats(b, Rank::Level(5.), a));
        assert!(Rank::Below(300.).beats(a, Rank::Below(200.), b), "the line just above the pointer");
        assert!(Rank::Below(200.).beats(b, Rank::Below(200.), a), "the later member on a tie");
        assert!(Rank::Below(0.).beats(b, Rank::Above, a));
        assert!(Rank::Above.beats(a, Rank::Above, b), "above everything: the first member");
    }
}
