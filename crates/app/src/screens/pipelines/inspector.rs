//! Node inspector: edits every field of the selected node's kind, its wiring, and deletes it.
//!
//! Text inputs and selects are gpui-component entities; they are built once per selected node
//! (and rebuilt when the structure changes, `Editor::rev`). Edits write into the local graph
//! and schedule the draft save.

use std::collections::HashMap;

use gpui::{Animation, AnimationExt as _};
use gpui_component::input::{Input, InputState};
use gpui_component::select::{Select, SelectState};
use harness_core::{
    AgentDef, Config, DelegateConfig, GateKind, Node, NodeKind, PipelineGraph, RouterRule, RuleOp,
    ToolAccess, ToolNode,
};

use super::edit;
use super::graph::kind_tile;
use super::PipelinesScreen;
use crate::screens::prelude::*;
use crate::ui::input::InputEvent;
use crate::ui::select::{Options, SelectEvent, SelectOption};

/// An editable field of the selected node.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum F {
    Name,
    Agent,
    Model,
    Instructions,
    Workdir,
    Tools,
    MaxSteps,
    Hint(usize),
    DelName,
    DelModel,
    DelInstructions,
    DelMaxSteps,
    Question,
    RuleField(usize),
    RuleOp(usize),
    RuleValue(usize),
    GatePrompt,
    BackTo,
    Files,
    Width,
    Height,
    RenderCommand,
    Command,
    InputName,
    Repo,
    Connect(Option<String>),
}

pub(super) struct Inspector {
    node: Id,
    rev: u64,
    inputs: HashMap<F, Entity<InputState>>,
    selects: HashMap<F, Entity<SelectState<Options>>>,
    _subs: Vec<Subscription>,
}

const RULE_OPS: [RuleOp; 4] = [
    RuleOp::Contains,
    RuleOp::Equals,
    RuleOp::StartsWith,
    RuleOp::Matches,
];
const MERGE_METHODS: [&str; 3] = ["squash", "merge", "rebase"];

fn op_key(op: RuleOp) -> &'static str {
    match op {
        RuleOp::Contains => "contains",
        RuleOp::Equals => "equals",
        RuleOp::StartsWith => "starts_with",
        RuleOp::Matches => "matches",
    }
}

fn access_key(t: ToolAccess) -> &'static str {
    match t {
        ToolAccess::None => "none",
        ToolAccess::ReadOnly => "read_only",
        ToolAccess::ReadAndRun => "read_and_run",
        ToolAccess::Full => "full",
    }
}

fn opt(v: String) -> Option<String> {
    (!v.trim().is_empty()).then_some(v)
}

/// Writes a text/select value into the node. Returns true when the inspector must rebuild.
fn apply_field(node: &mut Node, f: &F, v: String) {
    let num = |v: &str| v.trim().parse::<u32>().ok();
    match (&mut node.kind, f) {
        (_, F::Name) => node.name = v,
        (NodeKind::Agent(a), F::Agent) => a.agent = opt(v),
        (NodeKind::Agent(a), F::Model) => a.model = v,
        (NodeKind::Decision(d), F::Model) => d.model = v,
        (NodeKind::Agent(a), F::Instructions) => a.instructions = v,
        (NodeKind::Agent(a), F::Workdir) => a.workdir = opt(v),
        (NodeKind::Decision(d), F::Workdir) => d.workdir = opt(v),
        (NodeKind::Render(r), F::Workdir) => r.workdir = opt(v),
        (NodeKind::Tool(ToolNode::Command { workdir, .. }), F::Workdir)
        | (NodeKind::Tool(ToolNode::PullRequest { workdir, .. }), F::Workdir) => *workdir = opt(v),
        (NodeKind::Agent(a), F::Tools) => {
            a.tools = ToolAccess::ALL.into_iter().find(|t| access_key(*t) == v)
        }
        (NodeKind::Agent(a), F::MaxSteps) => {
            if let Some(n) = num(&v).filter(|n| *n > 0) {
                a.max_steps = n;
            }
        }
        (NodeKind::Decision(d), F::MaxSteps) => {
            if let Some(n) = num(&v).filter(|n| *n > 0) {
                d.max_steps = n;
            }
        }
        (NodeKind::Agent(a), F::Hint(i)) => {
            if a.variant_hints.len() <= *i {
                a.variant_hints.resize(*i + 1, String::new());
            }
            a.variant_hints[*i] = v;
            while a.variant_hints.last().is_some_and(|h| h.is_empty()) {
                a.variant_hints.pop();
            }
        }
        (NodeKind::Agent(a), F::DelName) => {
            if let Some(d) = a.delegate.as_mut() {
                d.name = v;
            }
        }
        (NodeKind::Agent(a), F::DelModel) => {
            if let Some(d) = a.delegate.as_mut() {
                d.model = v;
            }
        }
        (NodeKind::Agent(a), F::DelInstructions) => {
            if let Some(d) = a.delegate.as_mut() {
                d.instructions = v;
            }
        }
        (NodeKind::Agent(a), F::DelMaxSteps) => {
            if let (Some(d), Some(n)) = (a.delegate.as_mut(), num(&v).filter(|n| *n > 0)) {
                d.max_steps = n;
            }
        }
        (NodeKind::Decision(d), F::Question) => d.question = v,
        (NodeKind::Router(r), F::RuleField(i)) => {
            if let Some(rule) = r.rules.get_mut(*i) {
                rule.field = v;
            }
        }
        (NodeKind::Router(r), F::RuleOp(i)) => {
            if let (Some(rule), Some(op)) = (
                r.rules.get_mut(*i),
                RULE_OPS.into_iter().find(|o| op_key(*o) == v),
            ) {
                rule.op = op;
            }
        }
        (NodeKind::Router(r), F::RuleValue(i)) => {
            if let Some(rule) = r.rules.get_mut(*i) {
                rule.value = v;
            }
        }
        (NodeKind::Gate(g), F::GatePrompt) => g.prompt = v,
        (NodeKind::Gate(g), F::BackTo) => g.back_to = opt(v),
        (NodeKind::Render(r), F::Files) => r.files = v,
        (NodeKind::Render(r), F::Width) => {
            if let Some(n) = num(&v).filter(|n| *n > 0) {
                r.width = n;
            }
        }
        (NodeKind::Render(r), F::Height) => {
            if let Some(n) = num(&v).filter(|n| *n > 0) {
                r.height = n;
            }
        }
        (NodeKind::Render(r), F::RenderCommand) => r.command = v,
        (NodeKind::Tool(ToolNode::Command { command, .. }), F::Command) => *command = v,
        (NodeKind::Source(s), F::InputName) => s.input_name = v,
        (NodeKind::Source(s), F::Repo) => s.github_repo = v.trim().to_string(),
        _ => {}
    }
}

