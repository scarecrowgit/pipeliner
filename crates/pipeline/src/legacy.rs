//! Conversion of the old app's linear pipelines (`~/.config/harness/pipelines/<id>.toml`: named
//! workdirs and an ordered list of steps) into graphs.
//!
//! Each step becomes one node, wired in a line from an issue source and laid out left to right:
//!
//! - `agent` → an agent node with its instructions as the role, its model, tools and skills; with
//!   `delegate` it is an orchestrator with a sub-agent template attached by a spawn edge;
//! - `decision` → a router with one rule, `yes`, that asks the step's question; `else` goes to
//!   the `on_no` step (`end` leaves it unconnected, so the run ends);
//! - `approval` → a review whose Send back restarts the `back_to` step, drawn as a back edge;
//! - `render`, `command`, `merge`, `pull_request`, `report` → the matching action.
//!
//! What graphs cannot say is dropped: `allow_failure` of commands. A jump to a step that does
//! not exist, or back to an earlier one, is an error: graphs only go back from a review.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use base::paths::Paths;
use model::graph::{PORT_APPROVED, PORT_ELSE, PORT_OUT};
use model::{
    ActionNode, AgentNode, AskRule, EdgeDef, EdgeKind, GRAPH_SCHEMA, GraphDef, Limits, ModelRef, NodeDef, NodeId,
    NodeKind, PipelineId, Pos, ReviewNode, RouterNode, RouterRule, SkillPolicy, SourceNode, SpawnPolicy, SubagentNode,
    ToolAccess, ToolGrants, WorkdirDef,
};
use serde::Deserialize;

/// The rule id of a converted decision: the item goes on when the model answers yes.
pub const YES: &str = "yes";

/// Horizontal distance between converted nodes, and where the line starts.
const STEP_X: f32 = 190.0;
const LINE_X: f32 = 20.0;
const LINE_Y: f32 = 170.0;
const BELOW_Y: f32 = 330.0;

/// Converts the text of an old pipeline file. The id comes from the pipeline's name and `~` in
/// paths resolves under `$HOME`; use [`convert_file`] to give the id of the file instead.
pub fn convert(text: &str) -> Result<GraphDef> {
    convert_file(None, text, &Paths::from_env())
}

/// Converts an old pipeline file whose stem is `id` (the old files did not store it).
pub fn convert_file(id: Option<&str>, text: &str, paths: &Paths) -> Result<GraphDef> {
    let old: OldPipeline = toml::from_str(text).context("not an old pipeline file")?;
    let id = id.map(str::to_string).filter(|id| !id.is_empty()).unwrap_or_else(|| base::id::slug(&old.name, 60));
    Converter::new(&old, paths).run(PipelineId::new(id))
}

// ---------------------------------------------------------------- the old format

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldPipeline {
    name: String,
    description: String,
    workdirs: Vec<OldWorkdir>,
    steps: Vec<OldStep>,
}

