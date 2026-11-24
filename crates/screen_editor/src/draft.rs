//! The graph being edited, shared by the canvas, the inspector and the frame.
//!
//! Owned by the lead: the three parts of the editor are written against this API. Every change
//! to the graph goes through [`Draft::edit`], so undo, the dirty mark and the change event
//! cannot be forgotten.

use gpui::{Context, EventEmitter};
use hub::model::{GraphDef, NodeId, PipelineId, Problem, RunId};

/// What is selected on the canvas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    /// Nothing: the inspector shows the pipeline itself.
    #[default]
    None,
    Node(NodeId),
    /// An edge by its index in `GraphDef::edges`.
    Edge(usize),
}

/// What a [`Draft`] tells its observers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftEvent {
    /// The graph changed (an edit, an undo, a redo, a reload).
    Changed,
    /// The selection changed.
    Selected,
    /// The problems of the graph were recomputed.
    Checked,
    /// A test run was put on show, or taken off.
    RunShown,
}

/// How many edits can be undone.
const HISTORY: usize = 100;

/// One pipeline open in the editor.
pub struct Draft {
    pipeline: PipelineId,
    graph: Option<GraphDef>,
    /// The graph as last loaded or saved.
    saved: Option<GraphDef>,
    selection: Selection,
    undo: Vec<(GraphDef, &'static str)>,
    redo: Vec<(GraphDef, &'static str)>,
    /// Bumped on every change to the graph; lets a view tell "which graph did I check".
    revision: u64,
    problems: Vec<Problem>,
    checked_revision: Option<u64>,
    run: Option<RunId>,
}

impl EventEmitter<DraftEvent> for Draft {}

impl Draft {
    /// A draft of `pipeline` with nothing loaded yet.
    pub fn new(pipeline: PipelineId) -> Self {
        Self {
            pipeline,
            graph: None,
            saved: None,
            selection: Selection::None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            problems: Vec::new(),
            checked_revision: None,
            run: None,
        }
    }

    /// The pipeline it edits.
    pub fn pipeline(&self) -> &PipelineId {
        &self.pipeline
    }

    /// The graph, once loaded.
    pub fn graph(&self) -> Option<&GraphDef> {
        self.graph.as_ref()
    }

    /// Counts changes to the graph since the draft was made.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Replaces the graph with what is on disk and forgets the history. Used on first load and
    /// when the file changed underneath an unedited draft.
    pub fn load(&mut self, graph: GraphDef, cx: &mut Context<Self>) {
        self.saved = Some(graph.clone());
        self.graph = Some(graph);
        self.undo.clear();
        self.redo.clear();
        self.revision += 1;
        self.fix_selection();
        cx.emit(DraftEvent::Changed);
        cx.notify();
    }

    /// Changes the graph. `what` names the edit for the undo menu ("Move node", "Add edge").
    /// An edit that leaves the graph as it was records nothing. Edits with the same name that
    /// follow each other without anything in between (a drag, typing in one field) can be
    /// folded into one undo step with [`Draft::edit_folded`].
    pub fn edit(&mut self, what: &'static str, change: impl FnOnce(&mut GraphDef), cx: &mut Context<Self>) {
        self.apply(what, false, change, cx);
    }

    /// Like [`Draft::edit`], but joins the previous undo step when that step has the same name.
    pub fn edit_folded(&mut self, what: &'static str, change: impl FnOnce(&mut GraphDef), cx: &mut Context<Self>) {
        self.apply(what, true, change, cx);
    }

    fn apply(&mut self, what: &'static str, fold: bool, change: impl FnOnce(&mut GraphDef), cx: &mut Context<Self>) {
        let Some(graph) = self.graph.as_mut() else { return };
        let before = graph.clone();
        change(graph);
        if *graph == before {
            return;
        }
        let folded = fold && self.undo.last().is_some_and(|(_, last)| *last == what);
        if !folded {
            self.undo.push((before, what));
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.revision += 1;
        self.fix_selection();
        cx.emit(DraftEvent::Changed);
        cx.notify();
    }

    /// Ends a run of folded edits: the next [`Draft::edit_folded`] starts a new undo step.
    pub fn seal(&mut self) {
        if let Some((_, what)) = self.undo.last_mut() {
            *what = "";
        }
    }

    /// The name of the edit `undo` would take back.
    pub fn can_undo(&self) -> Option<&'static str> {
        self.undo.last().map(|(_, what)| *what)
    }

    /// The name of the edit `redo` would bring back.
    pub fn can_redo(&self) -> Option<&'static str> {
        self.redo.last().map(|(_, what)| *what)
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        let (Some((graph, what)), Some(current)) = (self.undo.pop(), self.graph.as_mut()) else { return };
        self.redo.push((std::mem::replace(current, graph), what));
        self.after_history(cx);
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        let (Some((graph, what)), Some(current)) = (self.redo.pop(), self.graph.as_mut()) else { return };
        self.undo.push((std::mem::replace(current, graph), what));
        self.after_history(cx);
    }

    fn after_history(&mut self, cx: &mut Context<Self>) {
        self.revision += 1;
        self.fix_selection();
        cx.emit(DraftEvent::Changed);
        cx.notify();
    }

    /// The graph differs from what was last loaded or saved.
    pub fn is_dirty(&self) -> bool {
        self.graph != self.saved
    }

    /// The graph as last loaded or saved.
    pub fn saved(&self) -> Option<&GraphDef> {
        self.saved.as_ref()
    }

    /// Records that `graph` (the one handed to the save command) is on disk now.
    pub fn mark_saved(&mut self, graph: GraphDef, cx: &mut Context<Self>) {
        self.saved = Some(graph);
        cx.notify();
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.selection != selection {
            self.selection = selection;
            self.fix_selection();
            cx.emit(DraftEvent::Selected);
            cx.notify();
        }
    }

    /// Drops a selection that points at something the graph no longer has.
    fn fix_selection(&mut self) {
        let gone = match (&self.selection, &self.graph) {
            (Selection::None, _) => false,
            (_, None) => true,
            (Selection::Node(id), Some(graph)) => graph.node(id).is_none(),
            (Selection::Edge(index), Some(graph)) => *index >= graph.edges.len(),
        };
        if gone {
            self.selection = Selection::None;
        }
    }

    /// The problems of the graph as last checked; see [`Draft::problems_are_current`].
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// The problems on show belong to the graph as it is now.
    pub fn problems_are_current(&self) -> bool {
        self.checked_revision == Some(self.revision)
    }

    /// Stores the result of checking the graph at `revision`. An answer for an older graph is
    /// dropped.
    pub fn set_problems(&mut self, revision: u64, problems: Vec<Problem>, cx: &mut Context<Self>) {
        if revision == self.revision {
            self.problems = problems;
            self.checked_revision = Some(revision);
            cx.emit(DraftEvent::Checked);
            cx.notify();
        }
    }

    /// The test run whose progress the canvas and the dock show.
    pub fn run(&self) -> Option<&RunId> {
        self.run.as_ref()
    }

    pub fn show_run(&mut self, run: Option<RunId>, cx: &mut Context<Self>) {
        if self.run != run {
            self.run = run;
            cx.emit(DraftEvent::RunShown);
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use hub::model::{GRAPH_SCHEMA, Limits, NodeDef, NodeKind, Pos, SourceNode};

    use super::*;

    fn graph() -> GraphDef {
        GraphDef {
            schema: GRAPH_SCHEMA,
            id: PipelineId::new("demo"),
            name: "Demo".into(),
            description: String::new(),
            limits: Limits::default(),
            workdirs: Vec::new(),
            nodes: vec![NodeDef {
                id: "src".into(),
                name: "Issues".into(),
                pos: Pos::default(),
                kind: NodeKind::Source(SourceNode::default()),
            }],
            edges: Vec::new(),
        }
    }

    #[gpui::test]
    fn edits_undo_redo_and_the_dirty_mark(cx: &mut TestAppContext) {
        let draft = cx.new(|_| Draft::new(PipelineId::new("demo")));
        draft.update(cx, |draft, cx| {
            draft.load(graph(), cx);
            assert!(!draft.is_dirty());

            draft.edit("Rename", |g| g.name = "Other".into(), cx);
            assert!(draft.is_dirty());
            assert_eq!(draft.can_undo(), Some("Rename"));

            // An edit that changes nothing records nothing.
            draft.edit("Rename", |g| g.name = "Other".into(), cx);
            draft.undo(cx);
            assert_eq!(draft.graph().map(|g| g.name.as_str()), Some("Demo"));
            assert!(!draft.is_dirty());
            assert_eq!(draft.can_redo(), Some("Rename"));

            draft.redo(cx);
            assert_eq!(draft.graph().map(|g| g.name.as_str()), Some("Other"));
        });
    }

    #[gpui::test]
    fn folded_edits_are_one_step_until_sealed(cx: &mut TestAppContext) {
        let draft = cx.new(|_| Draft::new(PipelineId::new("demo")));
        draft.update(cx, |draft, cx| {
            draft.load(graph(), cx);
            for x in [1.0, 2.0, 3.0] {
                draft.edit_folded("Move node", |g| g.nodes[0].pos.x = x, cx);
            }
            draft.seal();
            draft.edit_folded("Move node", |g| g.nodes[0].pos.x = 9.0, cx);

            draft.undo(cx);
            assert_eq!(draft.graph().map(|g| g.nodes[0].pos.x), Some(3.0));
            draft.undo(cx);
            assert_eq!(draft.graph().map(|g| g.nodes[0].pos.x), Some(0.0));
            assert_eq!(draft.can_undo(), None);
        });
    }

    #[gpui::test]
    fn a_selection_follows_the_graph_and_stale_problems_are_dropped(cx: &mut TestAppContext) {
        let draft = cx.new(|_| Draft::new(PipelineId::new("demo")));
        draft.update(cx, |draft, cx| {
            draft.load(graph(), cx);
            draft.select(Selection::Node("src".into()), cx);
            let checked = draft.revision();

            draft.edit("Delete node", |g| g.nodes.clear(), cx);
            assert_eq!(draft.selection(), &Selection::None);

            draft.set_problems(checked, Vec::new(), cx);
            assert!(!draft.problems_are_current(), "an answer for the older graph is dropped");
            draft.set_problems(draft.revision(), Vec::new(), cx);
            assert!(draft.problems_are_current());
        });
    }
}