/// Collects the entities while building an [`Inspector`].
struct Builder<'a> {
    window: &'a mut Window,
    inputs: HashMap<F, Entity<InputState>>,
    selects: HashMap<F, Entity<SelectState<Options>>>,
    subs: Vec<Subscription>,
}

impl Builder<'_> {
    /// A text input; `rows` makes it multi-line (grows up to 14 rows).
    fn input(
        &mut self,
        f: F,
        value: &str,
        placeholder: &str,
        rows: Option<usize>,
        cx: &mut Context<PipelinesScreen>,
    ) {
        let (value, placeholder) = (value.to_string(), placeholder.to_string());
        let window = &mut *self.window;
        let state = cx.new(|cx| {
            let s = InputState::new(window, cx);
            let s = match rows {
                Some(r) => s.auto_grow(r, 14),
                None => s,
            };
            s.placeholder(placeholder).default_value(value)
        });
        let key = f.clone();
        self.subs.push(
            cx.subscribe_in(&state, window, move |this, st, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    let v = st.read(cx).value().to_string();
                    this.on_field(&key, v, cx);
                }
            }),
        );
        self.inputs.insert(f, state);
    }

    fn select(
        &mut self,
        f: F,
        options: Vec<SelectOption>,
        selected: &str,
        cx: &mut Context<PipelinesScreen>,
    ) {
        let searchable = options.len() > 8;
        let state = ui::select::select_state(options, Some(selected), searchable, self.window, cx);
        let key = f.clone();
        self.subs.push(cx.subscribe_in(
            &state,
            self.window,
            move |this, _, ev: &SelectEvent<Options>, _, cx| {
                if let SelectEvent::Confirm(Some(v)) = ev {
                    this.on_field(&key, v.clone(), cx);
                }
            },
        ));
        self.selects.insert(f, state);
    }
}

/// Model options: `none_label` for the empty value, then the catalog, plus `current` if missing.
fn model_options(models: &[String], current: &str, none_label: String) -> Vec<SelectOption> {
    let mut v = vec![SelectOption::new("", none_label)];
    let mut all: Vec<String> = models.to_vec();
    if !current.is_empty() && !all.iter().any(|m| m == current) {
        all.push(current.to_string());
    }
    v.extend(
        all.into_iter()
            .map(|m| SelectOption::new(m.clone(), format!("{} · {m}", Config::short_model(&m)))),
    );
    v
}

fn workdir_options(g: &PipelineGraph, none_label: &str) -> Vec<SelectOption> {
    let mut v = vec![SelectOption::new("", none_label.to_string())];
    v.extend(
        g.workdirs
            .iter()
            .map(|w| SelectOption::new(w.id.clone(), format!("{} · {}", w.name, w.id))),
    );
    v
}

impl PipelinesScreen {
    /// Builds (or rebuilds) the inspector entities for the selected node when needed.
    pub(super) fn ensure_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_ref() else { return };
        let Some(node) = ed
            .selected
            .as_deref()
            .and_then(|s| ed.graph.node(s))
            .cloned()
        else {
            if let Some(ed) = self.ed.as_mut() {
                ed.inspector = None;
            }
            return;
        };
        if ed
            .inspector
            .as_ref()
            .is_some_and(|i| i.node == node.id && i.rev == ed.rev)
        {
            return;
        }
        let g = ed.graph.clone();
        let rev = ed.rev;
        let agents: Vec<AgentDef> = self
            .detail
            .as_ref()
            .map(|d| d.agents.clone())
            .unwrap_or_default();
        let models = self.catalog.models.clone();
        let default_model = self.catalog.default_model.clone();
        let mut b = Builder {
            window,
            inputs: HashMap::new(),
            selects: HashMap::new(),
            subs: Vec::new(),
        };

