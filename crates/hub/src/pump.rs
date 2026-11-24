//! The event pump: one foreground task drains the engine's events in batches and hands each
//! event to the one model it concerns. Each touched model applies its batch and notifies once.

use std::collections::HashMap;

use async_channel::Receiver;
use gpui::{AsyncApp, Context, Entity, WeakEntity};

use crate::api::{Event, RequestId, ReviewId, Run, RunId, SessionId};
use crate::hub::Hub;
use crate::models::{
    IndexInput, InboxInput, IssueInput, PermissionInput, ReviewModel, RunInput, RunModel, SessionInput, SessionModel,
};
use crate::notice::HubNotice;

/// Awaits events until the engine closes the stream or the hub is dropped.
pub(crate) async fn run(events: Receiver<Event>, hub: WeakEntity<Hub>, cx: &mut AsyncApp) {
    while let Ok(first) = events.recv().await {
        let mut batch = vec![first];
        while let Ok(event) = events.try_recv() {
            batch.push(event);
        }
        if hub.update(cx, |hub, cx| hub.dispatch(batch, cx)).is_err() {
            break;
        }
    }
}

/// One batch, sorted by the model each event is for.
#[derive(Default)]
struct Routed {
    index: Vec<IndexInput>,
    sessions: HashMap<SessionId, Vec<SessionInput>>,
    issues: Vec<IssueInput>,
    runs: Vec<Run>,
    run_models: HashMap<RunId, Vec<RunInput>>,
    inbox: Vec<InboxInput>,
    reviews: HashMap<ReviewId, crate::api::Review>,
    permissions: Vec<PermissionInput>,
    /// Requests seen in this batch, so a resolution in the same batch finds its session.
    requested: HashMap<RequestId, SessionId>,
    catalog_changed: bool,
    spend_changed: bool,
    settings_changed: bool,
    autopilots_changed: bool,
    runtimes: Vec<crate::api::Runtime>,
    /// Something Home's needs-you or working-now rows depend on.
    live_changed: bool,
    notices: Vec<HubNotice>,
}

impl Hub {
    pub(crate) fn dispatch(&mut self, batch: Vec<Event>, cx: &mut Context<Self>) {
        let routed = self.route(batch, cx);
        self.deliver(routed, cx);
    }

    fn route(&mut self, batch: Vec<Event>, cx: &mut Context<Self>) -> Routed {
        let mut routed = Routed::default();
        for event in batch {
            match event {
                Event::SessionChanged { session } => {
                    routed.live_changed = true;
                    if let Some(parent) = session.parent.clone() {
                        self.to_session(&mut routed, &parent, SessionInput::Child(session.clone()));
                    }
                    self.to_session(&mut routed, &session.id.clone(), SessionInput::Header(session.clone()));
                    routed.index.push(IndexInput::Changed(Box::new(session)));
                }
                Event::SessionRemoved { session } => {
                    routed.live_changed = true;
                    self.to_session(&mut routed, &session, SessionInput::Removed);
                    routed.index.push(IndexInput::Removed(session));
                }
                Event::Agent { session, event } => self.to_session(&mut routed, &session, SessionInput::Agent(event)),
                Event::IssueChanged { issue } => routed.issues.push(IssueInput::Changed(issue)),
                Event::IssueRemoved { key } => routed.issues.push(IssueInput::Removed(key)),
                Event::IssueActivity { key, event } => routed.issues.push(IssueInput::Activity(key, event)),
                Event::RunChanged { run } => {
                    routed.live_changed = true;
                    if self.run_models.contains_key(&run.id) {
                        routed.run_models.entry(run.id.clone()).or_default().push(RunInput::Changed(run.clone()));
                    }
                    routed.runs.push(run);
                }
                Event::RunLog { run, line } => {
                    if self.run_models.contains_key(&run) {
                        routed.run_models.entry(run).or_default().push(RunInput::Log(line));
                    }
                }
                Event::ReviewChanged { review } => {
                    routed.live_changed = true;
                    if self.reviews.contains_key(&review.id) {
                        routed.reviews.insert(review.id.clone(), review.clone());
                    }
                    routed.inbox.push(InboxInput::Review(review));
                }
                Event::InboxChanged { item } => {
                    routed.live_changed = true;
                    routed.inbox.push(InboxInput::Item(item));
                }
                Event::InboxRemoved { id } => {
                    routed.live_changed = true;
                    routed.inbox.push(InboxInput::Removed(id));
                }
                Event::PermissionRequested { request } => {
                    routed.live_changed = true;
                    routed.requested.insert(request.id.clone(), request.session.clone());
                    self.to_session(&mut routed, &request.session.clone(), SessionInput::Permission(request.clone()));
                    routed.permissions.push(PermissionInput::Requested(Box::new(request)));
                }
                Event::PermissionResolved { id } => {
                    routed.live_changed = true;
                    let in_batch = routed.requested.get(&id).cloned();
                    let known = in_batch.or_else(|| self.permissions.read(cx).session_of(&id).cloned());
                    let owner = known.or_else(|| {
                        self.sessions.iter().find_map(|(session, weak)| {
                            let model = weak.upgrade()?;
                            model.read(cx).has_request(&id).then(|| session.clone())
                        })
                    });
                    if let Some(session) = owner {
                        self.to_session(&mut routed, &session, SessionInput::Resolved(id.clone()));
                    }
                    routed.permissions.push(PermissionInput::Resolved(id));
                }
                Event::CatalogChanged => routed.catalog_changed = true,
                Event::SpendChanged => routed.spend_changed = true,
                Event::SettingsChanged => {
                    routed.settings_changed = true;
                    routed.issues.push(IssueInput::Settings);
                }
                Event::AutopilotsChanged => routed.autopilots_changed = true,
                Event::RuntimeChanged { runtime } => routed.runtimes.push(runtime),
                Event::Notice { level, text } => routed.notices.push(HubNotice::new(level, text)),
            }
        }
        routed
    }

