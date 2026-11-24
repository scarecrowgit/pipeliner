//! Agent and orchestrator nodes as the mockup's inspector draws them, sub-agent templates in
//! the same language, and the name and id every node has.

use std::rc::Rc;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};
use hub::api::{AgentDef, RunnableKind};
use hub::model::{
    AgentNode, EdgeDef, EdgeKind, FileMode, FileNode, GraphDef, NodeDef, NodeId, NodeKind, Pos, RunnableRef, SubagentNode,
};
use ui::dev::Probe;
use ui::prelude::*;

use super::fields::{SetModel, SetWorkdir, Spec, count_value, on_agent, on_kind, on_node, parse_count, parse_usd, usd_text, usd_value};
use super::form::{INNER, OnPick, Pick, field, note, psec, removable_tag, row_field, tag_row, titled};
use super::grants::Owner;
use super::{InspectorView, ops};
use crate::Selection;

/// The longest path in a file tag: what fits the panel in mono 11px.
const TAG_CHARS: usize = 44;

impl InspectorView {
    /// The library agent behind `reference`, if the catalog has it.
    fn library_def(&self, reference: Option<&RunnableRef>, cx: &gpui::App) -> Option<AgentDef> {
        let reference = reference?;
        self.catalog.read(cx).runnable(reference).and_then(|r| r.agent.clone())
    }

    /// Name and id side by side. The id is checked for shape and uniqueness as it is typed,
    /// and renaming it re-points the edges.
    pub(super) fn name_and_id(&mut self, graph: &GraphDef, node: &NodeDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let name = self.line(
            Spec::new("insp-name", node.name.clone(), "Rename node", on_node(|node, text| node.name = text.to_string())),
            None,
            window,
            cx,
        );
        let graph_for_id = Rc::new(graph.clone());
        let id = self.line(
            Spec::new(
                "insp-id",
                node.id.to_string(),
                "Change node id",
                Rc::new(|graph, shown, text| {
                    if let Selection::Node(old) = shown {
                        ops::rename_node(graph, old, &NodeId::new(text));
                    }
                }),
            )
            .parse({
                let old = node.id.clone();
                move |text| match ops::node_id_error(&graph_for_id, &old, text) {
                    Some(error) => Err(error.into()),
                    None => Ok(text.to_string()),
                }
            })
            .after(Rc::new(|this, text, cx| {
                let new = Selection::Node(NodeId::new(text));
                this.shown = new.clone();
                this.select(new, cx);
            }))
            .mono(),
            self.problem_at(&node.id, "id", cx),
            window,
            cx,
        );
        row()
            .items_start()
            .gap(px(SPACE_2))
            .child(div().flex_1().min_w_0().child(field("Name", name)))
            .child(div().w(px(112.)).flex_none().child(field("Id", id)))
            .into_any_element()
    }

    /// The first section of a node that is not an agent: name and id.
    pub(super) fn identity_section(&mut self, graph: &GraphDef, node: &NodeDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        psec().child(self.name_and_id(graph, node, window, cx)).into_any_element()
    }

    pub(super) fn agent_sections(&mut self, graph: &GraphDef, node: &NodeDef, agent: &AgentNode, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let def = self.library_def(agent.runnable.as_ref(), cx);
        let inherited = def.clone().unwrap_or_default();
        let orchestrator = matches!(node.kind, NodeKind::Orchestrator(_));
        vec![
            self.agent_main(graph, node, agent, def.as_ref(), orchestrator, window, cx),
            self.agent_work(graph, node, agent, window, cx),
            self.files_section(graph, node, agent, window, cx),
            self.tools_section(agent.tools.as_ref(), &inherited.tools, Owner::Agent, window, cx),
            self.skills_section(agent.skills.as_ref(), &inherited.skills, Owner::Agent, window, cx),
            self.spawn_section(graph, node, agent.spawn.as_ref(), &inherited.spawn, window, cx),
            self.agent_limits(node, agent, &inherited, window, cx),
        ]
    }