        b.input(F::Name, &node.name, "Node name", None, cx);
        match &node.kind {
            NodeKind::Source(s) => {
                b.input(F::InputName, &s.input_name, "issue", None, cx);
                b.input(F::Repo, &s.github_repo, "owner/name", None, cx);
            }
            NodeKind::Decision(d) => {
                b.input(
                    F::Question,
                    &d.question,
                    "A yes/no question about the input",
                    Some(3),
                    cx,
                );
                b.select(
                    F::Model,
                    model_options(&models, &d.model, "Choose a model".into()),
                    &d.model,
                    cx,
                );
                b.select(
                    F::Workdir,
                    workdir_options(&g, "No workdir (answer from the input)"),
                    d.workdir.as_deref().unwrap_or(""),
                    cx,
                );
                b.input(F::MaxSteps, &d.max_steps.to_string(), "12", None, cx);
            }
            NodeKind::Router(r) => {
                let mut fields = vec![
                    SelectOption::new("input", "input"),
                    SelectOption::new("title", "title"),
                ];
                for up in edit::upstream(&g, &node.id) {
                    let name = g.node(&up).map(|n| n.name.clone()).unwrap_or_default();
                    fields.push(SelectOption::new(
                        format!("out.{up}"),
                        format!("out.{up} · {name}"),
                    ));
                }
                for (i, rule) in r.rules.iter().enumerate() {
                    let mut opts = fields.clone();
                    if !opts.iter().any(|o| o.value == rule.field) {
                        opts.push(SelectOption::plain(rule.field.clone()));
                    }
                    b.select(F::RuleField(i), opts, &rule.field, cx);
                    let ops = RULE_OPS
                        .iter()
                        .map(|o| SelectOption::new(op_key(*o), super::graph::rule_op_label(*o)))
                        .collect();
                    b.select(F::RuleOp(i), ops, op_key(rule.op), cx);
                    b.input(F::RuleValue(i), &rule.value, "value", None, cx);
                }
            }
            NodeKind::Agent(a) => {
                let mut opts = vec![SelectOption::new("", "None · configure the step here")];
                opts.extend(agents.iter().map(|d| {
                    SelectOption::new(
                        d.id.clone(),
                        format!(
                            "{} · {} · v{}",
                            d.name,
                            Config::short_model(&d.model),
                            d.version
                        ),
                    )
                }));
                b.select(F::Agent, opts, a.agent.as_deref().unwrap_or(""), cx);
                let bound = a
                    .agent
                    .as_deref()
                    .and_then(|id| agents.iter().find(|d| d.id == id));
                let none_label = match bound {
                    Some(d) => format!("{}'s model · {}", d.name, Config::short_model(&d.model)),
                    None => format!("Default · {}", Config::short_model(&default_model)),
                };
                b.select(
                    F::Model,
                    model_options(&models, &a.model, none_label),
                    &a.model,
                    cx,
                );
                b.input(
                    F::Instructions,
                    &a.instructions,
                    "What this step must do",
                    Some(4),
                    cx,
                );
                b.select(
                    F::Workdir,
                    workdir_options(&g, "No workdir"),
                    a.workdir.as_deref().unwrap_or(""),
                    cx,
                );
                let mut tools = vec![SelectOption::new(
                    "",
                    match bound {
                        Some(d) => format!("{}'s tools · {}", d.name, d.tools.label()),
                        None => format!("Default · {}", ToolAccess::default().label()),
                    },
                )];
                tools.extend(
                    ToolAccess::ALL
                        .iter()
                        .map(|t| SelectOption::new(access_key(*t), t.label())),
                );
                b.select(F::Tools, tools, a.tools.map(access_key).unwrap_or(""), cx);
                b.input(F::MaxSteps, &a.max_steps.to_string(), "40", None, cx);
                for i in 0..a.variants.max(1) as usize {
                    if a.variants > 1 {
                        let hint = a.variant_hints.get(i).cloned().unwrap_or_default();
                        b.input(
                            F::Hint(i),
                            &hint,
                            "Strategy hint for this variant (optional)",
                            None,
                            cx,
                        );
                    }
                }
                if let Some(d) = &a.delegate {
                    b.input(F::DelName, &d.name, "Sub-agent name", None, cx);
                    b.select(
                        F::DelModel,
                        model_options(&models, &d.model, "Choose a model".into()),
                        &d.model,
                        cx,
                    );
                    b.input(
                        F::DelInstructions,
                        &d.instructions,
                        "How the sub-agent works",
                        Some(3),
                        cx,
                    );
                    b.input(F::DelMaxSteps, &d.max_steps.to_string(), "40", None, cx);
                }
            }
            NodeKind::Render(r) => {
                b.select(
                    F::Workdir,
                    workdir_options(&g, "Choose a workdir"),
                    r.workdir.as_deref().unwrap_or(""),
                    cx,
                );
                b.input(F::Files, &r.files, "**/*.html", None, cx);
                b.input(F::Width, &r.width.to_string(), "1440", None, cx);
                b.input(F::Height, &r.height.to_string(), "1600", None, cx);
                b.input(
                    F::RenderCommand,
                    &r.command,
                    "Built-in chromium (or a command with {in} {out})",
                    None,
                    cx,
                );
            }
            NodeKind::Gate(gate) => {
                b.input(
                    F::GatePrompt,
                    &gate.prompt,
                    "Title of the review item",
                    None,
                    cx,
                );
                let mut opts = vec![SelectOption::new("", "Nowhere · send-back is off")];
                for up in edit::upstream(&g, &node.id) {
                    if let Some(n) = g.node(&up).filter(|n| matches!(n.kind, NodeKind::Agent(_))) {
                        opts.push(SelectOption::new(n.id.clone(), n.name.clone()));
                    }
                }
                b.select(F::BackTo, opts, gate.back_to.as_deref().unwrap_or(""), cx);
            }
            NodeKind::Tool(t) => match t {
                ToolNode::Command {
                    workdir, command, ..
                } => {
                    b.select(
                        F::Workdir,
                        workdir_options(&g, "No workdir"),
                        workdir.as_deref().unwrap_or(""),
                        cx,
                    );
                    b.input(F::Command, command, "cargo test", Some(2), cx);
                }
                ToolNode::PullRequest { workdir, .. } => {
                    b.select(
                        F::Workdir,
                        workdir_options(&g, "Choose a workdir"),
                        workdir.as_deref().unwrap_or(""),
                        cx,
                    );
                }
                ToolNode::MergePr { .. } => {}
            },
        }
        // Connections: one "connect to" select per output port.
        for port in node.kind.ports() {
            let mut opts = vec![SelectOption::new("", "Connect to…")];
            opts.extend(
                g.nodes
                    .iter()
                    .filter(|n| n.id != node.id && !matches!(n.kind, NodeKind::Source(_)))
                    .filter(|n| {
                        !g.edges
                            .iter()
                            .any(|e| e.from == node.id && e.to == n.id && e.port == port)
                    })
                    .map(|n| {
                        SelectOption::new(n.id.clone(), format!("{} · {}", n.name, n.kind.label()))
                    }),
            );
            b.select(F::Connect(port), opts, "", cx);
        }

