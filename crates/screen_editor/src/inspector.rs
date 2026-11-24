//! The inspector: the fields of the selected node or edge, or of the pipeline when nothing is
//! selected.
//!
//! It never keeps a copy of the graph: every frame reads the draft, and every change goes
//! through [`Draft::edit`] (typing folds into one undo step per field, sealed when the field
//! loses focus). The text fields live in [`fields`]; each node kind draws its sections in its
//! own module.
//!
//! | Module | Shows |
//! | --- | --- |
//! | [`pipeline`] | nothing selected: name, description, limits, workdirs |
//! | [`agent`] | agent and orchestrator nodes, sub-agent templates |
//! | [`grants`] | model, tools, skills and spawn policy, shared by agents and sub-agents |
//! | [`router`] | router rules |
//! | [`kinds`] | source, file, review, action and pipeline nodes |
//! | [`edge`] | the selected edge |
//! | [`form`] | sections, labelled fields, pickers, removable tags |
//! | [`ops`] | graph edits and lookups the fields need |

mod agent;
mod edge;
mod fields;
mod form;
mod grants;
mod kinds;
mod ops;
mod pipeline;
mod router;
#[cfg(test)]
mod tests;

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px,
};
use hub::api::ProviderModels;
use hub::model::{GraphDef, NodeDef, NodeId, NodeKind, Severity};
use hub::{CatalogModel, RunModel};
use ui::dev::Probe;
use ui::prelude::*;

use crate::selectable::Texts;
use crate::{Draft, DraftEvent, Selection};
use fields::Fields;

/// The inspector of one draft.
pub struct InspectorView {
    draft: Entity<Draft>,
    /// What the fields are bound to; follows the draft's selection.
    shown: Selection,
    fields: Fields,
    catalog: Entity<CatalogModel>,
    /// Providers and their models, for the model pickers.
    models: Vec<ProviderModels>,
    /// The workdir whose fields are open in the pipeline view.
    open_workdir: Option<usize>,
    /// A workdir that could not be removed, and why.
    blocked_workdir: Option<(usize, String)>,
    /// The test run on show, for the spend of the selected node.
    run: Option<(Entity<RunModel>, Subscription)>,
    scroll: ScrollHandle,
    /// A field to put the caret in once it is drawn.
    pending_focus: Option<PendingFocus>,
    /// Problems and field messages, selectable.
    texts: Texts,
    _subs: Vec<Subscription>,
}

impl InspectorView {
    /// An inspector over `draft`; it follows the selection and edits through [`Draft::edit`].
    pub fn new(draft: Entity<Draft>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let catalog = hub::catalog(cx);
        let subs = vec![
            cx.subscribe_in(&draft, window, |this, _, event: &DraftEvent, _, cx| {
                match event {
                    DraftEvent::Changed | DraftEvent::Selected => this.follow_selection(cx),
                    DraftEvent::RunShown => this.follow_run(cx),
                    DraftEvent::Checked => {}
                }
                cx.notify();
            }),
            cx.observe(&catalog, |_, _, cx| cx.notify()),
        ];
        let models = hub::queries(cx).models(cx);
        cx.spawn(async move |this, cx| {
            let models = models.await;
            this.update(cx, |this, cx| {
                this.models = models;
                cx.notify();
            })
            .ok();
        })
        .detach();
        let shown = draft.read(cx).selection().clone();
        let mut view = Self {
            draft,
            shown,
            fields: Fields::default(),
            catalog,
            models: Vec::new(),
            open_workdir: None,
            blocked_workdir: None,
            run: None,
            scroll: ScrollHandle::new(),
            pending_focus: None,
            texts: Texts::default(),
            _subs: subs,
        };
        view.follow_run(cx);
        view
    }

    /// The draft it shows.
    pub fn draft(&self) -> &Entity<Draft> {
        &self.draft
    }

    /// What the inspector shows: the draft's selection as of the last event.
    pub fn shown(&self) -> &Selection {
        &self.shown
    }