    /// The library agent, name and id, role and model.
    #[expect(clippy::too_many_arguments, reason = "the first section of an agent node")]
    fn agent_main(
        &mut self,
        graph: &GraphDef,
        node: &NodeDef,
        agent: &AgentNode,
        def: Option<&AgentDef>,
        orchestrator: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = if orchestrator { RunnableKind::Orchestrator } else { RunnableKind::Agent };
        let word = if orchestrator { "orchestrator" } else { "agent" };
        let runnables: Vec<(RunnableRef, String)> =
            self.catalog.read(cx).of_kind(kind).map(|r| (r.reference.clone(), r.name.clone())).collect();
        let current = agent.runnable.as_ref();
        let mut picks = vec![Pick::new("None: defined here").sub("Role, model and tools come from this node").checked(current.is_none())];
        picks.extend(runnables.iter().map(|(r, name)| Pick::new(format!("{name} · {word}")).icon(if orchestrator { IconName::Orchestrator } else { IconName::Agent }).checked(Some(r) == current)));
        let value = match current {
            None => "None: defined here".to_string(),
            Some(r) => match runnables.iter().find(|(rr, _)| rr == r) {
                Some((_, name)) => format!("{name} · {word}"),
                None => format!("{} · missing", r.id()),
            },
        };
        let refs: Vec<RunnableRef> = runnables.into_iter().map(|(r, _)| r).collect();
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let reference = ix.checked_sub(1).and_then(|i| refs.get(i).cloned());
            this.edit_node("Set library agent", move |node| { if let Some(agent) = ops::agent_mut(node) { agent.runnable = reference; } }, cx);
        });
        let picker = self.picker("insp-runnable", value, current.is_none(), Some(if orchestrator { IconName::Orchestrator } else { IconName::Agent }), picks, on_pick, window, cx);
        let role_placeholder = def.map(|d| d.role.clone()).filter(|r| !r.is_empty()).unwrap_or_else(|| "What it does, how, and when it is done".to_string());
        let role = self.area(
            Spec::new("insp-role", agent.role.clone(), "Edit role", on_agent(|agent, text| agent.role = text.to_string()))
                .area(3, 12)
                .placeholder(role_placeholder),
            self.problem_at(&node.id, "role", cx),
            window,
            cx,
        );
        let set_model: SetModel = Rc::new(|node, model| {
            if let Some(agent) = ops::agent_mut(node) {
                agent.model = model;
            }
        });
        let inherited_model = def.map(|d| d.model.clone()).filter(|m| !m.is_empty());
        let model = self.model_field(
            "insp-model",
            agent.model.clone().unwrap_or_default(),
            inherited_model,
            set_model,
            self.problem_at(&node.id, "model", cx),
            window,
            cx,
        );
        let library_note = if current.is_some() {
            "The same agent you assign on the board or talk to in chat. Changes here apply only inside this pipeline."
        } else {
            "Everything this node does is set here. Pick a library agent to reuse one you assign on the board or talk to in chat."
        };
        psec()
            .child(field("Agent from your library", picker))
            .children(self.problem_line(self.problem_at(&node.id, "runnable", cx), INNER, cx))
            .child(note(library_note, INNER).mt(px(SPACE_1_5)))
            .child(div().mt(px(SPACE_3)).child(self.name_and_id(graph, node, window, cx)))
            .child(div().mt(px(SPACE_3)).child(field("Role", role)))
            .child(div().mt(px(SPACE_3)).child(field("Model", model)))
            .into_any_element()
    }

    /// Where it works and how many variants it runs.
    fn agent_work(&mut self, graph: &GraphDef, node: &NodeDef, agent: &AgentNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let workdir = self.workdir_picker(
            "insp-workdir",
            graph,
            &agent.workdir,
            Rc::new(|node, id| {
                if let Some(agent) = ops::agent_mut(node) {
                    agent.workdir = id;
                }
            }),
            window,
            cx,
        );
        let variants = Stepper::new("insp-variants", agent.variants as i32)
            .range(1, 8)
            .on_change(self.on_value(cx, |this, v: i32, cx| {
                this.edit_node("Set variants", move |node| { if let Some(agent) = ops::agent_mut(node) { agent.variants = v as u32; } }, cx)
            }));
        let explain = if agent.variants > 1 {
            format!("×{}: each variant runs in its own sandbox. A person picks one in Review.", agent.variants)
        } else {
            "Each variant runs in its own sandbox. A person picks one in Review.".to_string()
        };
        psec()
            .child(field("Workdir", workdir))
            .children(self.problem_line(self.problem_at(&node.id, "workdir", cx), INNER, cx))
            .child(row_field("Variants", variants.probe("insp-variants")).mt(px(SPACE_3)))
            .children(self.problem_line(self.problem_at(&node.id, "variants", cx), INNER, cx))
            .child(note(explain, INNER).mt(px(SPACE_1_5)))
            .into_any_element()
    }

    /// Files in (file nodes feeding it through context edges) and files out (`files_out`).
    fn files_section(&mut self, graph: &GraphDef, node: &NodeDef, agent: &AgentNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let files_in: Vec<AnyElement> = graph
            .edges_to(&node.id)
            .filter(|e| e.kind == EdgeKind::Context)
            .filter_map(|e| graph.node(&e.from))
            .filter_map(|n| match &n.kind {
                NodeKind::File(file) => Some(file.path.clone()),
                _ => None,
            })
            .flat_map(|path| path.split(", ").map(str::to_string).collect::<Vec<_>>())
            // The tag does not shrink: a long path is cut to what fits the panel.
            .map(|path| Tag::new(base::fmt::truncate_chars(&path, TAG_CHARS)).mono().into_any_element())
            .collect();
        let add_in = Button::new("insp-files-in-add", "Add file")
            .ghost()
            .small()
            .icon(IconName::Plus)
            .on_click(self.on_click(cx, |this, _, cx| this.add_context_file(cx)))
            .probe("insp-files-in-add");
        let mut out: Vec<AnyElement> = Vec::new();
        for (i, file) in agent.files_out.iter().enumerate() {
            let remove = self.on_click(cx, move |this, _, cx| {
                this.edit_node("Remove file out", move |node| {
                    if let Some(agent) = ops::agent_mut(node)
                        && i < agent.files_out.len()
                    {
                        agent.files_out.remove(i);
                    }
                }, cx)
            });
            out.push(removable_tag(format!("insp-files-out-{i}"), file.clone(), TagTone::Neutral, remove, window, cx));
        }
        if let Some(next) = graph.edges_from(&node.id).filter(|e| e.kind == EdgeKind::Route).find_map(|e| graph.node(&e.to)) {
            out.push(row().gap(px(SPACE_1)).type_style(LABEL).text_color(TEXT_MUTED).child(icon(IconName::ArrowRight, 12., TEXT_MUTED)).child(next.name.clone()).into_any_element());
        }
        let add_out = self.line(
            Spec::new("insp-files-out-add", "", "Add file out", on_agent(|agent, text| {
                let path = text.trim().to_string();
                if !agent.files_out.contains(&path) {
                    agent.files_out.push(path);
                }
            }))
            .on_submit()
            .mono()
            .placeholder("Add a file it must leave, like summary.md"),
            None,
            window,
            cx,
        );
        let empty_in = files_in.is_empty();
        titled("Files in", Some(add_in.into_any_element()))
            .when(empty_in, |d| d.child(note("No file nodes feed it. Add one to give it context from a workdir.", INNER)))
            .when(!empty_in, |d| d.child(tag_row(INNER, files_in)))
            .child(div().mt(px(SPACE_3_5)).mb(px(SPACE_2)).type_style(BODY_STRONG).child("Files out"))
            .when(!out.is_empty(), |d| d.child(tag_row(INNER, out)))
            .child(div().mt(px(SPACE_2)).w(px(INNER)).child(add_out))
            .child(note("Checked before the node counts as done, relative to its workdir.", INNER).mt(px(SPACE_1_5)))
            .into_any_element()
    }

    /// A read-mode file node on the left of the agent, feeding it through a context edge.
    fn add_context_file(&mut self, cx: &mut Context<Self>) {
        let Selection::Node(target) = self.shown.clone() else { return };
        let mut added = None;
        self.edit(
            "Add file",
            |graph| {
                let Some(at) = graph.node(&target) else { return };
                let workdir = graph.node(&target).and_then(ops::agent_of).map(|a| a.workdir.clone()).unwrap_or_default();
                let pos = Pos { x: at.pos.x - 192., y: at.pos.y + 40. };
                let id = NodeId::new(ops::unique("file", |id| graph.nodes.iter().any(|n| n.id.as_str() == id)));
                graph.nodes.push(NodeDef { id: id.clone(), name: "Files".into(), pos, kind: NodeKind::File(FileNode { workdir, path: String::new(), mode: FileMode::Read }) });
                graph.edges.push(EdgeDef { from: id.clone(), port: hub::model::PORT_OUT.into(), to: target.clone(), kind: EdgeKind::Context });
                added = Some(id);
            },
            cx,
        );
        if let Some(id) = added {
            self.select(Selection::Node(id), cx);
            self.focus_first = true;
        }
    }

    /// Budget per item and max steps, with the spend of the test run on show.
    fn agent_limits(&mut self, node: &NodeDef, agent: &AgentNode, inherited: &AgentDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let budget_ph = inherited.budget_usd.map(|b| format!("{b:.2}")).unwrap_or_else(|| "No cap".into());
        let budget = self.line(
            Spec::new("insp-budget", usd_text(agent.budget_usd), "Set budget", on_agent(|agent, text| agent.budget_usd = usd_value(text)))
                .parse(parse_usd)
                .mono()
                .placeholder(budget_ph),
            self.problem_at(&node.id, "budget", cx),
            window,
            cx,
        );
        let steps_ph = match inherited.max_steps {
            0 => "Default".to_string(),
            n => n.to_string(),
        };
        let steps = self.line(
            Spec::new("insp-max-steps", agent.max_steps.map(|n| n.to_string()).unwrap_or_default(), "Set max steps", on_agent(|agent, text| agent.max_steps = count_value(text)))
                .parse(parse_count)
                .mono()
                .placeholder(steps_ph),
            self.problem_at(&node.id, "max-steps", cx),
            window,
            cx,
        );
        let cap = agent.budget_usd.or(inherited.budget_usd);
        let spent = self.run.as_ref().and_then(|(run, _)| {
            let run = run.read(cx).run()?;
            let instances: Vec<_> = run.nodes.iter().filter(|n| n.node == node.id).collect();
            (!instances.is_empty()).then(|| instances.iter().map(|n| n.cost_usd).fold(0.0_f64, f64::max))
        });
        let spend = spent.map(|spent| {
            let of = cap.map(|c| format!(" of {}", base::fmt::cost(c))).unwrap_or_default();
            let fraction = cap.filter(|c| *c > 0.0).map(|c| (spent / c).min(1.0) as f32).unwrap_or(0.0);
            col()
                .gap(px(SPACE_2))
                .child(row().child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Spent in the test run")).child(div().type_style(MONO).child(base::fmt::cost(spent))).child(div().type_style(MONO).text_color(TEXT_MUTED).child(of)))
                .child(Progress::new(fraction))
        });
        psec()
            .gap(px(SPACE_2))
            .child(row().gap(px(SPACE_3)).child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Budget per issue")).child(row().w(px(120.)).gap(px(SPACE_1)).child(div().text_color(TEXT_MUTED).child("$")).child(budget)))
            .child(row().gap(px(SPACE_3)).child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Max steps")).child(div().w(px(120.)).child(steps)))
            .children(spend)
            .into_any_element()
    }

    pub(super) fn subagent_sections(&mut self, _graph: &GraphDef, node: &NodeDef, sub: &SubagentNode, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let role = self.area(
            Spec::new("insp-role", sub.role.clone(), "Edit role", on_kind(|kind, text| { if let NodeKind::Subagent(sub) = kind { sub.role = text.to_string(); } }))
                .area(3, 12)
                .placeholder("What the created agent does and what it hands back"),
            self.problem_at(&node.id, "role", cx),
            window,
            cx,
        );
        let set_model: SetModel = Rc::new(|node, model| {
            if let NodeKind::Subagent(sub) = &mut node.kind {
                sub.model = model.unwrap_or_default();
            }
        });
        let model = self.model_field("insp-model", sub.model.clone(), None, set_model, self.problem_at(&node.id, "model", cx), window, cx);
        let steps = self.line(
            Spec::new(
                "insp-max-steps",
                if sub.max_steps == 0 { String::new() } else { sub.max_steps.to_string() },
                "Set max steps",
                on_kind(|kind, text| { if let NodeKind::Subagent(sub) = kind { sub.max_steps = count_value(text).unwrap_or(0); } }),
            )
            .parse(parse_count)
            .mono()
            .placeholder("Default"),
            self.problem_at(&node.id, "max-steps", cx),
            window,
            cx,
        );
        let per_run = Stepper::new("insp-per-run", sub.per_run as i32).range(0, 20).on_change(self.on_value(cx, |this, v: i32, cx| {
            this.edit_node("Set per run", move |node| { if let NodeKind::Subagent(sub) = &mut node.kind { sub.per_run = v as u32; } }, cx)
        }));
        let main = psec()
            .gap(px(SPACE_3))
            .child(field("Role", role))
            .child(field("Model", model))
            .child(note("A template: the agent it is attached to creates these through its spawn tool. Never scheduled on its own.", INNER))
            .into_any_element();
        let limits = psec()
            .gap(px(SPACE_2))
            .child(row_field("Per run", per_run.probe("insp-per-run")))
            .children(self.problem_line(self.problem_at(&node.id, "per-run", cx), INNER, cx))
            .child(note("How many of these one parent run may create.", INNER))
            .child(row().mt(px(SPACE_2)).gap(px(SPACE_3)).child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Max steps")).child(div().w(px(120.)).child(steps)))
            .into_any_element();
        vec![
            main,
            self.tools_section(Some(&sub.tools), &sub.tools, Owner::Subagent, window, cx),
            self.skills_section(Some(&sub.skills), &sub.skills, Owner::Subagent, window, cx),
            limits,
        ]
    }

    /// A select box over the pipeline's workdirs; `set` gets the picked id.
    pub(super) fn workdir_picker(
        &mut self,
        key: &str,
        graph: &GraphDef,
        current: &str,
        set: SetWorkdir,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ids: Vec<String> = graph.workdirs.iter().map(|w| w.id.clone()).collect();
        let picks: Vec<Pick> = if ids.is_empty() {
            vec![Pick::new("No workdirs yet").sub("Add one with nothing selected").soon()]
        } else {
            graph
                .workdirs
                .iter()
                .map(|w| Pick::new(w.name.clone()).sub(hub::display_path(&w.path, cx)).hint(w.id.clone()).icon(IconName::Folder).checked(w.id == current))
                .collect()
        };
        let value: SharedString = match graph.workdir(current) {
            Some(w) => format!("{} · {}", w.name, hub::display_path(&w.path, cx)).into(),
            None if current.is_empty() => "Pick a workdir".into(),
            None => format!("{current} · missing").into(),
        };
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let (Some(id), set) = (ids.get(ix).cloned(), set.clone()) else { return };
            this.edit_node("Set workdir", move |node| set(node, id), cx);
        });
        self.picker(key.to_string(), value, current.is_empty(), Some(IconName::Folder), picks, on_pick, window, cx)
    }
}
