//! The draft and the file: loading it from the catalog, following the file on disk, saving a
//! short while after the last edit, and checking the graph for problems after each change.
//!
//! The draft is the file. Every save goes through `Commands::save_pipeline_with_result`, which
//! runs off the UI thread: when the file is read back through the catalog the save landed, and
//! an empty result means it did not (the hub has shown a notice saying why; its words become
//! the header's reason).

use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, App, Context, IntoElement, SharedString, Task, div, prelude::*, px};
use hub::LoadState;
use hub::api::{GraphChange, GraphDef, Origin, Place, PipelineVersion};
use hub::model::{NodeId, RunnableRef};
use hub::{HubNotice, Route};
use ui::dev::Probe;
use ui::prelude::*;

use super::EditorView;

/// How long after the last edit the draft is saved.
pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(800);
/// How long after the last edit the graph is checked.
pub(crate) const CHECK_DELAY: Duration = Duration::from_millis(250);

/// Where loading the pipeline stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Load {
    Loading,
    /// The catalog has no pipeline by this id.
    Missing,
    Failed(SharedString),
    Ready,
}

/// What the header says about saving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveState {
    Saved,
    /// Edited; a save follows shortly.
    Unsaved,
    Saving,
    /// The last save failed; the edits are still unsaved.
    Failed(SharedString),
}

impl SaveState {
    /// The words in the header.
    pub(crate) fn label(&self) -> SharedString {
        match self {
            SaveState::Saved => "Saved".into(),
            SaveState::Unsaved => "Unsaved changes".into(),
            SaveState::Saving => "Saving…".into(),
            SaveState::Failed(why) => format!("Not saved: {why}").into(),
        }
    }
}

/// The editor's side of the file: what is on disk, saves on their way, the checks and the
/// changes against the live version.
pub(super) struct Document {
    pub(super) load: Load,
    /// The catalog's value last looked at; a new one may mean the file changed.
    seen: Option<Arc<Option<GraphDef>>>,
    /// The versions last looked at.
    versions_seen: Option<Arc<Vec<PipelineVersion>>>,
    /// The graph last seen on disk.
    pub(super) disk: Option<GraphDef>,
    /// The file changed on disk under unsaved edits: what is on disk now, until the person
    /// picks "Reload" or "Keep mine".
    pub(super) conflict: Option<GraphDef>,
    /// Graphs handed to the save command whose file has not been read back yet, oldest first.
    pub(super) in_flight: Vec<GraphDef>,
    /// Why the last save failed.
    pub(super) save_error: Option<SharedString>,
    /// The last notice the hub showed while a save was on its way: why it failed, if it did.
    last_notice: Option<SharedString>,
    save_timer: Option<Task<()>>,
    check_timer: Option<Task<()>>,
    /// The node the route asked for, selected once the graph is there.
    pub(super) pending_node: Option<NodeId>,
    /// The file against the live version, for the header and the deploy dialog.
    pub(super) changes: Option<Vec<GraphChange>>,
    changes_task: Option<Task<()>>,
}

impl Document {
    /// Nothing loaded yet; `node` is selected once the graph arrives.
    pub(super) fn new(node: Option<NodeId>) -> Self {
        Self {
            load: Load::Loading,
            seen: None,
            versions_seen: None,
            disk: None,
            conflict: None,
            in_flight: Vec::new(),
            save_error: None,
            last_notice: None,
            save_timer: None,
            check_timer: None,
            pending_node: node,
            changes: None,
            changes_task: None,
        }
    }
}

/// Why the draft cannot be saved right now.
enum Unsavable {
    /// The library is still loading: try again shortly.
    NotYet,
    /// Never from here.
    ReadOnly(SharedString),
}

impl EditorView {
    /// Asks the catalog for the pipeline and its versions, and takes what it has already.
    pub(super) fn start_loading(&mut self, cx: &mut Context<Self>) {
        let id = self.pipeline.clone();
        self.catalog.update(cx, |catalog, cx| {
            catalog.ensure_pipeline(&id, cx);
            catalog.ensure_versions(&id, cx);
        });
        self.catalog_changed(cx);
    }

    /// The catalog notified: the file, its versions or the library may have changed.
    pub(super) fn catalog_changed(&mut self, cx: &mut Context<Self>) {
        let (value, failed, versions) = {
            let catalog = self.catalog.read(cx);
            let slot = catalog.pipeline(&self.pipeline);
            let value = slot.and_then(|slot| slot.value().cloned());
            let failed = slot.and_then(|slot| match slot.state() {
                LoadState::Failed(why) => Some(why.clone()),
                _ => None,
            });
            let versions = catalog.versions(&self.pipeline).and_then(|v| v.value().cloned());
            (value, failed, versions)
        };
        if let Some(versions) = versions
            && !self.doc.versions_seen.as_ref().is_some_and(|seen| Arc::ptr_eq(seen, &versions))
        {
            self.doc.versions_seen = Some(versions);
            self.refresh_changes(cx);
        }
        match value {
            Some(value) => {
                if self.doc.seen.as_ref().is_some_and(|seen| Arc::ptr_eq(seen, &value)) {
                    return;
                }
                self.doc.seen = Some(value.clone());
                match value.as_ref() {
                    Some(graph) => self.disk_changed(graph.clone(), cx),
                    None if self.parts.draft.read(cx).graph().is_none() => self.doc.load = Load::Missing,
                    None => {}
                }
            }
            None => {
                if let Some(why) = failed
                    && self.doc.load == Load::Loading
                {
                    self.doc.load = Load::Failed(why);
                }
            }
        }
        cx.notify();
    }