    /// Queues an input for a session only if a model of it is open.
    fn to_session(&self, routed: &mut Routed, session: &SessionId, input: SessionInput) {
        if self.sessions.contains_key(session) {
            routed.sessions.entry(session.clone()).or_default().push(input);
        }
    }

    fn deliver(&mut self, routed: Routed, cx: &mut Context<Self>) {
        if !routed.index.is_empty() {
            self.chat_index.update(cx, |model, cx| model.apply(routed.index, cx));
        }
        for (id, inputs) in routed.sessions {
            if let Some(model) = live::<SessionModel, _>(&mut self.sessions, &id) {
                model.update(cx, |model, cx| model.apply(inputs, cx));
            }
        }
        if !routed.issues.is_empty() {
            self.issues.update(cx, |model, cx| model.apply(routed.issues, cx));
        }
        if !routed.runs.is_empty() {
            self.runs.update(cx, |model, cx| model.apply(routed.runs, cx));
        }
        for (id, inputs) in routed.run_models {
            if let Some(model) = live::<RunModel, _>(&mut self.run_models, &id) {
                model.update(cx, |model, cx| model.apply(inputs, cx));
            }
        }
        if !routed.inbox.is_empty() {
            self.inbox.update(cx, |model, cx| model.apply(routed.inbox, cx));
        }
        for (id, review) in routed.reviews {
            if let Some(model) = live::<ReviewModel, _>(&mut self.reviews, &id) {
                model.update(cx, |model, cx| model.apply(review, cx));
            }
        }
        if !routed.permissions.is_empty() {
            self.permissions.update(cx, |model, cx| model.apply(routed.permissions, cx));
        }
        if routed.catalog_changed {
            self.catalog.update(cx, |model, cx| model.catalog_changed(cx));
        }
        if routed.spend_changed {
            self.catalog.update(cx, |model, cx| model.spend_changed(cx));
        }
        self.ops.update(cx, |ops, cx| {
            if routed.live_changed {
                ops.live_changed(cx);
            }
            if routed.spend_changed {
                ops.spend_changed(cx);
            }
            if routed.settings_changed {
                ops.settings_changed(cx);
            }
            if routed.autopilots_changed {
                ops.autopilots_changed(cx);
            }
            if !routed.runtimes.is_empty() {
                ops.runtime_changed(routed.runtimes, cx);
            }
        });
        for notice in routed.notices {
            self.notify_user(notice, cx);
        }
    }
}

/// The model behind a weak entry, removing the entry when the model is gone.
fn live<M: 'static, K: std::hash::Hash + Eq>(map: &mut HashMap<K, WeakEntity<M>>, key: &K) -> Option<Entity<M>> {
    let model = map.get(key).and_then(WeakEntity::upgrade);
    if model.is_none() {
        map.remove(key);
    }
    model
}
