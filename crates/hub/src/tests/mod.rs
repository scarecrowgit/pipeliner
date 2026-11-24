//! Tests of the hub against the fixture backend.

mod fixtures;
mod misc;
mod models;
mod pump;
mod session;
mod views;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Entity, TestAppContext};

use crate::Hub;
use crate::api::{AgentEvent, Event, SessionId};
use crate::fixtures::{FixtureApi, World};

/// The fixed clock of the test world.
pub(crate) const NOW: i64 = 1_795_000_000_000;

/// A hub over the Acme world, with a fixed clock.
pub(crate) fn setup(cx: &mut TestAppContext) -> (Arc<FixtureApi>, Entity<Hub>) {
    let api = Arc::new(FixtureApi::new(World::acme(NOW)));
    let hub = cx.update(|cx| Hub::new(api.clone(), cx));
    (api, hub)
}

/// Counts notifications of an entity.
pub(crate) fn notifications<T: 'static>(cx: &mut TestAppContext, entity: &Entity<T>) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(0));
    let counter = count.clone();
    cx.update(|cx| cx.observe(entity, move |_, _| counter.set(counter.get() + 1)).detach());
    count
}

/// Collects the events an entity emits.
pub(crate) fn collect<T, E: Clone + 'static>(cx: &mut TestAppContext, entity: &Entity<T>) -> Rc<RefCell<Vec<E>>>
where
    T: gpui::EventEmitter<E> + 'static,
{
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|cx| cx.subscribe(entity, move |_, event: &E, _| sink.borrow_mut().push(event.clone())).detach());
    events
}

pub(crate) fn agent(session: &str, event: AgentEvent) -> Event {
    Event::Agent { session: SessionId::new(session), event }
}
