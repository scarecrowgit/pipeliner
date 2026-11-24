//! The detail panel of a pipeline: what it is (live version, nodes, agents inside, queue),
//! what the draft changed since the live version, where it is used from, and "Open in editor",
//! which leaves for `Route::Editor`.

use gpui::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{GraphChange, NodeKind, PipelineId, Runnable, Target, VersionState};
use hub::model::RunnableRef;
use ui::dev::Probe;
use ui::prelude::*;

use crate::detail::{TEXT_W, lives_in, section};
use crate::rows;
use crate::view::{AgentsView, open_editor};

/// Probe of the "Open in editor" button.
pub(crate) const OPEN_EDITOR: &str = "agents-open-editor";

impl AgentsView {
    pub(crate) fn pipeline_panel(&mut self, runnable: &Runnable, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let id = PipelineId::new(runnable.reference.id());
        let catalog = self.catalog.read(cx);
        let activity = catalog.activity(&runnable.reference).cloned();
        let graph = catalog.pipeline(&id).and_then(|g| g.value()).and_then(|g| g.as_ref().clone());
        let live = catalog
            .versions(&id)
            .and_then(|v| v.value())
            .and_then(|v| v.iter().rev().find(|v| v.state == VersionState::Live).map(|v| v.version));

        let open = runnable.reference.clone();
        let mut buttons = vec![
            Button::new(OPEN_EDITOR, "Open in editor")
                .secondary()
                .small()
                .icon(IconName::External)
                .on_click(move |_, _, cx| open_editor(&open, cx))
                .probe(OPEN_EDITOR)
                .into_any_element(),
        ];
        buttons.extend(self.agent_buttons(runnable, cx));

        let version: SharedString = match live {
            Some(v) => format!("v{v}").into(),
            None => match activity.as_ref().map(|a| &a.state) {
                Some(hub::api::RunnableState::Live { version }) => format!("v{version}").into(),
                _ => "not deployed".into(),
            },
        };
        let mut facts = KeyValue::new().row("Live version", div().font_family(FONT_MONO).child(version));
        if let Some(graph) = &graph {
            facts = facts.row(
                "Nodes",
                div().child(format!(
                    "{} · {} inside",
                    base::fmt::plural(graph.nodes.len() as u64, "node", "nodes"),
                    base::fmt::plural(rows::agents_inside(graph) as u64, "agent", "agents")
                )),
            );
        }
        let queued = activity.as_ref().map_or(0, |a| a.queued);
        if queued > 0 {
            facts = facts.row("Queued", div().child(base::fmt::count(u64::from(queued))));
        }
        facts = facts
            .row("Activity", div().child(rows::runs_today(activity.as_ref())))
            .row("Lives in", div().min_w_0().child(truncate(lives_in(runnable, cx))));
        let head = self.panel_head(runnable, buttons, facts, window, cx);

        let changes = self.changes_section(&id, live, cx);
        let uses = self.uses_section(runnable, false, cx);
        let inside = self.inside_section(graph.as_ref(), cx);
        let runs = self.runs_section(&id, cx);
        let used = self.used_by_section(runnable, cx);
        vec![head, changes, uses, inside, runs, used]
    }

    fn changes_section(&mut self, id: &PipelineId, live: Option<u32>, _cx: &mut Context<Self>) -> AnyElement {
        let title = "Draft changes";
        let since: SharedString = live.map_or_else(|| "not deployed yet".into(), |v| format!("since v{v}").into());
        let body: AnyElement = match self.changes.get(id) {
            None => Note::new("Reading the draft…").into_any_element(),
            Some(changes) if changes.is_empty() => Note::new("The draft is the live version.").into_any_element(),
            Some(changes) => col()
                .children(changes.iter().enumerate().map(|(ix, change)| {
                    let (sign, color, name, what) = match change {
                        GraphChange::Added { name, what, .. } => ("+", SUCCESS, name.clone(), what.clone()),
                        GraphChange::Changed { name, what, .. } => ("~", WARNING, name.clone(), what.clone()),
                        GraphChange::Removed { name, .. } => ("−", DANGER, name.clone(), "removed".to_string()),
                        GraphChange::Wiring { what } => ("·", TEXT_MUTED, "Wiring".to_string(), what.clone()),
                    };
                    row()
                        .id(("agents-change", ix))
                        .min_h(px(28.))
                        .gap(px(SPACE_2))
                        .child(div().w(px(12.)).flex_none().font_family(FONT_MONO).text_color(color).child(sign))
                        .child(div().flex_none().type_style(BODY_STRONG).child(name))
                        .child(div().flex_1().min_w_0().type_style(LABEL).text_color(TEXT_SECONDARY).child(truncate(what)))
                }))
                .into_any_element(),
        };
        section(title).meta(div().child(since)).child(body).into_any_element()
    }

    fn inside_section(&mut self, graph: Option<&hub::api::GraphDef>, cx: &mut Context<Self>) -> AnyElement {
        let Some(graph) = graph else {
            return section("Agents inside").child(Note::new("Reading the pipeline…")).into_any_element();
        };
        let agents: Vec<(String, String, Option<RunnableRef>)> = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => {
                    let model = agent.model.as_ref().map(|m| m.short().to_string()).unwrap_or_default();
                    let detail = match (model.is_empty(), agent.variants) {
                        (true, n) if n > 1 => format!("×{n}"),
                        (true, _) => String::new(),
                        (false, n) => format!("{model} · ×{n}"),
                    };
                    Some((node.name.clone(), detail, agent.runnable.clone()))
                }
                _ => None,
            })
            .collect();
        let empty = agents.is_empty();
        let list = agents.into_iter().enumerate().map(|(ix, (name, detail, reference))| {
            row()
                .id(("agents-inside", ix))
                .h(px(32.))
                .gap(px(SPACE_2))
                .when(reference.is_some(), |d| d.cursor_pointer())
                .child(Tile::new(IconName::Agent))
                .child(div().flex_none().type_style(BODY_STRONG).child(name))
                .child(grow())
                .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).font_family(FONT_MONO).child(detail))
                .when_some(reference, |d, reference| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.select(Some(reference.clone()), cx)))
                })
        });
        section("Agents inside")
            .when(empty, |d| d.child(Note::new("No agents in this pipeline yet.")))
            .child(col().children(list))
            .into_any_element()
    }

    fn runs_section(&mut self, id: &PipelineId, cx: &mut Context<Self>) -> AnyElement {
        let runs = hub::runs(cx);
        let open: Vec<_> = runs.read(cx).open_runs().into_iter().filter(|r| r.pipeline == *id && !r.test).take(5).collect();
        let empty = open.is_empty();
        let list = open.into_iter().enumerate().map(|(ix, run)| {
            let target = Target::Run(run.id.clone());
            let label = run.issue.as_ref().map_or_else(|| run.title.clone(), |key| format!("{key} · {}", run.title));
            row()
                .id(("agents-run", ix))
                .h(px(28.))
                .gap(px(SPACE_2))
                .cursor_pointer()
                .child(StatusDot::new(Status::Running).small())
                .child(div().flex_1().min_w_0().child(truncate(label)))
                .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(base::fmt::cost(run.cost_usd)))
                .on_click(move |_, _, cx| hub::open(target.clone(), cx))
        });
        section("Running now")
            .when(empty, |d| d.child(div().w(px(TEXT_W)).child(Note::new("Nothing is running on it."))))
            .child(col().children(list))
            .into_any_element()
    }
}
