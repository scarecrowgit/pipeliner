//! Starter pipelines. Templates contain structure and instructions only — no sample data.

use std::path::PathBuf;

use harness_core::*;

#[derive(Clone, Debug)]
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub graph: PipelineGraph,
    pub color: u32,
    /// Saved agents the graph refers to by `id` (a template-local key). Creating a
    /// pipeline saves them (or reuses saved agents with the same name) and rewrites the
    /// references to the real ids, so they show up on the Agents screen.
    pub agents: Vec<AgentDef>,
}

fn agent_def(
    key: &str,
    name: &str,
    description: &str,
    role: &str,
    model: String,
    skills_on_demand: Vec<String>,
    max_steps: u32,
) -> AgentDef {
    AgentDef {
        id: key.into(),
        name: name.into(),
        description: description.into(),
        role: role.into(),
        model,
        tools: ToolAccess::Full,
        skills: vec![],
        skills_on_demand,
        max_steps,
        version: 1,
        created_at: now(),
        updated_at: now(),
    }
}

/// Rewrites template-local agent keys in `graph` to saved agent ids.
pub fn remap_agents(graph: &mut PipelineGraph, ids: &std::collections::HashMap<String, Id>) {
    for n in &mut graph.nodes {
        if let NodeKind::Agent(a) = &mut n.kind {
            if let Some(id) = a.agent.as_ref().and_then(|k| ids.get(k)) {
                a.agent = Some(id.clone());
            }
            if let Some(d) = &mut a.delegate {
                if let Some(id) = d.agent.as_ref().and_then(|k| ids.get(k)) {
                    d.agent = Some(id.clone());
                }
            }
        }
    }
}

/// The repository this app was built from (for pipelines that improve the app itself).
pub fn self_repo(config: &Config) -> Option<PathBuf> {
    if !config.self_repo.trim().is_empty() {
        return Some(harness_core::config::expand_tilde(&config.self_repo));
    }
    let p = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    p.canonicalize().ok().filter(|p| p.join(".git").exists())
}

/// Picks a configured model whose id contains `hint` (e.g. "opus"), else the default.
fn model(config: &Config, hint: &str) -> String {
    let mut candidates: Vec<String> = config.models.clone();
    candidates.push(config.default_model.clone());
    candidates.push(config.fast_model.clone());
    if let Some(m) = candidates.iter().find(|m| m.contains(hint)) {
        return m.clone();
    }
    // Same provider/prefix as the default, with the family swapped (claude-opus-5-5 -> claude-haiku-5-5).
    for fam in ["opus", "sonnet", "haiku"] {
        if config.default_model.contains(fam) {
            return config.default_model.replace(fam, hint);
        }
    }
    config.default_model.clone()
}

fn node(id: &str, name: &str, kind: NodeKind, x: f32, y: f32) -> Node {
    Node {
        id: id.into(),
        name: name.into(),
        kind,
        x,
        y,
    }
}

fn edge(from: &str, to: &str, port: Option<&str>) -> Edge {
    Edge {
        from: from.into(),
        to: to.into(),
        port: port.map(String::from),
    }
}

const GPUI_SKILLS: [&str; 6] = [
    "gpui-elements",
    "gpui-styling",
    "gpui-entities",
    "gpui-fundamentals",
    "gpui-patterns",
    "gpui-troubleshooting",
];

