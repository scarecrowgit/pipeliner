//! The editor's Problems list for a graph. Errors block a deploy; warnings do not.
//!
//! [`validate`] checks the structure (one available source, unique ids, edges between existing
//! nodes on ports their node has), the flow (reachability, loops made of routes, back edges and
//! Send back targets, variant scopes closed by a review, dead ends) and each node's own fields
//! (router rules, agents, actions, workdirs, limits). What exists outside the graph (library
//! agents, models, pipelines) is asked of a [`Library`].

mod flow;
mod nodes;
mod structure;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use model::{GraphDef, ModelRef, NodeDef, NodeId, NodeKind, PipelineId, Problem, RunnableRef, Severity};

/// The most variants one agent node may fan out into.
pub const MAX_VARIANTS: u32 = 8;

/// What exists outside the graph, as far as validation needs to know.
pub trait Library {
    /// Whether an agent or orchestrator with this reference is in the library.
    fn has_runnable(&self, reference: &RunnableRef) -> bool;
    /// Whether this model can be called through a configured provider.
    fn has_model(&self, model: &ModelRef) -> bool;
    /// Whether a pipeline with this id exists.
    fn has_pipeline(&self, id: &PipelineId) -> bool;
}

/// A library where everything exists; for tests and for checking a graph's shape alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct AnyLibrary;

impl Library for AnyLibrary {
    fn has_runnable(&self, _: &RunnableRef) -> bool {
        true
    }

    fn has_model(&self, _: &ModelRef) -> bool {
        true
    }

    fn has_pipeline(&self, _: &PipelineId) -> bool {
        true
    }
}

/// The catalog knows agents and pipelines, not providers: every model passes.
impl Library for catalog::Catalog {
    fn has_runnable(&self, reference: &RunnableRef) -> bool {
        self.get(reference).is_some()
    }

    fn has_model(&self, _: &ModelRef) -> bool {
        true
    }

    fn has_pipeline(&self, id: &PipelineId) -> bool {
        self.pipeline(id).is_some()
    }
}

/// Every problem of `graph`: errors first, then by the order of the nodes in the file;
/// problems of the graph as a whole come before those of its nodes.
pub fn validate(graph: &GraphDef, library: &dyn Library) -> Vec<Problem> {
    let mut ctx = Ctx::new(graph, library);
    structure::check(&mut ctx);
    let routes = structure::edges(&mut ctx);
    flow::check(&mut ctx, &routes);
    nodes::check(&mut ctx, &routes);
    ctx.finish()
}

/// Whether `graph` has no errors (warnings allowed): it may be deployed.
pub fn deployable(problems: &[Problem]) -> bool {
    problems.iter().all(|p| p.severity != Severity::Error)
}

struct Entry {
    severity: Severity,
    node: Option<usize>,
    message: String,
}

/// The graph under check, the index of its nodes, and the problems found so far.
pub(crate) struct Ctx<'a> {
    pub graph: &'a GraphDef,
    pub library: &'a dyn Library,
    /// Node id → index of its first node in the file.
    index: HashMap<&'a NodeId, usize>,
    found: Vec<Entry>,
}

impl<'a> Ctx<'a> {
    fn new(graph: &'a GraphDef, library: &'a dyn Library) -> Self {
        let mut index = HashMap::new();
        for (i, node) in graph.nodes.iter().enumerate() {
            index.entry(&node.id).or_insert(i);
        }
        Self { graph, library, index, found: Vec::new() }
    }

    pub fn find(&self, id: &NodeId) -> Option<usize> {
        self.index.get(id).copied()
    }

    pub fn node(&self, i: usize) -> &'a NodeDef {
        &self.graph.nodes[i]
    }

    pub fn kind(&self, i: usize) -> &'a NodeKind {
        &self.graph.nodes[i].kind
    }

    /// The node's name for a message, or its id when it has none.
    pub fn name(&self, i: usize) -> &'a str {
        let node = &self.graph.nodes[i];
        if node.name.trim().is_empty() { node.id.as_str() } else { node.name.as_str() }
    }

    pub fn error(&mut self, node: Option<usize>, message: impl Into<String>) {
        self.found.push(Entry { severity: Severity::Error, node, message: message.into() });
    }

    pub fn warn(&mut self, node: Option<usize>, message: impl Into<String>) {
        self.found.push(Entry { severity: Severity::Warning, node, message: message.into() });
    }

    fn finish(mut self) -> Vec<Problem> {
        let rank = |s: Severity| match s {
            Severity::Error => 0,
            Severity::Warning => 1,
        };
        self.found.sort_by_key(|e| (rank(e.severity), e.node.map_or(0, |i| i + 1)));
        self.found
            .into_iter()
            .map(|e| Problem {
                severity: e.severity,
                node: e.node.map(|i| self.graph.nodes[i].id.clone()),
                message: e.message,
            })
            .collect()
    }
}

/// The `route` edges that passed the structure checks, by node index.
pub(crate) struct Routes {
    pub succ: Vec<Vec<usize>>,
    pub pred: Vec<Vec<usize>>,
    /// Out-ports of each node that have at least one valid route edge.
    pub ports: Vec<Vec<String>>,
    /// Valid `back` edges: (review, target).
    pub back: Vec<(usize, usize)>,
    /// Nodes with a valid `spawn` edge into them, or a valid `context` edge out of them.
    pub attached: Vec<bool>,
    /// Nodes with a valid `spawn` edge out of them.
    pub spawns: Vec<bool>,
}
