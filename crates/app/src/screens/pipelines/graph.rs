//! Graph tab: the node palette, the auto-laid-out canvas with wires, and the inspector column.

use std::collections::HashMap;

use gpui::{Animation, AnimationExt as _, Hsla};
use harness_core::{
    AgentDef, AgentNode, Config, DecisionNode, GateKind, GateNode, Node, NodeKind, PipelineGraph,
    RenderNode, RouterNode, SourceNode, ToolNode,
};

use super::data::NodeStats;
use super::edit::{self, Layout, NodeChange};
use super::PipelinesScreen;
use crate::screens::prelude::*;

const CARD_W: f32 = 272.;
const LANE_GAP: f32 = 48.;
const ROW_GAP: f32 = 64.;
const PAD_X: f32 = 40.;
const PAD_Y: f32 = 32.;
const WIRE: f32 = 2.;
/// Vertical gap between items inside a card.
const ITEM_GAP: f32 = 6.;
const MAX_RULES_SHOWN: usize = 4;

/// Something the palette can add.
#[derive(Clone, Debug)]
enum PaletteItem {
    Source,
    Decision,
    Router,
    Gate,
    Render,
    Agent,
    Command,
    PullRequest,
    MergePr,
    Saved(Id),
}

/// Palette row: icon, icon color, label, hint, what it adds, why it is disabled.
type PaletteEntry = (
    AppIcon,
    Hsla,
    &'static str,
    &'static str,
    PaletteItem,
    Option<&'static str>,
);

/// Icon tile for a node kind: icon, tile background, icon color.
pub(super) fn kind_tile(kind: &NodeKind) -> (AppIcon, Hsla, Hsla) {
    let grey = (theme::SURFACE_HOVER, theme::TEXT_SECONDARY);
    let (icon, (bg, fg)) = match kind {
        NodeKind::Source(_) => (AppIcon::Inbox, (theme::INFO_SOFT, theme::INFO_TEXT)),
        NodeKind::Decision(_) => (AppIcon::GitBranch, grey),
        NodeKind::Router(_) => (AppIcon::GitFork, grey),
        NodeKind::Agent(_) => (AppIcon::Bot, grey),
        NodeKind::Render(_) => (AppIcon::File, grey),
        NodeKind::Gate(_) => (AppIcon::User, (theme::WARNING_SOFT, theme::WARNING)),
        NodeKind::Tool(ToolNode::Command { .. }) => (AppIcon::Terminal, grey),
        NodeKind::Tool(ToolNode::PullRequest { .. }) => (AppIcon::GitPullRequest, grey),
        NodeKind::Tool(ToolNode::MergePr { .. }) => (AppIcon::CircleCheck, grey),
    };
    (icon, bg, fg)
}

/// What a card shows; also decides its height so wires can be placed exactly.
struct CardSpec {
    title: String,
    right: String,
    desc: String,
    desc_mono: bool,
    rules: Vec<(String, String, String)>,
    more_rules: usize,
    variants: Vec<String>,
    stats: NodeStats,
    footer: String,
    change: Option<NodeChange>,
}

impl CardSpec {
    fn has_live(&self) -> bool {
        self.stats.running + self.stats.queued + self.stats.waiting > 0
    }

    fn height(&self) -> f32 {
        let mut items: Vec<f32> = vec![24., 18.];
        items.extend(self.rules.iter().map(|_| 24.));
        if self.more_rules > 0 {
            items.push(18.);
        }
        if !self.variants.is_empty() {
            items.push(24.);
        }
        if self.has_live() {
            items.push(22.);
        }
        if !self.footer.is_empty() {
            items.push(23.);
        }
        2. + 20. + items.iter().sum::<f32>() + ITEM_GAP * (items.len() as f32 - 1.)
    }
}

