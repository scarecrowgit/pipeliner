//! The library of things that can take work: agents, orchestrators and pipelines kept as files
//! people and agents edit, the skills installed on the machine, and templates to start from.
//!
//! [`Catalog::load`] reads every location into one cheap snapshot. Writing is done by the plain
//! functions in [`write`] and [`transfer`]; the caller reloads the catalog afterwards.
//!
//! Locations, a later one overriding an earlier one with the same id:
//!
//! 1. built-ins (Build and Plan), in code;
//! 2. Claude Code agents, read-only: `~/.claude/agents`, then `<repo>/.claude/agents`;
//! 3. the user library, `<config>/library/{agents,orchestrators,pipelines}/`;
//! 4. the project, `<repo>/.harness/{agents,orchestrators,pipelines}/`.
//!
//! The id of a file is its stem; references are `agent:<id>`, `orchestrator:<id>` and
//! `pipeline:<id>`. Agents and orchestrators are Markdown with a Claude Code style header (see
//! [`agent_file`]); pipelines are TOML holding a [`model::GraphDef`].

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use model::{AgentDef, GraphDef, PipelineId, Runnable, RunnableRef, Skill};

pub mod agent_file;
mod builtin;
pub mod claude;
mod load;
mod place;
mod self_edit;
mod skills;
mod templates;
pub mod transfer;
pub mod write;

#[cfg(test)]
mod tests;

pub use builtin::{BUILD_ID, PLAN_ID};
pub use place::Place;
pub use self_edit::{SelfEditModels, self_edit_pipeline, self_edit_pipeline_with};
pub use templates::{Instance, Template, TemplateContext, instantiate, templates};

/// A file that could not be read or parsed. The rest of the catalog is unaffected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadProblem {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PipelineEntry {
    pub graph: GraphDef,
    pub text: String,
}

/// A snapshot of everything on disk (and built in) at one moment.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    runnables: Vec<Runnable>,
    index: HashMap<RunnableRef, usize>,
    /// While loading: runnables by reference, so a later location replaces an earlier one.
    by_ref: BTreeMap<RunnableRef, Runnable>,
    pipelines: HashMap<PipelineId, PipelineEntry>,
    skills: Vec<Skill>,
    problems: Vec<LoadProblem>,
}

impl Catalog {
    /// Every agent, orchestrator and pipeline, sorted: orchestrators, pipelines, agents; each
    /// group by name.
    pub fn runnables(&self) -> &[Runnable] {
        &self.runnables
    }

    pub fn get(&self, reference: &RunnableRef) -> Option<&Runnable> {
        self.index.get(reference).map(|&i| &self.runnables[i])
    }

    /// The definition of an agent or orchestrator; `None` for pipelines and unknown references.
    pub fn agent_def(&self, reference: &RunnableRef) -> Option<&AgentDef> {
        self.get(reference)?.agent.as_ref()
    }

    /// A pipeline's graph as it is on disk (the draft; deployed versions live in the store).
    pub fn pipeline(&self, id: &PipelineId) -> Option<&GraphDef> {
        self.pipelines.get(id).map(|p| &p.graph)
    }

    /// The text of a pipeline's file, for the editor's code view.
    pub fn pipeline_text(&self, id: &PipelineId) -> Option<&str> {
        self.pipelines.get(id).map(|p| p.text.as_str())
    }

    /// Installed skills, sorted by name.
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }

    /// What a model gets when it loads the skill: its Markdown body, led by its folder so
    /// relative file references can be resolved. Read from disk on each call.
    pub fn skill_text(&self, name: &str) -> Option<String> {
        skills::skill_text(self.skill(name)?)
    }

    /// Files that did not load, in the order they were read.
    pub fn problems(&self) -> &[LoadProblem] {
        &self.problems
    }

    /// The agent a new chat starts with.
    pub fn default_chat_agent() -> RunnableRef {
        RunnableRef::agent(BUILD_ID)
    }
}