impl Default for OldPipeline {
    fn default() -> Self {
        Self { name: "New pipeline".into(), description: String::new(), workdirs: Vec::new(), steps: Vec::new() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldWorkdir {
    id: String,
    name: String,
    path: String,
    worktree: bool,
    base_branch: String,
    branch_prefix: String,
    setup: String,
    env: BTreeMap<String, String>,
}

impl Default for OldWorkdir {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            path: String::new(),
            worktree: true,
            base_branch: "main".into(),
            branch_prefix: String::new(),
            setup: String::new(),
            env: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OldKind {
    #[default]
    Agent,
    Decision,
    Render,
    Approval,
    Command,
    Merge,
    PullRequest,
    Report,
}

impl OldKind {
    fn word(self) -> &'static str {
        match self {
            OldKind::Agent => "Agent",
            OldKind::Decision => "Decision",
            OldKind::Render => "Render",
            OldKind::Approval => "Approval",
            OldKind::Command => "Command",
            OldKind::Merge => "Merge",
            OldKind::PullRequest => "Pull request",
            OldKind::Report => "Report",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OldTools {
    ReadOnly,
    ReadAndRun,
    #[default]
    Full,
}

impl OldTools {
    fn grants(self) -> ToolGrants {
        let access = match self {
            OldTools::ReadOnly => ToolAccess::ReadOnly,
            OldTools::ReadAndRun => ToolAccess::ReadAndRun,
            OldTools::Full => ToolAccess::Full,
        };
        ToolGrants { access, ..Default::default() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldDelegate {
    name: String,
    model: String,
    tools: OldTools,
    skills: Vec<String>,
    skills_on_demand: Vec<String>,
    max_steps: u64,
    instructions: String,
}

impl Default for OldDelegate {
    fn default() -> Self {
        Self {
            name: "Coder".into(),
            model: String::new(),
            tools: OldTools::Full,
            skills: Vec::new(),
            skills_on_demand: Vec::new(),
            max_steps: 80,
            instructions: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct OldStep {
    id: String,
    name: String,
    kind: OldKind,
    workdir: String,
    model: String,
    instructions: String,
    tools: OldTools,
    skills: Vec<String>,
    skills_on_demand: Vec<String>,
    max_steps: u64,
    delegate: Option<OldDelegate>,
    question: String,
    on_no: String,
    files: String,
    back_to: String,
    skip_if_nothing_to_review: bool,
    command: String,
    timeout_secs: u64,
    draft: bool,
    push: bool,
}

impl Default for OldStep {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: OldKind::Agent,
            workdir: String::new(),
            model: String::new(),
            instructions: String::new(),
            tools: OldTools::Full,
            skills: Vec::new(),
            skills_on_demand: Vec::new(),
            max_steps: 60,
            delegate: None,
            question: String::new(),
            on_no: String::new(),
            files: "**/*.html".into(),
            back_to: String::new(),
            skip_if_nothing_to_review: true,
            command: String::new(),
            timeout_secs: 600,
            draft: true,
            push: true,
        }
    }
}

// ---------------------------------------------------------------- conversion

struct Converter<'a> {
    old: &'a OldPipeline,
    paths: &'a Paths,
    taken: HashSet<String>,
    nodes: Vec<NodeDef>,
    /// Sub-agent templates of the step being converted, placed after it.
    attached: Vec<NodeDef>,
    edges: Vec<EdgeDef>,
}

fn model(text: &str) -> Option<ModelRef> {
    let text = text.trim();
    (!text.is_empty()).then(|| ModelRef::parse(text))
}

fn steps(max: u64) -> u32 {
    u32::try_from(max).unwrap_or(u32::MAX)
}

fn skills(forced: &[String], auto: &[String]) -> SkillPolicy {
    SkillPolicy { forced: forced.to_vec(), auto: auto.to_vec() }
}

impl<'a> Converter<'a> {
    fn new(old: &'a OldPipeline, paths: &'a Paths) -> Self {
        Self { old, paths, taken: HashSet::new(), nodes: Vec::new(), attached: Vec::new(), edges: Vec::new() }
    }

    /// `base`, or `base-2`, `base-3`… when it is taken.
    fn unique(&mut self, base: &str) -> String {
        let mut id = base.to_string();
        let mut n = 2;
        while self.taken.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.taken.insert(id.clone());
        id
    }

    fn edge(&mut self, from: &str, port: &str, to: &str, kind: EdgeKind) {
        self.edges.push(EdgeDef { from: from.into(), port: port.into(), to: to.into(), kind });
    }

    fn run(mut self, id: PipelineId) -> Result<GraphDef> {
        let old = self.old;
        // Step ids keep their names; the source and sub-agents take what is left.
        let mut ids = Vec::new();
        for step in &old.steps {
            let base = if step.id.trim().is_empty() { base::id::slug(step.kind.word(), 40) } else { step.id.clone() };
            ids.push(self.unique(&base));
        }
        let index = |target: &str| old.steps.iter().position(|s| s.id == target);
        let source = self.unique("source");
        self.nodes.push(NodeDef {
            id: source.as_str().into(),
            name: "Issues".into(),
            pos: Pos { x: LINE_X, y: LINE_Y },
            kind: NodeKind::Source(SourceNode::default()),
        });
        // The port the previous node leaves through towards the next step.
        let mut prev: Option<(String, &str)> = Some((source, PORT_OUT));
        for (i, step) in old.steps.iter().enumerate() {
            let id = ids[i].clone();
            let x = LINE_X + STEP_X * (i + 1) as f32;
            let name = if step.name.trim().is_empty() { step.kind.word().to_string() } else { step.name.clone() };
            if let Some((from, port)) = prev.take() {
                self.edge(&from, port, &id, EdgeKind::Route);
            }
            let kind = match step.kind {
                OldKind::Agent => self.agent(step, &id, x),
                OldKind::Decision => {
                    match step.on_no.trim() {
                        "" => {}
                        "end" => {}
                        target => {
                            let Some(to) = index(target) else {
                                bail!("Step {name} jumps to a missing step “{target}”");
                            };
                            if to <= i {
                                bail!("Step {name} jumps back to {target}; only an approval can go back");
                            }
                            let to = ids[to].clone();
                            self.edge(&id, PORT_ELSE, &to, EdgeKind::Route);
                        }
                    }
                    if step.on_no.trim().is_empty() && i + 1 < ids.len() {
                        let next = ids[i + 1].clone();
                        self.edge(&id, PORT_ELSE, &next, EdgeKind::Route);
                    }
                    NodeKind::Router(RouterNode {
                        rules: vec![RouterRule {
                            id: YES.into(),
                            label: "Yes".into(),
                            when: String::new(),
                            ask: Some(AskRule {
                                question: step.question.clone(),
                                model: model(&step.model).unwrap_or_default(),
                                workdir: step.workdir.clone(),
                                max_steps: steps(step.max_steps),
                            }),
                        }],
                    })
                }
                OldKind::Approval => {
                    let back = step.back_to.trim();
                    let send_back_to = if back.is_empty() {
                        None
                    } else {
                        let Some(to) = index(back) else {
                            bail!("Step {name} sends back to a missing step “{back}”");
                        };
                        if to >= i {
                            bail!("Step {name} sends back to {back}, which is not before it");
                        }
                        let to = ids[to].clone();
                        self.edge(&id, PORT_OUT, &to, EdgeKind::Back);
                        Some(NodeId::new(to))
                    };
                    NodeKind::Review(ReviewNode { send_back_to, skip_if_empty: step.skip_if_nothing_to_review })
                }
                OldKind::Render => {
                    NodeKind::Action(ActionNode::Render { workdir: step.workdir.clone(), files: step.files.clone() })
                }
                OldKind::Command => NodeKind::Action(ActionNode::Command {
                    workdir: step.workdir.clone(),
                    command: step.command.clone(),
                    tests: false,
                    parse: String::new(),
                    timeout_secs: step.timeout_secs,
                }),
                OldKind::Merge => NodeKind::Action(ActionNode::Merge { workdir: step.workdir.clone() }),
                OldKind::PullRequest => NodeKind::Action(ActionNode::PullRequest {
                    workdir: step.workdir.clone(),
                    draft: step.draft,
                    push: step.push,
                }),
                OldKind::Report => NodeKind::Action(ActionNode::Report),
            };
            prev = Some((
                id.clone(),
                match step.kind {
                    OldKind::Decision => YES,
                    OldKind::Approval => PORT_APPROVED,
                    _ => PORT_OUT,
                },
            ));
            self.nodes.push(NodeDef { id: id.into(), name, pos: Pos { x, y: LINE_Y }, kind });
            self.nodes.append(&mut self.attached);
        }
        let workdirs = old.workdirs.iter().map(|w| self.workdir(w)).collect();
        Ok(GraphDef {
            schema: GRAPH_SCHEMA,
            id,
            name: old.name.clone(),
            description: old.description.clone(),
            limits: Limits::default(),
            workdirs,
            nodes: self.nodes,
            edges: self.edges,
        })
    }

    /// An agent step; with a delegate it becomes an orchestrator and its template is added
    /// below it.
    fn agent(&mut self, step: &OldStep, id: &str, x: f32) -> NodeKind {
        let has_skills = !step.skills.is_empty() || !step.skills_on_demand.is_empty();
        let mut node = AgentNode {
            runnable: None,
            role: step.instructions.clone(),
            model: model(&step.model),
            workdir: step.workdir.clone(),
            variants: 1,
            tools: Some(step.tools.grants()),
            skills: has_skills.then(|| skills(&step.skills, &step.skills_on_demand)),
            spawn: None,
            budget_usd: None,
            max_steps: (step.max_steps > 0).then(|| steps(step.max_steps)),
            files_out: Vec::new(),
        };
        let Some(delegate) = &step.delegate else { return NodeKind::Agent(node) };
        node.spawn = Some(SpawnPolicy { enabled: true, ..Default::default() });
        let sub_name = if delegate.name.trim().is_empty() { "Coder".to_string() } else { delegate.name.clone() };
        let sub_id = self.unique(&format!("{id}-{}", base::id::slug(&sub_name, 40)));
        self.attached.push(NodeDef {
            id: sub_id.as_str().into(),
            name: sub_name,
            pos: Pos { x: x + STEP_X / 2.0, y: BELOW_Y },
            kind: NodeKind::Subagent(SubagentNode {
                role: delegate.instructions.clone(),
                model: model(&delegate.model).unwrap_or_default(),
                tools: delegate.tools.grants(),
                skills: skills(&delegate.skills, &delegate.skills_on_demand),
                max_steps: steps(delegate.max_steps),
                per_run: SpawnPolicy::default().per_run,
            }),
        });
        self.edge(id, PORT_OUT, &sub_id, EdgeKind::Spawn);
        NodeKind::Orchestrator(node)
    }

    fn workdir(&self, old: &OldWorkdir) -> WorkdirDef {
        let path = old.path.trim();
        WorkdirDef {
            id: old.id.clone(),
            name: old.name.clone(),
            path: if path.is_empty() { PathBuf::new() } else { self.paths.expand(path) },
            worktree: old.worktree,
            base_branch: old.base_branch.clone(),
            branch_prefix: old.branch_prefix.clone(),
            setup: old.setup.clone(),
            env: old.env.clone(),
        }
    }
}