fn model_label(m: &str) -> String {
    if m.is_empty() {
        String::new()
    } else {
        Config::short_model(m)
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn card_spec(
    n: &Node,
    agents: &[AgentDef],
    stats: Option<&NodeStats>,
    total_runs: usize,
    change: Option<NodeChange>,
) -> CardSpec {
    let stats = stats.cloned().unwrap_or_default();
    let mut s = CardSpec {
        title: n.name.clone(),
        right: String::new(),
        desc: String::new(),
        desc_mono: false,
        rules: vec![],
        more_rules: 0,
        variants: vec![],
        footer: String::new(),
        change,
        stats: NodeStats::default(),
    };
    let mut foot: Vec<String> = Vec::new();
    let perf = |foot: &mut Vec<String>| {
        if let Some(c) = stats.avg_cost() {
            foot.push(format!("avg {}", fmt_usd(c)));
        }
        if let Some(secs) = stats.avg_secs() {
            foot.push(fmt_duration(secs));
        }
        if let Some(p) = stats.picked_pct() {
            foot.push(format!("{p}% picked"));
        }
        if stats.failed > 0 {
            foot.push(format!("{} failed", stats.failed));
        }
    };
    let ports = |foot: &mut Vec<String>| {
        if !stats.ports.is_empty() {
            foot.push(
                stats
                    .ports
                    .iter()
                    .map(|(p, n)| {
                        format!(
                            "{} {n}",
                            edit::port_label(&Some(p.clone())).unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
    };
    match &n.kind {
        NodeKind::Source(src) => {
            s.right = "source".into();
            if src.github_repo.trim().is_empty() {
                s.desc = format!("run input · {}", src.input_name);
            } else {
                s.desc = format!("gh issues · {}", src.github_repo.trim());
            }
            s.desc_mono = true;
            if total_runs > 0 {
                foot.push(format!(
                    "{total_runs} run{}",
                    if total_runs == 1 { "" } else { "s" }
                ));
            }
        }
        NodeKind::Decision(d) => {
            s.right = model_label(&d.model);
            s.desc = if d.question.trim().is_empty() {
                "No question yet".into()
            } else {
                first_line(&d.question)
            };
            ports(&mut foot);
            perf(&mut foot);
        }
        NodeKind::Router(r) => {
            s.right = format!(
                "{} rule{}",
                r.rules.len(),
                if r.rules.len() == 1 { "" } else { "s" }
            );
            s.desc = if r.rules.is_empty() {
                "No rules: everything goes to else".into()
            } else {
                "first match wins, else otherwise".into()
            };
            s.rules = r
                .rules
                .iter()
                .take(MAX_RULES_SHOWN)
                .map(|r| {
                    (
                        r.field.clone(),
                        rule_op_label(r.op).to_string(),
                        r.value.clone(),
                    )
                })
                .collect();
            s.more_rules = r.rules.len().saturating_sub(MAX_RULES_SHOWN);
            ports(&mut foot);
        }
        NodeKind::Agent(a) => {
            let def = a
                .agent
                .as_deref()
                .and_then(|id| agents.iter().find(|d| d.id == id));
            let model = if !a.model.is_empty() {
                a.model.clone()
            } else {
                def.map(|d| d.model.clone()).unwrap_or_default()
            };
            s.right = if model.is_empty() {
                "default model".into()
            } else {
                model_label(&model)
            };
            if a.variants > 1 {
                s.title = format!("{} ×{}", n.name, a.variants);
                s.variants = (0..a.variants.min(6))
                    .map(|i| harness_core::variant_letter(i).to_string())
                    .collect();
            }
            s.desc = if !a.instructions.trim().is_empty() {
                first_line(&a.instructions)
            } else if let Some(d) = def.filter(|d| !d.description.is_empty()) {
                d.description.clone()
            } else {
                "No step instructions yet".into()
            };
            perf(&mut foot);
        }
        NodeKind::Render(r) => {
            s.right = "render".into();
            s.desc = format!("{} · {}×{}", r.files, r.width, r.height);
            s.desc_mono = true;
            perf(&mut foot);
        }
        NodeKind::Gate(g) => {
            s.right = "you".into();
            s.desc = if !g.prompt.trim().is_empty() {
                first_line(&g.prompt)
            } else {
                match g.kind {
                    GateKind::Pick => "Pick a variant, or send back with a note".into(),
                    GateKind::ApprovePr => "Approve the pull request; approving merges it".into(),
                }
            };
            if stats.done > 0 {
                foot.push(format!("{} decided", stats.done));
            }
        }
        NodeKind::Tool(t) => {
            s.right = "tool".into();
            match t {
                ToolNode::Command { command, .. } => {
                    s.desc = if command.trim().is_empty() {
                        "No command yet".into()
                    } else {
                        first_line(command)
                    };
                    s.desc_mono = !command.trim().is_empty();
                }
                ToolNode::PullRequest { draft, .. } => {
                    s.desc = if *draft {
                        "Open a draft pull request".into()
                    } else {
                        "Open a pull request".into()
                    }
                }
                ToolNode::MergePr { method } => {
                    s.desc = format!("gh pr merge --{method}");
                    s.desc_mono = true;
                }
            }
            perf(&mut foot);
        }
    }
    s.footer = foot.join(" · ");
    s.stats = stats;
    s
}

pub(super) fn rule_op_label(op: harness_core::RuleOp) -> &'static str {
    match op {
        harness_core::RuleOp::Contains => "contains",
        harness_core::RuleOp::Equals => "is",
        harness_core::RuleOp::StartsWith => "starts with",
        harness_core::RuleOp::Matches => "matches",
    }
}

/// Pixel geometry of a laid-out graph.
struct Geo {
    row_y: Vec<f32>,
    row_h: Vec<f32>,
    width: f32,
    height: f32,
}

impl Geo {
    fn new(layout: &Layout, heights: &HashMap<Id, f32>) -> Geo {
        let mut row_h = vec![0f32; layout.rows];
        for (id, cell) in &layout.pos {
            row_h[cell.row] = row_h[cell.row].max(heights.get(id).copied().unwrap_or(80.));
        }
        let mut row_y = Vec::with_capacity(layout.rows);
        let mut y = PAD_Y;
        for h in &row_h {
            row_y.push(y);
            y += h + ROW_GAP;
        }
        let width =
            PAD_X * 2. + layout.lanes as f32 * (CARD_W + LANE_GAP) - LANE_GAP + LANE_GAP / 2.;
        Geo {
            row_y,
            row_h,
            width,
            height: y - ROW_GAP + PAD_Y + 8.,
        }
    }
    fn lane_x(&self, lane: usize) -> f32 {
        PAD_X + lane as f32 * (CARD_W + LANE_GAP)
    }
    fn center(&self, lane: usize) -> f32 {
        self.lane_x(lane) + CARD_W / 2.
    }
    fn gap_after(&self, row: usize) -> f32 {
        self.row_y[row] + self.row_h[row] + ROW_GAP / 2.
    }
    fn gap_before(&self, row: usize) -> f32 {
        self.row_y[row] - ROW_GAP / 2.
    }
    fn gutter(&self, lane: usize) -> f32 {
        self.lane_x(lane) + CARD_W + LANE_GAP / 2.
    }
}

/// A straight wire piece between two points (horizontal or vertical).
fn segment(a: (f32, f32), b: (f32, f32), color: Hsla) -> Div {
    let (x1, x2) = (a.0.min(b.0), a.0.max(b.0));
    let (y1, y2) = (a.1.min(b.1), a.1.max(b.1));
    div()
        .absolute()
        .left(px(x1 - WIRE / 2.))
        .top(px(y1 - WIRE / 2.))
        .w(px(x2 - x1 + WIRE))
        .h(px(y2 - y1 + WIRE))
        .rounded(px(1.))
        .bg(color)
}

impl PipelinesScreen {
    fn add_from_palette(&mut self, item: PaletteItem, cx: &mut Context<Self>) {
        let default_model = self.catalog.default_model.clone();
        let saved = match &item {
            PaletteItem::Saved(id) => self
                .detail
                .as_ref()
                .and_then(|d| d.agents.iter().find(|a| &a.id == id))
                .map(|a| (a.id.clone(), a.name.clone())),
            _ => None,
        };
        let Some(ed) = self.ed.as_mut() else { return };
        let wd = ed.graph.workdirs.first().map(|w| w.id.clone());
        let (name, kind): (String, NodeKind) = match item {
            PaletteItem::Source => (
                "Source".into(),
                NodeKind::Source(SourceNode {
                    input_name: "issue".into(),
                    github_repo: String::new(),
                }),
            ),
            PaletteItem::Decision => (
                "Decision".into(),
                NodeKind::Decision(DecisionNode {
                    question: String::new(),
                    model: default_model,
                    workdir: None,
                    max_steps: 12,
                }),
            ),
            PaletteItem::Router => ("Router".into(), NodeKind::Router(RouterNode::default())),
            PaletteItem::Gate => ("Gate".into(), NodeKind::Gate(GateNode::default())),
            PaletteItem::Render => (
                "Render".into(),
                NodeKind::Render(RenderNode {
                    workdir: wd,
                    ..Default::default()
                }),
            ),
            PaletteItem::Agent => (
                "Agent step".into(),
                NodeKind::Agent(AgentNode {
                    workdir: wd,
                    ..Default::default()
                }),
            ),
            PaletteItem::Command => (
                "Command".into(),
                NodeKind::Tool(ToolNode::Command {
                    workdir: wd,
                    command: String::new(),
                    allow_failure: false,
                }),
            ),
            PaletteItem::PullRequest => (
                "Open PR".into(),
                NodeKind::Tool(ToolNode::PullRequest {
                    workdir: wd,
                    draft: false,
                }),
            ),
            PaletteItem::MergePr => (
                "Merge PR".into(),
                NodeKind::Tool(ToolNode::MergePr {
                    method: "squash".into(),
                }),
            ),
            PaletteItem::Saved(_) => {
                let Some((id, name)) = saved else { return };
                (
                    name,
                    NodeKind::Agent(AgentNode {
                        agent: Some(id),
                        workdir: wd,
                        ..Default::default()
                    }),
                )
            }
        };
        let id = edit::add_node(&mut ed.graph, &name, kind, ed.selected.as_deref());
        ed.selected = Some(id);
        ed.sel_seq += 1;
        ed.rev += 1;
        self.touch(cx);
    }

    pub(super) fn render_graph_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_inspector(window, cx);
        let palette = self.render_palette(cx);
        let canvas = self.render_canvas(cx);
        let inspector = self.render_inspector(cx);
        div()
            .size_full()
            .flex()
            .min_h_0()
            .child(palette)
            .child(canvas)
            .children(inspector)
            .into_any_element()
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let q = self.query.clone();
        let matches = |label: &str, hint: &str| {
            q.is_empty() || label.to_lowercase().contains(&q) || hint.to_lowercase().contains(&q)
        };
        let has_source = self.ed.as_ref().is_some_and(|ed| {
            ed.graph
                .nodes
                .iter()
                .any(|n| matches!(n.kind, NodeKind::Source(_)))
        });

        let item =
            |ix: usize,
             icon: AppIcon,
             icon_color: Hsla,
             label: String,
             hint: String,
             it: PaletteItem,
             disabled: Option<&'static str>,
             cx: &mut Context<Self>| {
                div()
                    .id(("pl-pal", ix))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(32.))
                    .px(theme::S2)
                    .rounded(theme::RADIUS_MD)
                    .border_1()
                    .border_color(theme::TRANSPARENT)
                    .text_body()
                    .fg_secondary()
                    .child(icon_sized(icon, theme::ICON_SM).text_color(icon_color))
                    .child(div().flex_1().min_w_0().ellipsis().child(label))
                    .child(div().text_mono_sm().fg_muted().child(hint))
                    .when_some(disabled, |el, why| {
                        el.opacity(0.45)
                            .cursor_not_allowed()
                            .tooltip(move |window, cx| {
                                gpui_component::tooltip::Tooltip::new(why).build(window, cx)
                            })
                    })
                    .when(disabled.is_none(), |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(theme::SURFACE_RAISED).border_color(theme::LINE))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_from_palette(it.clone(), cx)
                            }))
                    })
                    .into_any_element()
            };
        let section = |title: &str| {
            div()
                .mt(theme::S3)
                .mb(theme::S1)
                .px(theme::S2)
                .text_size(px(11.))
                .line_height(px(16.))
                .fg_muted()
                .child(title.to_uppercase())
                .into_any_element()
        };

        let muted = theme::TEXT_MUTED;
        let flow: Vec<PaletteEntry> = vec![
            (
                AppIcon::Inbox,
                muted,
                "Source",
                "input",
                PaletteItem::Source,
                has_source.then_some("A pipeline has exactly one Source"),
            ),
            (
                AppIcon::GitBranch,
                muted,
                "Decision",
                "yes / no",
                PaletteItem::Decision,
                None,
            ),
            (
                AppIcon::GitFork,
                muted,
                "Router",
                "rules",
                PaletteItem::Router,
                None,
            ),
            (
                AppIcon::User,
                theme::WARNING,
                "Gate",
                "you decide",
                PaletteItem::Gate,
                None,
            ),
            (
                AppIcon::File,
                muted,
                "Render",
                "html→png",
                PaletteItem::Render,
                None,
            ),
        ];
        let steps: Vec<PaletteEntry> = vec![
            (
                AppIcon::Bot,
                muted,
                "Agent step",
                "",
                PaletteItem::Agent,
                None,
            ),
            (
                AppIcon::Terminal,
                muted,
                "Command",
                "shell",
                PaletteItem::Command,
                None,
            ),
            (
                AppIcon::GitPullRequest,
                muted,
                "Pull request",
                "gh pr",
                PaletteItem::PullRequest,
                None,
            ),
            (
                AppIcon::CircleCheck,
                muted,
                "Merge PR",
                "gh merge",
                PaletteItem::MergePr,
                None,
            ),
        ];
        let mut children: Vec<AnyElement> =
            vec![ui::input::field_sm(&self.search).into_any_element()];
        let mut ix = 0;
        for (title, list) in [("Flow", flow), ("Steps", steps)] {
            let shown: Vec<_> = list
                .into_iter()
                .filter(|(_, _, l, h, _, _)| matches(l, h))
                .collect();
            if shown.is_empty() {
                continue;
            }
            children.push(section(title));
            for (icon, color, label, hint, it, dis) in shown {
                children.push(item(
                    ix,
                    icon,
                    color,
                    label.to_string(),
                    hint.to_string(),
                    it,
                    dis,
                    cx,
                ));
                ix += 1;
            }
        }
        let agents: Vec<&AgentDef> = self
            .detail
            .as_ref()
            .map(|d| {
                d.agents
                    .iter()
                    .filter(|a| matches(&a.name, &a.model))
                    .collect()
            })
            .unwrap_or_default();
        let total_agents = self.detail.as_ref().map(|d| d.agents.len()).unwrap_or(0);
        if !agents.is_empty() || (q.is_empty() && total_agents == 0) {
            children.push(section("Agents"));
        }
        for a in agents {
            children.push(item(
                ix,
                AppIcon::Bot,
                theme::ACCENT_TEXT,
                a.name.clone(),
                model_label(&a.model),
                PaletteItem::Saved(a.id.clone()),
                None,
                cx,
            ));
            ix += 1;
        }
        if q.is_empty() && total_agents == 0 {
            children.push(
                div()
                    .px(theme::S2)
                    .text_label()
                    .fg_muted()
                    .child("No saved agents yet.")
                    .into_any_element(),
            );
        }
        if ix == 0 && !q.is_empty() {
            children.push(
                div()
                    .px(theme::S2)
                    .pt(theme::S3)
                    .text_label()
                    .fg_muted()
                    .child("Nothing matches.")
                    .into_any_element(),
            );
        }
        children.push(
            div()
                .id("pl-pal-new-agent")
                .flex()
                .items_center()
                .gap(theme::S1)
                .mt(theme::S3)
                .px(theme::S2)
                .text_label()
                .fg_accent()
                .cursor_pointer()
                .hover(|s| s.text_color(theme::ACCENT))
                .child(icon_sized(AppIcon::Plus, px(12.)))
                .child("Manage agents")
                .on_click(|_, _, cx| state::navigate(Route::Agents { agent: None }, cx))
                .into_any_element(),
        );
        children.push(
            div()
                .px(theme::S2)
                .pt(theme::S4)
                .text_label()
                .fg_muted()
                .child(
                    "Click to add below the selected node (it takes over its wire), or at the end.",
                )
                .into_any_element(),
        );
        div()
            .id("pl-palette")
            .w(px(220.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(theme::S3)
            .py(theme::S4)
            .border_r_1()
            .border_color(theme::LINE)
            .children(children)
            .into_any_element()
    }

    fn render_canvas(&self, cx: &mut Context<Self>) -> AnyElement {
        let (Some(d), Some(ed)) = (&self.detail, &self.ed) else {
            return div().flex_1().into_any_element();
        };
        let g: &PipelineGraph = &ed.graph;
        let deselect = cx.listener(|this, _, _, cx| this.select_node(None, cx));
        if g.nodes.is_empty() {
            return div()
                .id("pl-canvas")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    EmptyState::new(
                        AppIcon::Workflow,
                        "Empty graph",
                        "Add a Source from the palette, then the steps that follow it.",
                    )
                    .compact(),
                )
                .into_any_element();
        }
        let layout = edit::layout(g);
        let changes = if ed.graph != d.pipeline.graph {
            edit::node_changes(&d.pipeline.graph, g)
        } else {
            HashMap::new()
        };
        let specs: HashMap<Id, CardSpec> = g
            .nodes
            .iter()
            .map(|n| {
                (
                    n.id.clone(),
                    card_spec(
                        n,
                        &d.agents,
                        d.stats.get(&n.id),
                        d.total_runs,
                        changes.get(&n.id).copied(),
                    ),
                )
            })
            .collect();
        let heights: HashMap<Id, f32> = specs
            .iter()
            .map(|(id, s)| (id.clone(), s.height()))
            .collect();
        let geo = Geo::new(&layout, &heights);
        let selected = ed.selected.clone();

        // Wires and their port labels.
        let mut wires: Vec<AnyElement> = Vec::new();
        let mut labels: Vec<AnyElement> = Vec::new();
        for (ix, e) in g.edges.iter().enumerate() {
            let (Some(pc), Some(cc)) = (layout.pos.get(&e.from), layout.pos.get(&e.to)) else {
                continue;
            };
            let hot = selected
                .as_deref()
                .is_some_and(|s| s == e.from || s == e.to);
            let color = if hot {
                theme::ACCENT_RING
            } else {
                theme::LINE_STRONG
            };
            let px0 = geo.center(pc.lane);
            let py0 = geo.row_y[pc.row] + heights.get(&e.from).copied().unwrap_or(0.);
            let py1 = geo.gap_after(pc.row);
            let cx0 = geo.center(cc.lane);
            let cy0 = geo.row_y[cc.row];
            let blocked = cc.row > pc.row + 1
                && layout
                    .pos
                    .values()
                    .any(|c| c.lane == cc.lane && c.row > pc.row && c.row < cc.row);
            let points: Vec<(f32, f32)> = if cc.row > pc.row && !blocked {
                vec![(px0, py0), (px0, py1), (cx0, py1), (cx0, cy0)]
            } else {
                let gx = geo.gutter(pc.lane.max(cc.lane));
                let cy1 = geo.gap_before(cc.row);
                vec![
                    (px0, py0),
                    (px0, py1),
                    (gx, py1),
                    (gx, cy1),
                    (cx0, cy1),
                    (cx0, cy0),
                ]
            };
            for w in points.windows(2) {
                if w[0] != w[1] {
                    wires.push(segment(w[0], w[1], color).into_any_element());
                }
            }
            // Arrival dot on the child's top edge.
            wires.push(
                div()
                    .absolute()
                    .left(px(cx0 - 3.))
                    .top(px(cy0 - 4.))
                    .size(px(6.))
                    .rounded(theme::RADIUS_FULL)
                    .bg(color)
                    .into_any_element(),
            );
            if let Some(label) = edit::port_label(&e.port) {
                let lx = points[2].0;
                let text = match (&g.node(&e.from).map(|n| &n.kind), e.port.as_deref()) {
                    (Some(NodeKind::Router(r)), Some(p)) if p.starts_with("rule") => {
                        let i: usize = p[4..].parse().unwrap_or(0);
                        match r.rules.get(i.wrapping_sub(1)) {
                            Some(rule) if !rule.value.is_empty() => {
                                format!(
                                    "{label} · {} {} {}",
                                    rule.field,
                                    rule_op_label(rule.op),
                                    rule.value
                                )
                            }
                            _ => label,
                        }
                    }
                    _ => label,
                };
                labels.push(
                    div()
                        .id(("pl-wire-label", ix))
                        .absolute()
                        .left(px(lx + 6.))
                        .top(px(py1 + 3.))
                        .max_w(px(CARD_W - 16.))
                        .ellipsis()
                        .text_mono_sm()
                        .text_color(if hot {
                            theme::ACCENT_TEXT
                        } else {
                            theme::TEXT_MUTED
                        })
                        .child(text)
                        .into_any_element(),
                );
            }
        }

        let sel_seq = ed.sel_seq;
        let cards: Vec<AnyElement> = g
            .nodes
            .iter()
            .filter_map(|n| {
                let cell = layout.pos.get(&n.id)?;
                let spec = specs.get(&n.id)?;
                let x = geo.lane_x(cell.lane);
                let y = geo.row_y[cell.row];
                let is_sel = selected.as_deref() == Some(n.id.as_str());
                Some(self.render_card(n, spec, x, y, is_sel, sel_seq, cx))
            })
            .collect();

        div()
            .id("pl-canvas")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_scroll()
            .bg(theme::BG)
            .on_click(deselect)
            .child(
                div().flex().min_w(relative(1.)).child(
                    div()
                        .relative()
                        .flex_none()
                        .mx_auto()
                        .w(px(geo.width))
                        .h(px(geo.height))
                        .children(wires)
                        .children(labels)
                        .children(cards),
                ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        &self,
        n: &Node,
        s: &CardSpec,
        x: f32,
        y: f32,
        selected: bool,
        sel_seq: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = n.id.clone();
        let (icon, tile_bg, tile_fg) = kind_tile(&n.kind);
        let (tile_bg, tile_fg) = if selected && tile_bg == theme::SURFACE_HOVER {
            (theme::ACCENT_SOFT, theme::ACCENT_TEXT)
        } else {
            (tile_bg, tile_fg)
        };
        let h = s.height();
        let border = if selected {
            theme::ACCENT_RING
        } else if s.change.is_some() {
            theme::ACCENT_STRONG
        } else {
            theme::LINE_STRONG
        };
        let key = |what: &str| SharedString::from(format!("pl-card-{what}-{}", n.id));

        let mut body = div()
            .id(ElementId::Name(key("box")))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(ITEM_GAP))
            .px(theme::S3)
            .py(px(10.))
            .rounded(theme::RADIUS_XL)
            .border_1()
            .border_color(border)
            .bg(if selected {
                theme::SURFACE_RAISED
            } else {
                theme::SURFACE
            })
            .overflow_hidden()
            .text_size(px(12.))
            .line_height(px(18.))
            .fg_muted()
            .cursor_pointer()
            .when(!selected, |el| {
                el.hover(|st| st.border_color(theme::TEXT_DISABLED))
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(theme::S2)
                    .h(px(24.))
                    .min_w_0()
                    .child(icon_tile(icon, tile_bg, tile_fg))
                    .child(
                        div()
                            .min_w_0()
                            .ellipsis()
                            .text_body_strong()
                            .fg()
                            .child(s.title.clone()),
                    )
                    .when_some(s.change, |el, c| {
                        el.child(
                            Badge::new(if c == NodeChange::Added {
                                "new"
                            } else {
                                "edited"
                            })
                            .accent(),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex_none()
                            .text_label()
                            .fg_muted()
                            .child(s.right.clone()),
                    ),
            )
            .child(
                div()
                    .h(px(18.))
                    .flex_none()
                    .ellipsis()
                    .when(s.desc_mono, |el| {
                        el.text_mono_sm().fg_secondary().line_height(px(18.))
                    })
                    .child(s.desc.clone()),
            );
        for (i, (field, op, value)) in s.rules.iter().enumerate() {
            body = body.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(24.))
                    .child(
                        div()
                            .w(px(12.))
                            .text_mono_sm()
                            .fg_muted()
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .h(px(24.))
                            .px(theme::S2)
                            .min_w_0()
                            .rounded(theme::RADIUS_MD)
                            .border_1()
                            .border_color(theme::LINE_STRONG)
                            .bg(theme::SURFACE_RAISED)
                            .overflow_hidden()
                            .child(div().fg_muted().child(field.clone()))
                            .child(div().fg_secondary().child(op.clone()))
                            .child(
                                div()
                                    .ellipsis()
                                    .fg()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(if value.is_empty() {
                                        "…".to_string()
                                    } else {
                                        value.clone()
                                    }),
                            ),
                    ),
            );
        }
        if s.more_rules > 0 {
            body = body.child(
                div()
                    .h(px(18.))
                    .flex_none()
                    .child(format!("+{} more", s.more_rules)),
            );
        }
        if !s.variants.is_empty() {
            body = body.child(div().flex().flex_none().gap(px(6.)).h(px(24.)).children(
                s.variants.iter().map(|v| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(24.))
                        .rounded(theme::RADIUS_MD)
                        .border_1()
                        .border_color(theme::LINE)
                        .bg(theme::BG_SIDEBAR)
                        .text_size(px(11.))
                        .fg_secondary()
                        .child(format!("{} {v}", n.name))
                        .ellipsis()
                }),
            ));
        }
        if s.has_live() {
            let st = &s.stats;
            let run = st
                .active_run
                .map(|r| format!(" · #{r}"))
                .unwrap_or_default();
            body = body.child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(6.))
                    .h(px(22.))
                    .when(st.running > 0, |el| {
                        el.child(
                            Badge::new(format!("{} running{run}", st.running)).leading(
                                StatusDot::new(Status::Running)
                                    .id(ElementId::Name(key("run")))
                                    .small(),
                            ),
                        )
                    })
                    .when(st.waiting > 0, |el| {
                        let since = st
                            .waiting_since
                            .map(|t| format!(" · {}", fmt_ago(t)))
                            .unwrap_or_default();
                        el.child(
                            Badge::new(format!("{} waiting{since}", st.waiting))
                                .warning()
                                .dot(),
                        )
                    })
                    .when(st.queued > 0, |el| {
                        el.child(
                            Badge::new(format!("{} queued", st.queued))
                                .leading(StatusDot::new(Status::Queued).small()),
                        )
                    }),
            );
        }
        if !s.footer.is_empty() {
            body = body.child(
                div()
                    .flex_none()
                    .h(px(23.))
                    .pt(px(6.))
                    .border_t_1()
                    .border_color(theme::LINE_SUBTLE)
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .ellipsis()
                    .child(s.footer.clone()),
            );
        }
        let body = body.on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.select_node(Some(id.clone()), cx);
        }));
        // Selected: the accent ring grows in.
        let body: AnyElement = if selected {
            body.with_animation(
                ElementId::NamedInteger(key("ring"), sel_seq),
                Animation::new(theme::DUR_STATE).with_easing(theme::ease_out),
                |el, t| {
                    el.shadow(vec![gpui::BoxShadow {
                        color: theme::ACCENT_GLOW,
                        offset: gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(3. * t),
                    }])
                },
            )
            .into_any_element()
        } else {
            body.into_any_element()
        };
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(CARD_W))
            .h(px(h))
            .child(body)
            .with_animation(
                ElementId::Name(key("in")),
                Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                move |el, t| el.opacity(t).top(px(y + 8. * (1. - t))),
            )
            .into_any_element()
    }
}