        let Builder {
            inputs,
            selects,
            subs,
            ..
        } = b;
        if let Some(ed) = self.ed.as_mut() {
            ed.inspector = Some(Inspector {
                node: node.id.clone(),
                rev,
                inputs,
                selects,
                _subs: subs,
            });
        }
    }

    /// A text or select field changed.
    fn on_field(&mut self, f: &F, v: String, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(id) = ed.selected.clone() else {
            return;
        };
        if let F::Connect(port) = f {
            if v.is_empty() || !edit::connect(&mut ed.graph, &id, port.clone(), &v) {
                return;
            }
            ed.rev += 1;
        } else {
            let Some(node) = ed.graph.node_mut(&id) else {
                return;
            };
            let before = node.clone();
            apply_field(node, f, v);
            if *node == before {
                return;
            }
            // Picking a saved agent renames a still-default step after it.
            if let (F::Agent, NodeKind::Agent(a)) = (f, &node.kind) {
                let agent_name = a
                    .agent
                    .as_deref()
                    .and_then(|aid| self.detail.as_ref()?.agents.iter().find(|d| d.id == aid))
                    .map(|d| d.name.clone());
                if let Some(name) = agent_name.filter(|_| before.name.starts_with("Agent step")) {
                    node.name = name;
                }
                ed.rev += 1;
            }
        }
        self.touch(cx);
    }

    /// Applies a click edit to the selected node; `rebuild` refreshes the inspector entities.
    fn edit_node(&mut self, rebuild: bool, cx: &mut Context<Self>, f: impl FnOnce(&mut Node)) {
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(node) = ed.selected.clone().and_then(|id| ed.graph.node_mut(&id)) else {
            return;
        };
        f(node);
        if rebuild {
            ed.rev += 1;
        }
        self.touch(cx);
    }

    /// Applies a click edit to the whole graph (rules, wires); always rebuilds.
    fn edit_graph(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut PipelineGraph, &str)) {
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(id) = ed.selected.clone() else {
            return;
        };
        f(&mut ed.graph, &id);
        ed.rev += 1;
        self.touch(cx);
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        let Some(id) = ed.selected.take() else { return };
        edit::remove_node(&mut ed.graph, &id);
        ed.sel_seq += 1;
        ed.rev += 1;
        self.touch(cx);
    }

    pub(super) fn render_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.ed.as_ref()?;
        let insp = ed.inspector.as_ref()?;
        let node = ed.graph.node(&insp.node)?;
        let d = self.detail.as_ref()?;
        let g = &ed.graph;
        let input = |f: F| {
            insp.inputs
                .get(&f)
                .map(|s| field_input(s).into_any_element())
        };
        let area = |f: F| {
            insp.inputs
                .get(&f)
                .map(|s| area_input(s).into_any_element())
        };
        let select = |f: F| {
            insp.selects
                .get(&f)
                .map(|s| field_select(s).into_any_element())
        };

        let (icon, bg, fg) = kind_tile(&node.kind);
        let change = (ed.graph != d.pipeline.graph)
            .then(|| {
                edit::node_changes(&d.pipeline.graph, g)
                    .get(&node.id)
                    .copied()
            })
            .flatten();
        let pos = g.nodes.iter().position(|n| n.id == node.id).unwrap_or(0) + 1;
        let mut sub = vec![node.kind.label().to_string()];
        if let NodeKind::Agent(a) = &node.kind {
            if a.variants > 1 {
                sub.push(format!("fan-out ×{}", a.variants));
            }
        }
        sub.push(format!("node {pos} of {}", g.nodes.len()));
        sub.push(node.id.clone());
        let next_version = d.pipeline.version + 1;

        let mut col = div().flex().flex_col().child(
            div()
                .flex()
                .flex_col()
                .gap(theme::S1)
                .px(theme::S5)
                .py(theme::S4)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .child(icon_tile(icon, bg, fg))
                        .child(
                            div()
                                .min_w_0()
                                .ellipsis()
                                .text_heading()
                                .fg()
                                .child(node.name.clone()),
                        )
                        .when_some(change, |el, c| {
                            el.child(
                                Badge::new(match c {
                                    edit::NodeChange::Added => format!("new in v{next_version}"),
                                    edit::NodeChange::Edited => {
                                        format!("edited in v{next_version}")
                                    }
                                })
                                .accent(),
                            )
                        })
                        .child(div().flex_1())
                        .child(
                            IconButton::new("pl-insp-close", AppIcon::X)
                                .small()
                                .tooltip("Close inspector")
                                .on_click(cx.listener(|this, _, _, cx| this.select_node(None, cx))),
                        ),
                )
                .child(div().text_label().fg_muted().child(sub.join(" · "))),
        );
        col = col.child(sec("Name", None).children(input(F::Name)));

        match &node.kind {
            NodeKind::Source(_) => {
                col = col
                    .child(
                        sec("Input name", Some("what the input is called in prompts"))
                            .children(input(F::InputName)),
                    )
                    .child(
                        sec("GitHub repo", Some("optional · runs read issues with gh"))
                            .children(input(F::Repo))
                            .child(hint(
                                "owner/name. Start a run with an issue number or URL as its input.",
                            )),
                    );
            }
            NodeKind::Decision(dn) => {
                col = col
                    .child(sec("Question", Some("answered yes or no")).children(area(F::Question)))
                    .child(sec("Model", None).children(select(F::Model)))
                    .child(
                        sec("Workdir", Some("read-only, to look before deciding"))
                            .children(select(F::Workdir)),
                    )
                    .child(sec("Max steps", None).children(input(F::MaxSteps)))
                    .when(dn.model.is_empty(), |el| {
                        el.child(
                            div()
                                .px(theme::S5)
                                .child(warn("Pick a model, or the run fails here.")),
                        )
                    });
            }
            NodeKind::Router(r) => {
                let mut rules = sec("Rules", Some("top to bottom, first match wins"));
                let n = r.rules.len();
                for i in 0..n {
                    rules = rules.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme::S1)
                            .p(theme::S2)
                            .rounded(theme::RADIUS_LG)
                            .border_1()
                            .border_color(theme::LINE)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme::S1)
                                    .child(div().flex_1().text_label().fg_muted().child(format!(
                                        "Rule {} → port rule{}",
                                        i + 1,
                                        i + 1
                                    )))
                                    .child(
                                        IconButton::new(("pl-rule-up", i), AppIcon::ChevronLeft)
                                            .small()
                                            .tooltip("Move up")
                                            .disabled(i == 0)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.edit_graph(cx, |g, id| {
                                                    edit::swap_rules(g, id, i, i - 1)
                                                })
                                            })),
                                    )
                                    .child(
                                        IconButton::new(("pl-rule-down", i), AppIcon::ChevronRight)
                                            .small()
                                            .tooltip("Move down")
                                            .disabled(i + 1 == n)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.edit_graph(cx, |g, id| {
                                                    edit::swap_rules(g, id, i, i + 1)
                                                })
                                            })),
                                    )
                                    .child(
                                        IconButton::new(("pl-rule-del", i), AppIcon::Trash)
                                            .small()
                                            .tooltip("Remove rule")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.edit_graph(cx, |g, id| {
                                                    edit::remove_rule(g, id, i)
                                                })
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(theme::S1)
                                    .child(
                                        div().flex_1().min_w_0().children(select(F::RuleField(i))),
                                    )
                                    .child(div().w(px(120.)).children(select(F::RuleOp(i)))),
                            )
                            .children(input(F::RuleValue(i))),
                    );
                }
                rules = rules.child(
                    Button::new("pl-rule-add", "Add rule")
                        .small()
                        .icon(AppIcon::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.edit_node(true, cx, |n| {
                                if let NodeKind::Router(r) = &mut n.kind {
                                    r.rules.push(RouterRule {
                                        field: "input".into(),
                                        op: RuleOp::Contains,
                                        value: String::new(),
                                    });
                                }
                            })
                        })),
                );
                col = col.child(rules.child(hint("Fields: input, title, or out.<node> (an upstream step's summary). No match takes the else port.")));
            }
            NodeKind::Agent(a) => col = self.agent_sections(a, node, insp, d, cx, col),
            NodeKind::Render(r) => {
                col = col
                    .child(sec("Workdir", None).children(select(F::Workdir)))
                    .when(r.workdir.is_none(), |el| {
                        el.child(div().px(theme::S5).child(warn("A render needs a workdir.")))
                    })
                    .child(
                        sec("Files", Some("glob, relative to the workdir"))
                            .children(input(F::Files)),
                    )
                    .child(
                        sec("Size", Some("viewport in px")).child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::S2)
                                .child(div().flex_1().children(input(F::Width)))
                                .child(div().fg_muted().child("×"))
                                .child(div().flex_1().children(input(F::Height))),
                        ),
                    )
                    .child(sec("Command", Some("optional")).children(input(F::RenderCommand)));
            }
            NodeKind::Gate(gate) => {
                let kind = gate.kind;
                col = col
                    .child(
                        sec("Kind", None)
                            .child(segmented(
                                "pl-gate-kind",
                                &["Pick a variant", "Approve PR"],
                                if kind == GateKind::Pick { 0 } else { 1 },
                                cx.listener(|this, ix: &usize, _, cx| {
                                    let k = if *ix == 0 { GateKind::Pick } else { GateKind::ApprovePr };
                                    this.edit_node(false, cx, |n| {
                                        if let NodeKind::Gate(g) = &mut n.kind {
                                            g.kind = k;
                                        }
                                    })
                                }),
                            ))
                            .child(hint(match kind {
                                GateKind::Pick => "Shows upstream variants side by side; you pick one or send back.",
                                GateKind::ApprovePr => "Shows the run's pull request; approving merges it.",
                            })),
                    )
                    .child(sec("Prompt", Some("review item title")).children(input(F::GatePrompt)))
                    .child(
                        sec("Send back to", Some("re-runs with your note as layer 6"))
                            .children(select(F::BackTo)),
                    );
            }
            NodeKind::Tool(t) => {
                let ix = match t {
                    ToolNode::Command { .. } => 0,
                    ToolNode::PullRequest { .. } => 1,
                    ToolNode::MergePr { .. } => 2,
                };
                col = col.child(sec("Tool", None).child(segmented(
                    "pl-tool-kind",
                    &["Command", "Pull request", "Merge PR"],
                    ix,
                    cx.listener(|this, ix: &usize, _, cx| {
                        let ix = *ix;
                        this.edit_node(true, cx, |n| {
                            let wd = match &n.kind {
                                NodeKind::Tool(ToolNode::Command { workdir, .. })
                                | NodeKind::Tool(ToolNode::PullRequest { workdir, .. }) => {
                                    workdir.clone()
                                }
                                _ => None,
                            };
                            let new = match ix {
                                0 => ToolNode::Command {
                                    workdir: wd,
                                    command: String::new(),
                                    allow_failure: false,
                                },
                                1 => ToolNode::PullRequest {
                                    workdir: wd,
                                    draft: false,
                                },
                                _ => ToolNode::MergePr {
                                    method: "squash".into(),
                                },
                            };
                            let same = matches!(
                                (&n.kind, &new),
                                (
                                    NodeKind::Tool(ToolNode::Command { .. }),
                                    ToolNode::Command { .. }
                                ) | (
                                    NodeKind::Tool(ToolNode::PullRequest { .. }),
                                    ToolNode::PullRequest { .. }
                                ) | (
                                    NodeKind::Tool(ToolNode::MergePr { .. }),
                                    ToolNode::MergePr { .. }
                                )
                            );
                            if !same {
                                n.kind = NodeKind::Tool(new);
                            }
                        })
                    }),
                )));
                match t {
                    ToolNode::Command { allow_failure, .. } => {
                        let on = *allow_failure;
                        col = col
                            .child(sec("Workdir", None).children(select(F::Workdir)))
                            .child(
                                sec("Command", Some("runs in the workdir's shell wrapper"))
                                    .children(area(F::Command)),
                            )
                            .child(sec_row(toggle(
                                "pl-tool-allow",
                                on,
                                "Allow failure",
                                "A non-zero exit doesn't fail the run",
                                cx.listener(move |this, _, _, cx| {
                                    this.edit_node(false, cx, |n| {
                                        if let NodeKind::Tool(ToolNode::Command {
                                            allow_failure,
                                            ..
                                        }) = &mut n.kind
                                        {
                                            *allow_failure = !on;
                                        }
                                    })
                                }),
                            )));
                    }
                    ToolNode::PullRequest { draft, workdir } => {
                        let on = *draft;
                        col = col
                            .child(
                                sec("Workdir", Some("its run branch is pushed"))
                                    .children(select(F::Workdir)),
                            )
                            .when(workdir.is_none(), |el| {
                                el.child(
                                    div()
                                        .px(theme::S5)
                                        .child(warn("A pull request needs a workdir.")),
                                )
                            })
                            .child(sec_row(toggle(
                                "pl-tool-draft",
                                on,
                                "Draft pull request",
                                "Open it as a draft",
                                cx.listener(move |this, _, _, cx| {
                                    this.edit_node(false, cx, |n| {
                                        if let NodeKind::Tool(ToolNode::PullRequest {
                                            draft, ..
                                        }) = &mut n.kind
                                        {
                                            *draft = !on;
                                        }
                                    })
                                }),
                            )));
                    }
                    ToolNode::MergePr { method } => {
                        let ix = MERGE_METHODS.iter().position(|m| m == method).unwrap_or(0);
                        col = col.child(sec("Merge method", Some("gh pr merge")).child(segmented(
                            "pl-merge-method",
                            &MERGE_METHODS,
                            ix,
                            cx.listener(|this, ix: &usize, _, cx| {
                                let m = MERGE_METHODS[*ix].to_string();
                                this.edit_node(false, cx, |n| {
                                    if let NodeKind::Tool(ToolNode::MergePr { method }) =
                                        &mut n.kind
                                    {
                                        *method = m;
                                    }
                                })
                            }),
                        )));
                    }
                }
            }
        }

        col = col.child(self.connections_section(node, insp, cx));
        let sources = g
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Source(_)))
            .count();
        let only_source = matches!(node.kind, NodeKind::Source(_)) && sources <= 1;
        col = col.child(sec_row(
            Button::new("pl-node-delete", "Delete node")
                .danger()
                .small()
                .icon(AppIcon::Trash)
                .disabled(only_source)
                .when(only_source, |b| {
                    b.tooltip("Every pipeline needs its Source")
                })
                .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))),
        ));

        Some(
            div()
                .id("pl-inspector")
                .w(px(360.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(theme::LINE)
                .bg(theme::BG_SIDEBAR)
                .child(col.pb(theme::S8))
                .with_animation(
                    ElementId::NamedInteger("pl-inspector-in".into(), ed.sel_seq),
                    Animation::new(theme::DUR_OVERLAY).with_easing(theme::ease_out),
                    |el, t| el.opacity(t).pl(px(24. * (1. - t))),
                )
                .into_any_element(),
        )
    }

    fn agent_sections(
        &self,
        a: &harness_core::AgentNode,
        node: &Node,
        insp: &Inspector,
        d: &super::data::Detail,
        cx: &mut Context<Self>,
        mut col: Div,
    ) -> Div {
        let input = |f: F| {
            insp.inputs
                .get(&f)
                .map(|s| field_input(s).into_any_element())
        };
        let area = |f: F| {
            insp.inputs
                .get(&f)
                .map(|s| area_input(s).into_any_element())
        };
        let select = |f: F| {
            insp.selects
                .get(&f)
                .map(|s| field_select(s).into_any_element())
        };
        let bound = a
            .agent
            .as_deref()
            .and_then(|id| d.agents.iter().find(|x| x.id == id));
        let live = d.pipeline.graph.node(&node.id).and_then(|n| match &n.kind {
            NodeKind::Agent(l) => Some(l.instructions.clone()),
            _ => None,
        });
        let instr_edited = live.as_ref().is_some_and(|l| *l != a.instructions);
        let tokens = a.instructions.chars().count().div_ceil(4);

        let mut agent_sec = sec("Agent", Some("role = prompt layer 2")).children(select(F::Agent));
        match bound {
            Some(def) => {
                let id = def.id.clone();
                agent_sec = agent_sec
                    .child(hint(format!(
                        "{} · {} · {} · v{}. Model and tools below override it for this step.",
                        def.name,
                        Config::short_model(&def.model),
                        def.tools.label(),
                        def.version
                    )))
                    .child(
                        div()
                            .id("pl-insp-agent-link")
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .text_label()
                            .fg_accent()
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::ACCENT))
                            .child(format!("Edit {}'s role in Agents", def.name))
                            .child(icon_sized(AppIcon::ChevronRight, px(12.)))
                            .on_click(move |_, _, cx| state::navigate(Route::Agents { agent: Some(id.clone()) }, cx)),
                    );
            }
            None if a.agent.is_some() => agent_sec = agent_sec.child(warn("The saved agent this step used no longer exists.")),
            None => agent_sec = agent_sec.child(hint("Without a saved agent the step has no role prompt; model and tools come from below.")),
        }
        col = col
            .child(agent_sec)
            .child(sec("Model", None).children(select(F::Model)))
            .child(
                sec_with(
                    "Step instructions",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(Badge::new("layer 3"))
                        .when(instr_edited, |el| el.child(Badge::new("edited").accent()))
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_label()
                                .fg_muted()
                                .child(format!("~{tokens} tokens")),
                        ),
                )
                .children(area(F::Instructions)),
            )
            .child(sec("Workdir", None).children(select(F::Workdir)))
            .child(sec("Tool access", None).children(select(F::Tools)));

        // Skills: always loaded / on demand.
        let mut skills: Vec<String> = self.catalog.skills.clone();
        for s in a.skills.iter().chain(a.skills_on_demand.iter()) {
            if !skills.contains(s) {
                skills.push(s.clone());
            }
        }
        for (title, meta, on_demand) in [
            ("Skills", "always in the prompt", false),
            ("Skills on demand", "loaded with load_skill", true),
        ] {
            let chosen = if on_demand {
                &a.skills_on_demand
            } else {
                &a.skills
            };
            let mut s = sec(title, Some(meta));
            if skills.is_empty() {
                s = s.child(hint(
                    "No skills found in the configured skill folders (Settings).",
                ));
            } else {
                s = s.child(div().flex().flex_wrap().gap(px(6.)).children(
                    skills.iter().enumerate().map(|(ix, name)| {
                        let on = chosen.contains(name);
                        let name = name.clone();
                        div()
                            .id(ElementId::NamedInteger(
                                if on_demand {
                                    "pl-skill-od".into()
                                } else {
                                    "pl-skill".into()
                                },
                                ix as u64,
                            ))
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .h(px(24.))
                            .px(theme::S2)
                            .rounded(theme::RADIUS_MD)
                            .border_1()
                            .text_mono_sm()
                            .cursor_pointer()
                            .when(on, |el| {
                                el.bg(theme::ACCENT_SOFT)
                                    .border_color(theme::ACCENT_RING)
                                    .text_color(theme::ACCENT_TEXT)
                            })
                            .when(!on, |el| {
                                el.bg(theme::SURFACE_RAISED)
                                    .border_color(theme::LINE_STRONG)
                                    .text_color(theme::TEXT_SECONDARY)
                                    .hover(|s| s.border_color(theme::TEXT_DISABLED))
                            })
                            .when(on, |el| el.child(icon_sized(AppIcon::Check, px(11.))))
                            .child(name.clone())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let name = name.clone();
                                this.edit_node(false, cx, move |n| {
                                    if let NodeKind::Agent(a) = &mut n.kind {
                                        let list = if on_demand {
                                            &mut a.skills_on_demand
                                        } else {
                                            &mut a.skills
                                        };
                                        if let Some(p) = list.iter().position(|s| *s == name) {
                                            list.remove(p);
                                        } else {
                                            list.push(name);
                                        }
                                    }
                                })
                            }))
                    }),
                ));
            }
            col = col.child(s);
        }

        col = col.child(
            sec("Max steps", Some("model ↔ tool turns before it stops"))
                .children(input(F::MaxSteps)),
        );

        // Variants.
        let v = a.variants.max(1);
        let mut var_sec = sec("Variants", Some("each its own task and worktree")).child(
            div()
                .flex()
                .items_center()
                .gap(theme::S3)
                .child(stepper(
                    v,
                    1,
                    10,
                    cx.listener(|this, delta: &i32, _, cx| {
                        let delta = *delta;
                        this.edit_node(true, cx, move |n| {
                            if let NodeKind::Agent(a) = &mut n.kind {
                                a.variants = (a.variants as i32 + delta).clamp(1, 10) as u32;
                                a.variant_hints.truncate(a.variants as usize);
                            }
                        })
                    }),
                ))
                .child(div().text_label().fg_muted().child(if v > 1 {
                    format!("fans out into {v} variants for a gate to pick from")
                } else {
                    "one result".to_string()
                })),
        );
        if v > 1 {
            if let Some(wd) = a
                .workdir
                .as_deref()
                .and_then(|w| self.ed.as_ref()?.graph.workdir(w))
            {
                if !wd.worktree {
                    var_sec = var_sec.child(warn(
                        "Variants need worktrees on in this workdir (Settings).",
                    ));
                }
            }
            for i in 0..v as usize {
                var_sec = var_sec.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .child(VariantChip::variant(i as u32))
                        .child(div().flex_1().min_w_0().children(input(F::Hint(i)))),
                );
            }
        }
        col = col.child(var_sec);

        // Delegate.
        let on = a.delegate.is_some();
        let default_model = self.catalog.default_model.clone();
        let mut del = sec("Delegate", Some("hand sub-tasks to a sub-agent")).child(toggle(
            "pl-delegate",
            on,
            "Allow the delegate tool",
            "The agent can spawn a coder sub-agent for parts of the work",
            cx.listener(move |this, _, _, cx| {
                let default_model = default_model.clone();
                this.edit_node(true, cx, move |n| {
                    if let NodeKind::Agent(a) = &mut n.kind {
                        a.delegate = if a.delegate.is_some() {
                            None
                        } else {
                            Some(DelegateConfig {
                                agent: None,
                                name: "coder".into(),
                                model: default_model,
                                instructions: String::new(),
                                skills_on_demand: vec![],
                                max_steps: harness_core::default_max_steps(),
                            })
                        };
                    }
                })
            }),
        ));
        if on {
            del = del
                .child(label("Name"))
                .children(input(F::DelName))
                .child(label("Model"))
                .children(select(F::DelModel))
                .child(label("Instructions"))
                .children(area(F::DelInstructions))
                .child(label("Max steps"))
                .children(input(F::DelMaxSteps));
        }
        col.child(del)
    }

    fn connections_section(&self, node: &Node, insp: &Inspector, cx: &mut Context<Self>) -> Div {
        let Some(ed) = self.ed.as_ref() else {
            return div();
        };
        let g = &ed.graph;
        let mut s = sec("Connections", Some("where each output goes"));
        let ports = node.kind.ports();
        let mut rows = 0usize;
        let mut ports_all: Vec<Option<String>> = ports.clone();
        for e in g.edges.iter().filter(|e| e.from == node.id) {
            if !ports_all.contains(&e.port) {
                ports_all.push(e.port.clone());
            }
        }
        for port in ports_all {
            let stray = !ports.contains(&port);
            let title = match (edit::port_label(&port), stray) {
                (Some(l), false) => l,
                (Some(l), true) => format!("{l} (no such port)"),
                (None, _) => "next".to_string(),
            };
            let targets: Vec<&harness_core::Edge> = g
                .edges
                .iter()
                .filter(|e| e.from == node.id && e.port == port)
                .collect();
            let mut block = div()
                .flex()
                .flex_col()
                .gap(theme::S1)
                .child(div().text_label().fg_secondary().child(title));
            for e in targets {
                let to = e.to.clone();
                let to_name = g
                    .node(&e.to)
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| e.to.clone());
                let p = port.clone();
                let ix = rows;
                rows += 1;
                block = block.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S2)
                        .h(px(32.))
                        .px(theme::S2)
                        .rounded(theme::RADIUS_LG)
                        .border_1()
                        .border_color(theme::LINE_STRONG)
                        .bg(theme::SURFACE_RAISED)
                        .child(
                            icon_sized(AppIcon::ChevronRight, theme::ICON_SM)
                                .text_color(theme::TEXT_MUTED),
                        )
                        .child(
                            div()
                                .id(("pl-conn-go", ix))
                                .flex_1()
                                .min_w_0()
                                .ellipsis()
                                .text_body()
                                .fg()
                                .cursor_pointer()
                                .hover(|s| s.text_color(theme::ACCENT_TEXT))
                                .child(to_name)
                                .on_click({
                                    let to = to.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_node(Some(to.clone()), cx)
                                    })
                                }),
                        )
                        .child(
                            IconButton::new(("pl-conn-del", ix), AppIcon::X)
                                .small()
                                .tooltip("Disconnect")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let (p, to) = (p.clone(), to.clone());
                                    this.edit_graph(cx, move |g, id| {
                                        edit::disconnect(g, id, &p, &to)
                                    })
                                })),
                        ),
                );
            }
            if !stray {
                block = block.children(
                    insp.selects
                        .get(&F::Connect(port.clone()))
                        .map(|st| field_select(st).into_any_element()),
                );
            }
            s = s.child(block);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Small inspector pieces