    /// Puts the caret in the first text field (the name) once it is drawn: after adding a
    /// node, or when the person presses Enter on a selected one.
    pub fn focus_first_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A node added a moment ago is selected, but the draft's event has not reached this
        // view yet: drop the fields of what it showed before, or the old name takes focus.
        self.follow_selection(cx);
        match self.first_field() {
            Some(editor) => editor.focus(window, cx),
            None => {
                self.pending_focus = Some(PendingFocus::First);
                cx.notify();
            }
        }
    }

    /// The name field, or the first field drawn when there is no name (an edge). Fields are
    /// not made in the order they are shown: an agent makes its role before its name.
    fn first_field(&self) -> Option<fields::Editor> {
        self.fields.editor("insp-name").or_else(|| self.fields.keys().first().and_then(|key| self.fields.editor(key)))
    }

    /// Puts the caret in field `key` once it is drawn: a field that just appeared (a new
    /// rule's label, a new workdir's name).
    fn focus_field_when_drawn(&mut self, key: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.pending_focus = Some(PendingFocus::Field(key.into()));
        cx.notify();
    }

    /// The text of field `key` (its probe name) as last drawn.
    pub fn field_text(&self, key: &str, cx: &gpui::App) -> Option<String> {
        self.fields.editor(key).map(|editor| match editor {
            fields::Editor::Line(input) => input.read(cx).text().to_string(),
            fields::Editor::Area(area) => area.read(cx).text().to_string(),
        })
    }

    /// Rebinds the fields when the draft's selection moved on.
    fn follow_selection(&mut self, cx: &mut Context<Self>) {
        let selection = self.draft.read(cx).selection().clone();
        if selection != self.shown {
            self.draft.update(cx, |draft, _| draft.seal());
            self.fields.reset();
            self.shown = selection;
            self.open_workdir = None;
            self.blocked_workdir = None;
            self.pending_focus = None;
            self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        }
    }

    fn follow_run(&mut self, cx: &mut Context<Self>) {
        let id = self.draft.read(cx).run().cloned();
        self.run = id.map(|id| {
            let run = hub::run(&id, cx);
            let sub = cx.observe(&run, |_, _, cx| cx.notify());
            (run, sub)
        });
    }

    /// Changes the graph as one undo step named `what`.
    fn edit(&mut self, what: &'static str, change: impl FnOnce(&mut GraphDef), cx: &mut Context<Self>) {
        self.draft.update(cx, |draft, cx| draft.edit(what, change, cx));
    }

    /// Changes the node on show as one undo step.
    fn edit_node(&mut self, what: &'static str, change: impl FnOnce(&mut NodeDef), cx: &mut Context<Self>) {
        let Selection::Node(id) = self.shown.clone() else { return };
        self.edit(
            what,
            move |graph| {
                if let Some(node) = ops::node_mut(graph, &id) {
                    change(node);
                }
            },
            cx,
        );
    }

    /// Selects `selection` on the canvas too.
    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.draft.update(cx, |draft, cx| draft.select(selection, cx));
    }

    /// The draft's problems about node `id` (or about the pipeline itself when `None`).
    fn problems_of(&self, id: Option<&NodeId>, cx: &gpui::App) -> Vec<(Severity, String)> {
        self.draft
            .read(cx)
            .problems()
            .iter()
            .filter(|problem| problem.node.as_ref() == id)
            .map(|problem| (problem.severity, problem.message.clone()))
            .collect()
    }

    /// The first problem of node `id` that is plainly about `field`.
    fn problem_at(&self, id: &NodeId, field: &str, cx: &gpui::App) -> Option<SharedString> {
        self.problems_of(Some(id), cx)
            .into_iter()
            .find(|(_, message)| ops::problem_field(message).0 == Some(field) && ops::problem_field(message).1.is_none())
            .map(|(_, message)| message.into())
    }

    fn node_view(&mut self, graph: &GraphDef, node: &NodeDef, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let header = self.header(form::kind_icon(&node.kind), form::kind_tone(&node.kind), node.name.clone(), kind_word(node), true, window, cx);
        let mut sections = Vec::new();
        let problems = self.problems_of(Some(&node.id), cx);
        if let Some(problems) = self.problem_list(problems, cx) {
            sections.push(problems);
        }
        match &node.kind {
            NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => {
                sections.extend(self.agent_sections(graph, node, agent, window, cx));
            }
            NodeKind::Subagent(sub) => {
                sections.push(self.identity_section(graph, node, window, cx));
                sections.extend(self.subagent_sections(graph, node, sub, window, cx));
            }
            NodeKind::Router(router) => {
                sections.push(self.identity_section(graph, node, window, cx));
                sections.extend(self.router_sections(graph, node, router, window, cx));
            }
            _ => {
                sections.push(self.identity_section(graph, node, window, cx));
                sections.extend(self.kind_sections(graph, node, window, cx));
            }
        }
        sections.push(self.remove_section("Remove node", cx));
        (header, sections)
    }

    /// The header row: the kind's tile, the name, the kind, and the close button.
    #[expect(clippy::too_many_arguments, reason = "one header row, its parts passed in")]
    fn header(
        &mut self,
        kind_icon: IconName,
        tone: TileTone,
        name: String,
        kind: String,
        closable: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let close = closable.then(|| {
            IconButton::new("insp-close", IconName::X)
                .small()
                .tooltip("Close inspector")
                .on_click(cx.listener(|this, _, _, cx| this.select(Selection::None, cx)))
                .probe("insp-close")
        });
        row()
            .flex_none()
            .h(px(HEADER_HEIGHT))
            .gap(px(SPACE_2))
            .pl(px(SPACE_5))
            .pr(px(SPACE_3))
            .border_b_1()
            .border_color(LINE)
            .child(Tile::new(kind_icon).tone(tone))
            .child(div().flex_shrink().min_w_0().type_style(BODY_STRONG).child(truncate(name)))
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(kind))
            .child(grow())
            .children(close)
            .into_any_element()
    }

    /// The problems of what is shown, in a banner at the top; they can be selected.
    fn problem_list(&mut self, problems: Vec<(Severity, String)>, cx: &mut Context<Self>) -> Option<AnyElement> {
        if problems.is_empty() {
            return None;
        }
        let error = problems.iter().any(|(severity, _)| *severity == Severity::Error);
        let lines: Vec<_> = problems
            .into_iter()
            .map(|(severity, message)| {
                let color = if severity == Severity::Error { DANGER } else { WARNING };
                let message = self.texts.text("problem", message, LABEL, TEXT_SECONDARY, cx);
                row()
                    .items_start()
                    .gap(px(SPACE_2))
                    .child(div().flex_none().pt(px(SPACE_0_5)).child(icon(IconName::Alert, 14., color)))
                    .child(div().w(px(form::INNER - 54.)).child(message))
            })
            .collect();
        Some(
            form::psec()
                .child(
                    div().id("insp-problems").debug_selector(|| "insp-problems".into()).child(
                        Banner::new()
                            .tone(if error { BannerTone::Danger } else { BannerTone::Warning })
                            .child(col().gap(px(SPACE_1_5)).children(lines)),
                    ),
                )
                .probe("insp-problems")
                .into_any_element(),
        )
    }

    /// The last section: a danger button that removes what is shown.
    fn remove_section(&mut self, label: &'static str, cx: &mut Context<Self>) -> AnyElement {
        form::psec()
            .border_b_0()
            .child(
                row().ml(px(-SPACE_2)).child(
                    Button::new("insp-delete", label)
                        .danger()
                        .small()
                        .icon(IconName::X)
                        .on_click(cx.listener(|this, _, _, cx| this.remove_shown(cx)))
                        .probe("insp-delete"),
                )
                .child(grow()),
            )
            .into_any_element()
    }

    /// Removes the selected node or edge.
    fn remove_shown(&mut self, cx: &mut Context<Self>) {
        match self.shown.clone() {
            Selection::Node(id) => self.edit("Remove node", |graph| ops::remove_node(graph, &id), cx),
            Selection::Edge(index) => {
                self.edit(
                    "Remove edge",
                    |graph| {
                        if index < graph.edges.len() {
                            graph.edges.remove(index);
                        }
                    },
                    cx,
                );
            }
            Selection::None => return,
        }
        self.select(Selection::None, cx);
    }
}