    /// The file on disk is `graph` now.
    fn disk_changed(&mut self, graph: GraphDef, cx: &mut Context<Self>) {
        if self.doc.disk.as_ref() == Some(&graph) {
            return;
        }
        self.doc.disk = Some(graph.clone());
        self.refresh_changes(cx);
        let draft = self.parts.draft.clone();
        if draft.read(cx).graph().is_none() {
            draft.update(cx, |draft, cx| draft.load(graph, cx));
            self.doc.load = Load::Ready;
            if let Some(node) = self.doc.pending_node.take() {
                self.select_node(node, false, cx);
            }
            return;
        }
        if !self.doc.in_flight.is_empty() {
            // One of our saves landing. A file read back slightly differently from what was
            // written still ends every save sent before it.
            let landed = self.doc.in_flight.iter().position(|sent| *sent == graph);
            match landed {
                Some(ix) => drop(self.doc.in_flight.drain(..=ix)),
                None => self.doc.in_flight.clear(),
            }
            return;
        }
        let (same, dirty) = {
            let draft = draft.read(cx);
            (draft.graph() == Some(&graph), draft.is_dirty())
        };
        if same {
            draft.update(cx, |draft, cx| draft.mark_saved(graph, cx));
            self.doc.conflict = None;
        } else if dirty {
            self.doc.conflict = Some(graph);
        } else {
            draft.update(cx, |draft, cx| draft.load(graph, cx));
            self.doc.conflict = None;
        }
    }

    /// What the header says about saving.
    pub(crate) fn save_state(&self, cx: &App) -> SaveState {
        let dirty = self.parts.draft.read(cx).is_dirty();
        if !self.doc.in_flight.is_empty() {
            SaveState::Saving
        } else if let Some(why) = self.doc.save_error.clone().filter(|_| dirty) {
            SaveState::Failed(why)
        } else if dirty {
            SaveState::Unsaved
        } else {
            SaveState::Saved
        }
    }

    /// Where the pipeline's file is written.
    fn place(&self, cx: &App) -> Result<Place, Unsavable> {
        let catalog = self.catalog.read(cx);
        if *catalog.state() == LoadState::Loading {
            return Err(Unsavable::NotYet);
        }
        let reference = RunnableRef::pipeline(self.pipeline.as_str());
        let Some(runnable) = catalog.runnable(&reference) else {
            return Err(Unsavable::ReadOnly("it is not in the library".into()));
        };
        match &runnable.origin {
            Origin::Library => Ok(Place::Library),
            Origin::Project { repo } => Ok(Place::Project(repo.clone())),
            Origin::Builtin => Err(Unsavable::ReadOnly("built-in pipelines are read-only".into())),
            Origin::ClaudeCode { .. } | Origin::Spawned { .. } => {
                Err(Unsavable::ReadOnly("this pipeline is read-only".into()))
            }
        }
    }