// ---------------------------------------------------------------------------

/// Inspector section (`.sec`): 16/20 padding, top rule, title + muted meta.
fn sec(title: &str, meta: Option<&str>) -> Div {
    sec_with(
        title,
        div().when_some(meta, |el, m| {
            el.child(div().text_label().fg_muted().child(m.to_string()))
        }),
    )
}

fn sec_with(title: &str, meta: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::S2)
        .px(theme::S5)
        .py(theme::S4)
        .border_t_1()
        .border_color(theme::LINE)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(div().text_body_strong().fg().child(title.to_string()))
                .child(div().flex_1().min_w_0().child(meta)),
        )
}

/// A section that is just one row (toggles, delete).
fn sec_row(el: impl IntoElement) -> Div {
    div()
        .flex()
        .px(theme::S5)
        .py(theme::S4)
        .border_t_1()
        .border_color(theme::LINE)
        .child(el)
}

fn hint(text: impl Into<SharedString>) -> Div {
    div().text_label().fg_muted().child(text.into())
}

fn warn(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme::S1)
        .text_label()
        .text_color(theme::WARNING)
        .child(icon_sized(AppIcon::Alert, px(12.)))
        .child(text.into())
}

/// 32px single-line input.
pub(super) fn field_input(s: &Entity<InputState>) -> Input {
    Input::new(s)
        .h(px(32.))
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
        .text_size(px(13.))
}

