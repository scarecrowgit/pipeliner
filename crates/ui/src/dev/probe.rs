//! Probes: elements named for the dev driver. A probe records its bounds (and optionally a
//! text value) under its name each frame; pointer commands aim at the visible centre and
//! GPUI's own hit testing does the rest.
//!
//! Limit: a probe inside a `.cached()` view records only on frames where that view is
//! re-rendered (GPUI replays a cached view's prepaint without calling its elements). No view
//! in this workspace is cached today.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
    InteractiveElement, Interactivity, IntoElement, LayoutId, ParentElement, Pixels, Point,
    SharedString, StatefulInteractiveElement, StyleRefinement, Styled, Window, point, px,
};

use super::enabled;

/// Names an element so the dev driver can find it. A no-op unless dev mode is on.
pub trait Probe: IntoElement {
    /// Record this element's bounds under `name` each frame.
    fn probe(self, name: impl Into<SharedString>) -> Probed<Self> {
        Probed { inner: self, name: name.into(), value: None }
    }

    /// Like [`probe`](Probe::probe), with a value a script can read back with `text <name>`
    /// (a count, the text of a field).
    fn probe_value(
        self,
        name: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Probed<Self> {
        Probed { inner: self, name: name.into(), value: Some(value.into()) }
    }
}

impl<E: IntoElement> Probe for E {}

/// An element wrapped by [`Probe::probe`]. Styling, interaction and children pass through to
/// the inner element, so `.probe(…)` can sit anywhere in a builder chain.
pub struct Probed<E> {
    inner: E,
    name: SharedString,
    value: Option<SharedString>,
}

impl<E: IntoElement> IntoElement for Probed<E> {
    type Element = ProbeElement;

    fn into_element(self) -> ProbeElement {
        ProbeElement { name: self.name, value: self.value, child: self.inner.into_any_element() }
    }
}

impl<E: Styled> Styled for Probed<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl<E: ParentElement> ParentElement for Probed<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements)
    }
}

impl<E: InteractiveElement> InteractiveElement for Probed<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inner.interactivity()
    }
}

impl<E: StatefulInteractiveElement> StatefulInteractiveElement for Probed<E> {}

/// The element behind [`Probed`]: shares its child's layout node and records the child's
/// bounds during prepaint.
pub struct ProbeElement {
    name: SharedString,
    value: Option<SharedString>,
    child: AnyElement,
}

impl IntoElement for ProbeElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ProbeElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if enabled() {
            let visible = window.content_mask().bounds.intersect(&bounds);
            let probe = ProbeRecord {
                name: self.name.clone(),
                value: self.value.clone(),
                bounds: rect(bounds),
                visible: rect(visible),
            };
            record(window.window_handle().window_id().as_u64(), probe, cx);
        }
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// `[x, y, width, height]` in window pixels.
pub(super) type Rect = [f32; 4];

pub(super) fn rect(b: Bounds<Pixels>) -> Rect {
    [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]
}

/// One probe as painted in a frame.
#[derive(Clone)]
pub(super) struct ProbeRecord {
    pub name: SharedString,
    pub value: Option<SharedString>,
    pub bounds: Rect,
    pub visible: Rect,
}

impl ProbeRecord {
    pub fn is_visible(&self) -> bool {
        self.visible[2] > 0.5 && self.visible[3] > 0.5
    }

    /// The centre of the visible part: where pointer commands aim.
    pub fn centre(&self) -> Point<Pixels> {
        let [x, y, w, h] = self.visible;
        point(px(x + w / 2.), px(y + h / 2.))
    }
}

/// Probes by window: the frame being drawn and the last finished one. UI thread only.
#[derive(Default)]
struct Store {
    /// Windows whose frame is being collected; probes push without scheduling a publish.
    open: HashSet<u64>,
    building: HashMap<u64, Vec<ProbeRecord>>,
    last: HashMap<u64, Vec<ProbeRecord>>,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

/// Add a probe to the frame being drawn. On an ordinary frame the first probe schedules the
/// publish: effects deferred during a draw flush once the draw's update returns.
fn record(window: u64, probe: ProbeRecord, cx: &mut App) {
    let first = STORE.with_borrow_mut(|store| {
        let first = store.open.insert(window);
        let list = store.building.entry(window).or_default();
        if first {
            list.clear();
        }
        // Lists measure some children twice per frame; keep one record per name and place.
        list.retain(|p| !(p.name == probe.name && p.bounds == probe.bounds));
        list.push(probe);
        first
    });
    if first {
        cx.defer(move |_| end_frame(window));
    }
}

/// Start collecting a frame the driver forced (called just before its draw), so a frame
/// without probes is published too, as an empty list.
pub(super) fn begin_frame(window: u64) {
    STORE.with_borrow_mut(|store| {
        store.open.insert(window);
        store.building.insert(window, Vec::new());
    })
}

/// Publish the collected frame as the window's probe list.
pub(super) fn end_frame(window: u64) {
    STORE.with_borrow_mut(|store| {
        if store.open.remove(&window) {
            let list = store.building.remove(&window).unwrap_or_default();
            store.last.insert(window, list);
        }
    })
}

/// The probe named `name`, the visible one first, searching `prefer`'s window first.
pub(super) fn find(name: &str, prefer: Option<u64>) -> Option<(u64, ProbeRecord)> {
    STORE.with_borrow(|store| {
        let mut windows: Vec<u64> = store.last.keys().copied().collect();
        windows.sort_by_key(|w| (Some(*w) != prefer, *w));
        windows.into_iter().find_map(|w| {
            let list = store.last.get(&w)?;
            let hit = list
                .iter()
                .find(|p| p.name == name && p.is_visible())
                .or_else(|| list.iter().find(|p| p.name == name))?;
            Some((w, hit.clone()))
        })
    })
}

/// Every probe of the last frame of every window, as JSON.
pub(super) fn probes_json() -> String {
    STORE.with_borrow(|store| {
        let mut windows: Vec<&u64> = store.last.keys().collect();
        windows.sort();
        let mut items = Vec::new();
        for w in windows {
            for p in store.last.get(w).into_iter().flatten() {
                let [x, y, width, height] = p.bounds;
                let value =
                    p.value.as_ref().map_or_else(|| "null".into(), |v| super::proto::json_string(v));
                items.push(format!(
                    "{{\"name\":{},\"window\":{w},\"x\":{x},\"y\":{y},\"width\":{width},\
                     \"height\":{height},\"visible\":{},\"value\":{value}}}",
                    super::proto::json_string(&p.name),
                    p.is_visible(),
                ));
            }
        }
        format!("[{}]", items.join(","))
    })
}