pub fn all(config: &Config) -> Vec<Template> {
    let repo = self_repo(config);
    let repo_path = repo
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let has_nix = repo
        .as_ref()
        .map(|p| p.join("shell.nix").exists())
        .unwrap_or(false);
    let mut env = std::collections::BTreeMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        // Per run: parallel runs build different code under the same crate names and would
        // overwrite each other's artifacts in a shared target dir.
        "{run_cache}/target".to_string(),
    );
    let app = Workdir {
        id: "app".into(),
        name: "App repo".into(),
        path: repo_path,
        worktree: true,
        base_branch: "main".into(),
        branch_prefix: "pipeliner/".into(),
        setup: String::new(),
        shell: if has_nix {
            "nix-shell shell.nix --run".into()
        } else {
            String::new()
        },
        env,
    };
    let gpui_skills: Vec<String> = GPUI_SKILLS.iter().map(|s| s.to_string()).collect();
    let skills = harness_core::skills::list(config);
    let have = |n: &str| skills.iter().any(|s| s.name == n);
    let design_skills: Vec<String> = ["ui-ux-pro-max"]
        .iter()
        .filter(|s| have(s))
        .map(|s| s.to_string())
        .collect();
    let gpui_skills: Vec<String> = gpui_skills.into_iter().filter(|s| have(s)).collect();

    let design_build = PipelineGraph {
        workdirs: vec![app.clone()],
        nodes: vec![
            node("source", "Issue", NodeKind::Source(SourceNode { input_name: "issue".into(), github_repo: String::new() }), 0.0, 0.0),
            node(
                "triage",
                "Needs design?",
                NodeKind::Decision(DecisionNode {
                    question: "Does this issue need changes to the UI mockups before it is coded? Changes to what the user sees \
                               (new or changed controls, layout, copy, states, screens) need a mockup change; pure logic, data, \
                               performance, build or refactoring work does not. The mockups are static HTML screens in \
                               `design/screens/`; read one only when the issue is ambiguous."
                        .into(),
                    model: model(config, "haiku"),
                    workdir: Some("app".into()),
                    max_steps: 12,
                }),
                0.0,
                1.0,
            ),
            node(
                "design",
                "Designer",
                NodeKind::Agent(AgentNode {
                    agent: Some("designer".into()),
                    model: String::new(),
                    instructions: "You change the UI mockups for one issue. The mockups are static HTML screens in \
                                   `design/screens/` (read `design/README.md` for the format and `design/tokens.css` for the \
                                   design tokens). Edit the HTML in place (edit_file for small changes); keep files static \
                                   (no scripts, no network assets), keep the existing structure, change only what the issue \
                                   needs and stay consistent with the surrounding screens. Do not touch code outside \
                                   `design/`. The user compares renders of the screens you change before anything is coded."
                        .into(),
                    workdir: Some("app".into()),
                    tools: None,
                    skills: vec![],
                    skills_on_demand: vec![],
                    max_steps: 60,
                    variants: 2,
                    variant_hints: vec![
                        "the smallest change that solves the issue, reusing existing components".into(),
                        "a bolder take that rethinks the affected area for clarity".into(),
                    ],
                    delegate: None,
                }),
                -1.0,
                2.0,
            ),
            node(
                "render",
                "Render mockups",
                NodeKind::Render(RenderNode { workdir: Some("app".into()), files: "design/screens/*.html".into(), ..Default::default() }),
                -1.0,
                3.0,
            ),
            node(
                "pick",
                "Pick a design",
                NodeKind::Gate(GateNode { kind: GateKind::Pick, prompt: String::new(), back_to: Some("design".into()) }),
                -1.0,
                4.0,
            ),
            node(
                "build",
                "GPUI build",
                NodeKind::Agent(AgentNode {
                    agent: Some("gpui-lead".into()),
                    model: String::new(),
                    instructions: "You lead coders to implement one issue in this Rust + GPUI codebase. Plan the work, hand \
                                   focused tasks to the coder with `delegate` (exact files and acceptance criteria), review \
                                   what they did (read the code, `git diff`), and send follow-ups until it is right. You may \
                                   make small fixes yourself. Then verify: build, run the tests and clippy as AGENTS.md says, \
                                   and fix until everything passes. Do not commit or push. If an earlier step changed mockups \
                                   in `design/screens/`, they are approved: implement them faithfully. Use `load_skill` for \
                                   GPUI guidance. Finish with a short report for the pull request: cause/goal, what changed \
                                   (files), how you checked it, anything not addressed."
                        .into(),
                    workdir: Some("app".into()),
                    tools: None,
                    skills: vec![],
                    skills_on_demand: vec![],
                    max_steps: 80,
                    variants: 1,
                    variant_hints: vec![],
                    delegate: Some(DelegateConfig {
                        agent: Some("coder".into()),
                        name: String::new(),
                        model: String::new(),
                        instructions: String::new(),
                        skills_on_demand: vec![],
                        max_steps: 80,
                    }),
                }),
                0.0,
                5.0,
            ),
            node("pr", "Open PR", NodeKind::Tool(ToolNode::PullRequest { workdir: Some("app".into()), draft: false }), 0.0, 6.0),
            node(
                "approve",
                "Approve PR",
                NodeKind::Gate(GateNode { kind: GateKind::ApprovePr, prompt: String::new(), back_to: Some("build".into()) }),
                0.0,
                7.0,
            ),
        ],
        edges: vec![
            edge("source", "triage", None),
            edge("triage", "design", Some("yes")),
            edge("triage", "build", Some("no")),
            edge("design", "render", None),
            edge("render", "pick", None),
            edge("pick", "build", None),
            edge("build", "pr", None),
            edge("pr", "approve", None),
        ],
    };

    let design_agents = vec![
        agent_def(
            "designer",
            "Designer",
            "Changes the static HTML mockups in design/screens for one issue",
            "You are a product designer for a desktop app. You work on static HTML mockups, keep them consistent \
             with the existing screens and the design tokens, and change only what the task needs. You explain \
             your choices in a few plain sentences.",
            model(config, "opus"),
            design_skills,
            60,
        ),
        agent_def(
            "gpui-lead",
            "GPUI lead",
            "Plans GPUI work, delegates it to the Coder, reviews and verifies",
            "You are a senior Rust engineer who leads work on a GPUI desktop app. You plan, hand focused tasks to \
             coders, review their changes, and verify with builds, tests and clippy before you report.",
            model(config, "opus"),
            gpui_skills.clone(),
            80,
        ),
        agent_def(
            "coder",
            "Coder",
            "Implements one focused task in a Rust + GPUI codebase",
            "You implement one task given by the orchestrator in a Rust + GPUI codebase. Read the relevant code \
             first, make focused changes in the existing style (edit_file for small edits), and check your work (at \
             least `cargo check` as AGENTS.md says). Do not commit. Use `load_skill` for GPUI guidance. Reply with \
             what you changed and how you checked it.",
            model(config, "sonnet"),
            gpui_skills,
            80,
        ),
    ];

    let agent_pr = PipelineGraph {
        workdirs: vec![Workdir { id: "code".into(), name: "Code".into(), path: String::new(), worktree: true, base_branch: "main".into(), branch_prefix: "pipeliner/".into(), setup: String::new(), shell: String::new(), env: Default::default() }],
        nodes: vec![
            node("source", "Task", NodeKind::Source(SourceNode::default()), 0.0, 0.0),
            node(
                "work",
                "Agent",
                NodeKind::Agent(AgentNode {
                    model: model(config, "sonnet"),
                    instructions: "Implement the task in this repository. Keep changes focused, follow the project's \
                                   conventions, build and test before you finish. Do not commit."
                        .into(),
                    workdir: Some("code".into()),
                    ..Default::default()
                }),
                0.0,
                1.0,
            ),
            node("review", "Review", NodeKind::Gate(GateNode { kind: GateKind::Pick, prompt: String::new(), back_to: Some("work".into()) }), 0.0, 2.0),
            node("pr", "Open PR", NodeKind::Tool(ToolNode::PullRequest { workdir: Some("code".into()), draft: true }), 0.0, 3.0),
        ],
        edges: vec![edge("source", "work", None), edge("work", "review", None), edge("review", "pr", None)],
    };

    let blank = PipelineGraph {
        workdirs: vec![],
        nodes: vec![node(
            "source",
            "Source",
            NodeKind::Source(SourceNode::default()),
            0.0,
            0.0,
        )],
        edges: vec![],
    };

    vec![
        Template {
            id: "design-gpui",
            name: "Design → GPUI build",
            description: "Triage decides whether the mockups change; two designers propose variants you pick from; an \
                          orchestrator with a coder builds it in this app's repo and opens a PR for your approval.",
            graph: design_build,
            color: 0x7170ff,
            agents: design_agents,
        },
        Template {
            id: "agent-pr",
            name: "Agent → PR",
            description: "One agent works in a repository, you review the diff, a pull request is opened.",
            graph: agent_pr,
            color: 0x4cb782,
            agents: vec![],
        },
        Template { id: "blank", name: "Blank", description: "Just a source node. Build your own graph.", graph: blank, color: 0x4ea7fc, agents: vec![] },
    ]
}

pub fn instantiate(t: &Template, name: &str) -> Pipeline {
    Pipeline {
        id: new_id(),
        name: if name.trim().is_empty() {
            t.name.to_string()
        } else {
            name.trim().to_string()
        },
        description: t.description.to_string(),
        color: t.color,
        version: 1,
        draft: None,
        graph: t.graph.clone(),
        archived: false,
        created_at: now(),
        updated_at: now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_are_valid() {
        let c = Config {
            default_model: "nr/cc/claude-opus-5-5".into(),
            ..Config::default()
        };
        for t in all(&c) {
            crate::pipeline::validate(&t.graph).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        }
        let t = &all(&c)[0];
        let NodeKind::Decision(d) = &t.graph.node("triage").unwrap().kind else {
            panic!()
        };
        assert_eq!(d.model, "nr/cc/claude-haiku-5-5");
    }
}