/// Multi-line input that grows with its text.
pub(super) fn area_input(s: &Entity<InputState>) -> Input {
    Input::new(s)
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
        .text_size(px(13.))
}

/// 32px select.
pub(super) fn field_select(s: &Entity<SelectState<Options>>) -> Select<Options> {
    Select::new(s)
        .h(px(32.))
        .text_size(px(13.))
        .bg(theme::SURFACE_RAISED)
        .rounded(theme::RADIUS_LG)
}

/// On/off switch with a label and a muted description.
pub(super) fn toggle(
    id: impl Into<ElementId>,
    on: bool,
    title: &str,
    desc: &str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_start()
        .gap(theme::S3)
        .cursor_pointer()
        .child(
            div()
                .mt(px(2.))
                .flex_none()
                .w(px(30.))
                .h(px(18.))
                .p(px(2.))
                .rounded(theme::RADIUS_FULL)
                .bg(if on {
                    theme::ACCENT_STRONG
                } else {
                    theme::LINE_STRONG
                })
                .flex()
                .when(on, |el| el.justify_end())
                .child(
                    div()
                        .size(px(14.))
                        .rounded(theme::RADIUS_FULL)
                        .bg(theme::TEXT),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().text_body().fg().child(title.to_string()))
                .child(div().text_label().fg_muted().child(desc.to_string())),
        )
        .on_click(on_click)
}

