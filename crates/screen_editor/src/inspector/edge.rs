//! The selected edge: where it goes from and to, which port it leaves by, its kind.

use std::rc::Rc;

use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use hub::model::{EdgeKind, GraphDef};
use ui::dev::Probe;
use ui::prelude::*;

use super::form::{INNER, OnPick, Pick, field, note, psec};
use super::{InspectorView, ops};

/// The widest a node id is drawn beside its name before it ends in `…`, px.
const EDGE_ID_MAX: f32 = 120.0;

/// What each edge kind means, in the order of the kind control.
const KINDS: [(EdgeKind, &str, &str); 4] = [
    (EdgeKind::Route, "Route", "The item moves along it."),
    (EdgeKind::Context, "Context", "A file node feeding an agent."),
    (EdgeKind::Spawn, "Spawn", "An agent and a sub-agent template it may create."),
    (EdgeKind::Back, "Back", "A review sending work back upstream; the only edge that may close a cycle."),
];

impl InspectorView {
    pub(super) fn edge_view(&mut self, graph: &GraphDef, index: usize, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let Some(edge) = graph.edges.get(index).cloned() else {
            return self.pipeline_view(graph, window, cx);
        };
        let from = graph.node(&edge.from);
        let to = graph.node(&edge.to);
        let name = |n: Option<&hub::model::NodeDef>, id: &hub::model::NodeId| n.map(|n| n.name.clone()).unwrap_or_else(|| format!("{id} · missing"));
        let title = format!("{} → {}", name(from, &edge.from), name(to, &edge.to));
        let header = self.header(IconName::ArrowRight, TileTone::Neutral, title, "Edge".into(), true, window, cx);

        let ports = from.map(ops::out_ports).unwrap_or_default();
        let mut ports_all = ports.clone();
        if !ports_all.contains(&edge.port) {
            ports_all.push(edge.port.clone());
        }
        let picks: Vec<Pick> = ports_all
            .iter()
            .map(|p| Pick::new(from.map(|n| ops::port_label(n, p)).unwrap_or_else(|| p.clone())).checked(*p == edge.port))
            .collect();
        let value = from.map(|n| ops::port_label(n, &edge.port)).unwrap_or_else(|| edge.port.clone());
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let Some(port) = ports_all.get(ix).cloned() else { return };
            this.edit(
                "Change port",
                move |g| {
                    if let Some(edge) = g.edges.get_mut(index) {
                        edge.port = port;
                    }
                },
                cx,
            );
        });
        let port = self.picker("insp-edge-port", value, false, None, picks, on_pick, window, cx);
        let kind_ix = KINDS.iter().position(|(k, ..)| *k == edge.kind).unwrap_or(0);
        let mut kinds = Segmented::new("insp-edge-kind").small().full_width().selected(kind_ix);
        for (_, label, _) in KINDS {
            kinds = kinds.option(label);
        }
        let kinds = kinds.on_select(self.on_value(cx, move |this, ix: usize, cx| {
            let Some((kind, ..)) = KINDS.get(ix) else { return };
            let kind = *kind;
            this.edit(
                "Change edge kind",
                move |g| {
                    if let Some(edge) = g.edges.get_mut(index) {
                        edge.kind = kind;
                    }
                },
                cx,
            );
        }));
        let node_row = |label: &'static str, n: Option<&hub::model::NodeDef>, id: &hub::model::NodeId| {
            row()
                .gap(px(SPACE_2))
                .min_h(px(CONTROL_HEIGHT_MD))
                .child(div().w(px(48.)).flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(label))
                .children(n.map(|n| Tile::new(super::form::kind_icon(&n.kind)).tone(super::form::kind_tone(&n.kind))))
                .child(div().flex_shrink().min_w_0().child(truncate(name(n, id))))
                .child(div().flex_shrink().min_w_0().max_w(px(EDGE_ID_MAX)).type_style(MONO_SM).text_color(TEXT_MUTED).child(truncate_lh(id.to_string(), MONO_SM.line)))
        };
        let problems: Vec<_> = self
            .problems_of(Some(&edge.from), cx)
            .into_iter()
            .filter(|(_, m)| m.contains("edge") || m.contains("connects") || m.contains("Context edges") || m.contains("Spawn edges") || m.contains("send work back"))
            .collect();
        let mut sections = Vec::new();
        if let Some(list) = self.problem_list(problems, cx) {
            sections.push(list);
        }
        sections.push(
            psec()
                .gap(px(SPACE_1))
                .child(node_row("From", from, &edge.from).probe("insp-edge-from"))
                .child(node_row("To", to, &edge.to).probe("insp-edge-to"))
                .into_any_element(),
        );
        sections.push(
            psec()
                .gap(px(SPACE_3))
                .child(field("Leaves by port", port))
                .child(field("Kind", div().child(kinds).probe("insp-edge-kind")))
                .child(note(KINDS[kind_ix].2, INNER))
                .into_any_element(),
        );
        sections.push(self.remove_section("Remove edge", cx));
        (header, sections)
    }
}