/// Which field takes the caret once drawn.
enum PendingFocus {
    /// The first text field (the name).
    First,
    /// The field of this key.
    Field(SharedString),
}

/// The word after the name in the header: the kind, and the action's kind for actions.
fn kind_word(node: &NodeDef) -> String {
    match &node.kind {
        NodeKind::Action(action) => format!("Action · {}", ops::action_label(ops::action_kind(action))),
        kind => kind.label().to_string(),
    }
}

impl Render for InspectorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_selection(cx);
        self.fields.begin_frame();
        self.texts.begin_frame();
        let graph = self.draft.read(cx).graph().cloned();
        let (header, sections) = match (&graph, self.shown.clone()) {
            (None, _) => (
                div().into_any_element(),
                vec![form::psec().child(Note::new("Loading the pipeline…")).into_any_element()],
            ),
            (Some(graph), Selection::None) => self.pipeline_view(graph, window, cx),
            (Some(graph), Selection::Node(id)) => match graph.node(&id) {
                Some(node) => {
                    let node = node.clone();
                    self.node_view(graph, &node, window, cx)
                }
                None => self.pipeline_view(graph, window, cx),
            },
            (Some(graph), Selection::Edge(index)) => self.edge_view(graph, index, window, cx),
        };
        self.fields.end_frame();
        self.texts.end_frame();
        let wanted = match &self.pending_focus {
            Some(PendingFocus::First) => self.first_field(),
            Some(PendingFocus::Field(key)) => self.fields.editor(key),
            None => None,
        };
        if let Some(editor) = wanted {
            self.pending_focus = None;
            window.defer(cx, move |window, cx| editor.focus(window, cx));
        }
        col()
            .debug_selector(|| "inspector".into())
            .flex_none()
            .w(px(PANEL_WIDTH))
            .h_full()
            .min_h_0()
            .bg(BG)
            .child(header)
            .child(
                div()
                    .id("insp-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(col().children(sections.into_iter().map(|s| div().flex_shrink_0().child(s)))),
            )
            .probe("inspector")
    }
}