/// `− N +` stepper; the handler gets −1 or +1.
fn stepper(
    value: u32,
    min: u32,
    max: u32,
    on_step: impl Fn(&i32, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_step = std::rc::Rc::new(on_step);
    let btn = |id: &'static str, label: &'static str, delta: i32, enabled: bool| {
        let on_step = on_step.clone();
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.))
            .h_full()
            .rounded(theme::RADIUS_MD)
            .fg_muted()
            .when(enabled, |el| {
                el.cursor_pointer()
                    .hover(|s| s.text_color(theme::TEXT))
                    .on_click(move |_, w, cx| on_step(&delta, w, cx))
            })
            .when(!enabled, |el| el.opacity(0.45).cursor_not_allowed())
            .child(label)
    };
    div()
        .flex()
        .items_center()
        .h(px(32.))
        .rounded(theme::RADIUS_MD)
        .border_1()
        .border_color(theme::LINE_STRONG)
        .bg(theme::SURFACE_RAISED)
        .child(btn("pl-step-dec", "−", -1, value > min))
        .child(
            div()
                .min_w(px(20.))
                .text_center()
                .text_body()
                .fg()
                .child(value.to_string()),
        )
        .child(btn("pl-step-inc", "+", 1, value < max))
}

/// Segmented control whose handler gets the index (works with `cx.listener`).
pub(super) fn segmented(
    id: &'static str,
    items: &[&str],
    selected: usize,
    on_select: impl Fn(&usize, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    ui::segmented(id, items, selected, move |ix, w, cx| on_select(&ix, w, cx))
}