    /// Saves after [`SAVE_DELAY`] unless another edit comes first.
    pub(super) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.parts.draft.read(cx).is_dirty() {
            self.doc.save_timer = None;
            return;
        }
        self.doc.save_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update(cx, |this, cx| this.save_now(cx)).ok();
        }));
    }

    /// Hands the draft to the save command now, if it has unsaved edits. Waits while the
    /// file changed under the edits and the person has not chosen yet.
    pub(super) fn save_now(&mut self, cx: &mut Context<Self>) {
        self.doc.save_timer = None;
        if self.doc.conflict.is_some() {
            return;
        }
        let graph = {
            let draft = self.parts.draft.read(cx);
            match draft.graph() {
                Some(graph) if draft.is_dirty() => graph.clone(),
                _ => return,
            }
        };
        let place = match self.place(cx) {
            Ok(place) => place,
            Err(Unsavable::NotYet) => {
                self.schedule_save(cx);
                return;
            }
            Err(Unsavable::ReadOnly(why)) => {
                self.doc.save_error = Some(why);
                cx.notify();
                return;
            }
        };
        self.doc.save_error = None;
        self.doc.last_notice = None;
        self.doc.in_flight.push(graph.clone());
        self.parts.draft.update(cx, |draft, cx| draft.mark_saved(graph.clone(), cx));
        let saved = hub::commands(cx).save_pipeline_with_result(place, graph.clone(), cx);
        cx.spawn(async move |this, cx| {
            if saved.await.is_none() {
                this.update(cx, |this, cx| this.save_failed(&graph, cx)).ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// The save of `graph` failed: the draft is unsaved again and the header says why.
    fn save_failed(&mut self, graph: &GraphDef, cx: &mut Context<Self>) {
        if !self.doc.in_flight.contains(graph) {
            return;
        }
        self.doc.in_flight.clear();
        let why = self.doc.last_notice.take().unwrap_or_else(|| "the save failed".into());
        self.doc.save_error = Some(why);
        if let Some(disk) = self.doc.disk.clone() {
            self.parts.draft.update(cx, |draft, cx| draft.mark_saved(disk, cx));
        }
        cx.notify();
    }

    /// The view is going away: what is not saved yet is saved, whatever else is pending.
    pub(super) fn save_on_release(&mut self, cx: &mut App) {
        let graph = {
            let draft = self.parts.draft.read(cx);
            match draft.graph() {
                Some(graph) if draft.is_dirty() => graph.clone(),
                _ => return,
            }
        };
        if let Ok(place) = self.place(cx) {
            hub::commands(cx).save_pipeline(place, graph, cx);
        }
    }

    /// Remembers what the hub said while a save is on its way: if the save then fails, the
    /// notice (shown before the empty result arrives) says why.
    pub(super) fn hub_notice(&mut self, notice: &HubNotice, _cx: &mut Context<Self>) {
        if !self.doc.in_flight.is_empty() {
            self.doc.last_notice = Some(notice.detail.clone().unwrap_or_else(|| notice.text.clone()));
        }
    }

    /// Checks the graph after [`CHECK_DELAY`]; the answer is stored for the revision it was
    /// asked for, so an answer for an older graph is dropped by the draft.
    pub(super) fn schedule_check(&mut self, cx: &mut Context<Self>) {
        let draft = self.parts.draft.clone();
        self.doc.check_timer = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(CHECK_DELAY).await;
            let Ok(Some((graph, revision))) =
                draft.read_with(cx, |draft, _| draft.graph().cloned().map(|graph| (graph, draft.revision())))
            else {
                return;
            };
            let Ok(asked) = cx.update(|cx| hub::queries(cx).pipeline_problems(graph, cx)) else { return };
            let problems = asked.await;
            draft.update(cx, |draft, cx| draft.set_problems(revision, problems, cx)).ok();
        }));
    }

    /// Reads the file against the live version again.
    pub(super) fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        let asked = hub::queries(cx).pipeline_changes(&self.pipeline, cx);
        self.doc.changes_task = Some(cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.doc.changes = result.ok();
                cx.notify();
            })
            .ok();
        }));
    }

    /// "Reload": takes the file as it is on disk and drops the edits.
    pub(super) fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        if let Some(graph) = self.doc.conflict.take() {
            self.parts.draft.update(cx, |draft, cx| draft.load(graph, cx));
            cx.notify();
        }
    }

    /// "Keep mine": the edits win and are saved over the file.
    pub(super) fn keep_mine(&mut self, cx: &mut Context<Self>) {
        if self.doc.conflict.take().is_some() {
            self.save_now(cx);
            cx.notify();
        }
    }

    /// The notice over the canvas while the file changed under the edits.
    pub(super) fn conflict_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.doc.conflict.as_ref()?;
        let banner = Banner::new()
            .tone(BannerTone::Warning)
            .child(icon(IconName::Alert, 16., WARNING))
            .child(
                col()
                    .flex_1()
                    .child(div().type_style(BODY_STRONG).child("The file changed on disk"))
                    .child(
                        div()
                            .type_style(LABEL)
                            .text_color(TEXT_SECONDARY)
                            .child("Someone saved this pipeline while you were editing it."),
                    ),
            )
            .child(
                Button::new("editor-reload", "Reload")
                    .secondary()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.reload_from_disk(cx)))
                    .probe("editor-reload"),
            )
            .child(
                Button::new("editor-keep-mine", "Keep mine")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx)))
                    .probe("editor-keep-mine"),
            );
        Some(div().absolute().top(px(SPACE_3)).left(px(SPACE_5)).right(px(SPACE_5)).child(banner).into_any_element())
    }

    /// The content while the pipeline is loading, missing or failed.
    pub(super) fn render_placeholder(&self, load: Load, cx: &mut Context<Self>) -> AnyElement {
        let (title, detail) = match load {
            Load::Loading | Load::Ready => ("Loading the pipeline…".to_string(), None),
            Load::Missing => (format!("There is no pipeline {}", self.pipeline), Some("It may have been deleted or renamed.".to_string())),
            Load::Failed(why) => ("Could not load the pipeline".to_string(), Some(why.to_string())),
        };
        let missing = self.doc.load != Load::Loading;
        col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(SPACE_2))
            .child(div().type_style(HEADING).child(title))
            .children(detail.map(|d| div().type_style(LABEL).text_color(TEXT_MUTED).child(d)))
            .when(missing, |d| {
                d.child(
                    div().mt(px(SPACE_2)).child(
                        Button::new("editor-back", "All agents & pipelines")
                            .secondary()
                            .on_click(cx.listener(|_, _, _, cx| {
                                hub::navigate(Route::Agents { runnable: None }, cx);
                            })),
                    ),
                )
            })
            .probe("editor-placeholder")
            .into_any_element()
    }
}
