//! Model, tools, skills and spawn policy: the parts an agent node shares with a sub-agent
//! template. An agent node may leave each of them to its library agent (`None`); the first
//! change copies the inherited value into the node and changes that.

use std::rc::Rc;

use gpui::{AnyElement, App, Context, SharedString, WeakEntity, Window, div, prelude::*, px};
use hub::model::{GraphDef, ModelRef, NodeDef, NodeKind, SkillPolicy, SpawnPolicy, ToolAccess, ToolGrants};
use ui::dev::Probe;
use ui::prelude::*;

use super::fields::{Spec, on_node, parse_usd, usd_text, usd_value};
use super::form::{INNER, OnPick, Pick, note, removable_tag, row_field, tag_row, titled};
use super::{InspectorView, ops};

/// The built-in tools of an access level, as the agent sees them.
fn level_tools(access: ToolAccess) -> &'static [&'static str] {
    match access {
        ToolAccess::ReadOnly => &["read_file", "list_dir", "grep"],
        ToolAccess::ReadAndRun => &["read_file", "list_dir", "grep", "bash"],
        ToolAccess::Full => &["read_file", "list_dir", "grep", "bash", "edit_file", "write_file"],
    }
}

const LEVELS: [ToolAccess; 3] = [ToolAccess::ReadOnly, ToolAccess::ReadAndRun, ToolAccess::Full];

/// Where a grants section writes: the agent node's override, or the sub-agent's own value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Agent,
    Subagent,
}

/// Changes the tools of the node on show, starting from `inherited` when it has none.
fn with_tools(node: &mut NodeDef, inherited: &ToolGrants, f: impl FnOnce(&mut ToolGrants)) {
    match &mut node.kind {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => f(agent.tools.get_or_insert_with(|| inherited.clone())),
        NodeKind::Subagent(sub) => f(&mut sub.tools),
        _ => {}
    }
}

/// Changes the skills of the node on show, starting from `inherited` when it has none.
fn with_skills(node: &mut NodeDef, inherited: &SkillPolicy, f: impl FnOnce(&mut SkillPolicy)) {
    match &mut node.kind {
        NodeKind::Agent(agent) | NodeKind::Orchestrator(agent) => f(agent.skills.get_or_insert_with(|| inherited.clone())),
        NodeKind::Subagent(sub) => f(&mut sub.skills),
        _ => {}
    }
}

/// Opens a menu of `picks` under the element anchored as `key`.
pub(crate) fn open_picks(key: SharedString, picks: Vec<Pick>, on_pick: OnPick, view: WeakEntity<InspectorView>, window: &mut Window, cx: &mut App) {
    let anchor = Anchor::of(key.clone(), window, cx);
    let picks = Rc::new(picks);
    ui::overlays(window, cx).open_menu(
        anchor,
        Placement::below().end(),
        move |_, _| {
            let mut menu = Menu::new(SharedString::from(format!("{key}-menu"))).width(INNER).max_height(420.);
            for (ix, pick) in picks.iter().enumerate() {
                if let Some(group) = &pick.group {
                    if ix > 0 {
                        menu = menu.separator();
                    }
                    menu = menu.label(group.clone());
                }
                let mut item = MenuItem::new(SharedString::from(format!("{key}-{ix}")), pick.label.clone()).check_column().checked(pick.checked);
                if let Some(sub) = &pick.sub {
                    item = item.subline(sub.clone());
                }
                if let Some(hint) = &pick.hint {
                    item = item.hint(hint.clone());
                }
                if let Some(icon) = pick.icon {
                    item = item.icon(icon);
                }
                if pick.soon {
                    item = item.coming_soon();
                } else {
                    let (view, on_pick) = (view.clone(), on_pick.clone());
                    item = item.on_click(move |_, window, cx| {
                        view.update(cx, |this, cx| on_pick(this, ix, window, cx)).ok();
                    });
                }
                menu = menu.item(item);
            }
            menu
        },
        window,
        cx,
    );
}

impl InspectorView {
    /// Every model the providers offer, as `provider/model`, grouped by provider.
    fn model_picks(&self, current: &ModelRef, inherit: Option<(&str, &ModelRef)>) -> (Vec<Pick>, Vec<Option<ModelRef>>) {
        let mut picks = Vec::new();
        let mut values = Vec::new();
        if let Some((label, inherited)) = inherit {
            let shown = if inherited.is_empty() { "the default".to_string() } else { inherited.short().to_string() };
            picks.push(Pick::new(format!("{label} · {shown}")).checked(current.is_empty()));
            values.push(None);
        }
        for provider in &self.models {
            for (ix, model) in provider.models.iter().enumerate() {
                let reference = ModelRef::new(provider.provider.id.clone(), model.clone());
                let mut pick = Pick::new(model.clone()).checked(!current.is_empty() && reference == *current);
                if ix == 0 {
                    pick = pick.group(provider.provider.id.clone());
                }
                picks.push(pick);
                values.push(Some(reference));
            }
        }
        (picks, values)
    }

    /// The model of the node on show. `current` is its own value (empty: inherited or the
    /// default); `inherited` what it falls back to. The mockup's segmented control shows the
    /// models of the current provider when there are two or three; "More" lists them all.
    #[expect(clippy::too_many_arguments, reason = "a model field is its value, its fallback and where it writes")]
    pub(crate) fn model_field(
        &mut self,
        key: &str,
        current: ModelRef,
        inherited: Option<ModelRef>,
        set: super::fields::SetModel,
        problem: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let effective = if current.is_empty() { inherited.clone().unwrap_or_default() } else { current.clone() };
        let inherit_label = if inherited.is_some() { "Library agent's" } else { "Default model" };
        let (picks, values) = self.model_picks(&current, Some((inherit_label, inherited.as_ref().unwrap_or(&ModelRef::default()))));
        let pick_set = set.clone();
        let inherited_for_pick = inherited.clone();
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let value = values.get(ix).cloned().flatten().filter(|m| Some(m) != inherited_for_pick.as_ref());
            let set = pick_set.clone();
            this.edit_node("Set model", move |node| set(node, value), cx);
        });
        let provider = self.models.iter().find(|p| p.provider.id == effective.provider);
        let quick = provider.filter(|p| (2..=3).contains(&p.models.len()));
        let control = match quick {
            Some(provider) => {
                let models = provider.models.clone();
                let provider_id = provider.provider.id.clone();
                let selected = models.iter().position(|m| *m == effective.model).unwrap_or(models.len());
                let mut seg = Segmented::new(SharedString::from(key.to_string())).small().full_width().selected(selected);
                for (ix, model) in models.iter().enumerate() {
                    seg = seg.option(model.clone()).option_probe(format!("{key}-{ix}"));
                }
                let inherited = inherited.clone();
                let seg = seg.on_select(self.on_value(cx, move |this, ix: usize, cx| {
                    let Some(model) = models.get(ix) else { return };
                    let value = Some(ModelRef::new(provider_id.clone(), model.clone())).filter(|m| Some(m) != inherited.as_ref());
                    let set = set.clone();
                    this.edit_node("Set model", move |node| set(node, value), cx);
                }));
                let more_key: SharedString = format!("{key}-more").into();
                let view = cx.entity().downgrade();
                let k = more_key.clone();
                let more = IconButton::new(more_key.clone(), IconName::More)
                    .small()
                    .tooltip("Other models")
                    .on_click(move |_, window, cx| open_picks(k.clone(), picks.clone(), on_pick.clone(), view.clone(), window, cx));
                row()
                    .gap(px(SPACE_1))
                    .child(div().flex_1().min_w_0().child(seg).probe(SharedString::from(key.to_string())))
                    .child(more.popover_anchor(more_key.clone()).probe(more_key))
                    .into_any_element()
            }
            None => {
                let value = if effective.is_empty() { "Default model".to_string() } else { effective.to_string() };
                self.picker(key.to_string(), value, current.is_empty(), None, picks, on_pick, window, cx)
            }
        };
        let hint = (current.is_empty() && inherited.is_some()).then(|| note("From the library agent.", INNER));
        col()
            .gap(px(SPACE_1_5))
            .child(control)
            .children(hint)
            .children(self.problem_line(problem, INNER, cx))
            .into_any_element()
    }

    /// The tools section. `own` is the node's value; `inherited` what an agent node falls back to.
    pub(crate) fn tools_section(&mut self, own: Option<&ToolGrants>, inherited: &ToolGrants, owner: Owner, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let grants = own.unwrap_or(inherited).clone();
        let tools: Vec<&str> = level_tools(grants.access).iter().copied().filter(|t| !grants.deny.iter().any(|d| d == t)).collect();
        let count = tools.len() + grants.allow.len();
        let level = LEVELS.iter().position(|l| *l == grants.access).unwrap_or(2);
        let base = inherited.clone();
        let access = Segmented::new("insp-tools-access")
            .small()
            .full_width()
            .option("Read only")
            .option_probe("insp-tools-access-read")
            .option("Read and run")
            .option_probe("insp-tools-access-run")
            .option("Full")
            .option_probe("insp-tools-access-full")
            .selected(level)
            .on_select(self.on_value(cx, move |this, ix: usize, cx| {
                let base = base.clone();
                this.edit_node("Set tool access", move |node| with_tools(node, &base, |t| t.access = LEVELS[ix.min(2)]), cx);
            }));
        let mut tags: Vec<AnyElement> = tools.iter().map(|t| Tag::new(t.to_string()).mono().into_any_element()).collect();
        for (i, name) in grants.allow.iter().enumerate() {
            let base = inherited.clone();
            let remove = self.on_click(cx, move |this, _, cx| {
                let base = base.clone();
                this.edit_node("Remove tool", move |node| with_tools(node, &base, |t| { if i < t.allow.len() { t.allow.remove(i); } }), cx);
            });
            tags.push(removable_tag(format!("insp-tools-allow-{i}"), name.clone(), TagTone::Accent, remove, window, cx));
        }
        let denied: Vec<AnyElement> = grants
            .deny
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let base = inherited.clone();
                let remove = self.on_click(cx, move |this, _, cx| {
                    let base = base.clone();
                    this.edit_node("Give tool back", move |node| with_tools(node, &base, |t| { if i < t.deny.len() { t.deny.remove(i); } }), cx);
                });
                removable_tag(format!("insp-tools-deny-{i}"), name.clone(), TagTone::Neutral, remove, window, cx)
            })
            .collect();
        let base = inherited.clone();
        let add = self.line(
            Spec::new("insp-tools-add", "", "Allow tool", on_node(move |node, text| {
                let name = text.trim().to_string();
                with_tools(node, &base, |t| {
                    if let Some(rest) = name.strip_prefix('-') {
                        let rest = rest.trim().to_string();
                        t.allow.retain(|a| *a != rest);
                        if !t.deny.contains(&rest) {
                            t.deny.push(rest);
                        }
                    } else if !t.allow.contains(&name) {
                        t.deny.retain(|d| *d != name);
                        t.allow.push(name);
                    }
                })
            }))
            .parse(super::fields::parse_word)
            .on_submit()
            .mono()
            .placeholder("Add a tool by name; -name takes one away"),
            None,
            window,
            cx,
        );
        let reset = (owner == Owner::Agent && own.is_some()).then(|| {
            Button::new("insp-tools-reset", "Use the library's")
                .ghost()
                .small()
                .on_click(self.on_click(cx, |this, _, cx| {
                    this.edit_node("Reset tools", |node| { if let Some(agent) = ops::agent_mut(node) { agent.tools = None; } }, cx)
                }))
                .probe("insp-tools-reset")
        });
        let meta = row().gap(px(SPACE_2)).children(reset).child(count.to_string());
        titled("Tools", Some(meta.into_any_element()))
            .gap(px(SPACE_2_5))
            .child(div().child(access).probe("insp-tools-access"))
            .child(tag_row(INNER, tags))
            .when(!denied.is_empty(), |d| {
                d.child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Taken away")).child(tag_row(INNER, denied))
            })
            .child(div().w(px(INNER)).child(add))
            .when(owner == Owner::Agent && own.is_none(), |d| d.child(note("From the library agent. A change here applies only inside this pipeline.", INNER)))
            .into_any_element()
    }

    /// The skills section: forced and auto skills, each with its switch and an ×.
    pub(crate) fn skills_section(&mut self, own: Option<&SkillPolicy>, inherited: &SkillPolicy, owner: Owner, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let policy = own.unwrap_or(inherited).clone();
        let installed: Vec<_> = self.catalog.read(cx).skills().value().map(|s| s.as_ref().clone()).unwrap_or_default();
        let meta = format!("{} forced · {} auto", policy.forced.len(), policy.auto.len());
        let mut rows: Vec<AnyElement> = Vec::new();
        let entries: Vec<(String, bool)> =
            policy.forced.iter().map(|s| (s.clone(), true)).chain(policy.auto.iter().map(|s| (s.clone(), false))).collect();
        for (i, (name, forced)) in entries.into_iter().enumerate() {
            let skill = installed.iter().find(|s| s.name == name);
            let tokens = skill.map(|s| base::fmt::tokens(s.tokens)).unwrap_or_default();
            let base_policy = inherited.clone();
            let flip_name = name.clone();
            let flip = Button::new(SharedString::from(format!("insp-skill-{i}-force")), if forced { "Forced" } else { "Force load" })
                .ghost()
                .small()
                .selected(forced)
                .icon(if forced { IconName::Lock } else { IconName::Plus })
                .on_click(self.on_click(cx, move |this, _, cx| {
                    let (base, name) = (base_policy.clone(), flip_name.clone());
                    this.edit_node("Force skill", move |node| with_skills(node, &base, |p| flip_skill(p, &name)), cx);
                }))
                .probe(format!("insp-skill-{i}-force"));
            let base_policy = inherited.clone();
            let gone = name.clone();
            let remove = IconButton::new(SharedString::from(format!("insp-skill-{i}-remove")), IconName::X)
                .small()
                .tooltip("Remove skill")
                .on_click(self.on_click(cx, move |this, _, cx| {
                    let (base, name) = (base_policy.clone(), gone.clone());
                    this.edit_node("Remove skill", move |node| with_skills(node, &base, |p| { p.forced.retain(|s| *s != name); p.auto.retain(|s| *s != name); }), cx);
                }))
                .probe(format!("insp-skill-{i}-remove"));
            let how = if forced { format!("always in context · {tokens} tokens") } else { format!("auto · loads when relevant · {tokens}") };
            rows.push(
                col()
                    .p(px(SPACE_2_5))
                    .px(px(SPACE_3))
                    .rounded(px(RADIUS_XL))
                    .border_1()
                    .border_color(LINE)
                    .bg(SURFACE)
                    .child(row().gap(px(SPACE_2)).child(div().flex_1().min_w_0().type_style(MONO).font_weight(gpui::FontWeight::MEDIUM).child(truncate(name.clone()))).child(flip).child(remove))
                    .children(skill.map(|s| div().w(px(INNER - 26.)).type_style(LABEL).text_color(TEXT_SECONDARY).child(s.description.clone())))
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(how))
                    .into_any_element(),
            );
        }
        let taken: Vec<String> = policy.forced.iter().chain(policy.auto.iter()).cloned().collect();
        let free: Vec<String> = installed.iter().map(|s| s.name.clone()).filter(|n| !taken.contains(n)).collect();
        let picks: Vec<Pick> = if free.is_empty() {
            vec![Pick::new("No other skills installed").soon()]
        } else {
            free.iter()
                .map(|n| {
                    let desc = installed.iter().find(|s| &s.name == n).map(|s| s.description.clone()).unwrap_or_default();
                    Pick::new(n.clone()).sub(desc)
                })
                .collect()
        };
        let base_policy = inherited.clone();
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let Some(name) = free.get(ix).cloned() else { return };
            let base = base_policy.clone();
            this.edit_node("Add skill", move |node| with_skills(node, &base, |p| p.auto.push(name)), cx);
        });
        let view = cx.entity().downgrade();
        let add = Button::new("insp-skill-add", "Add skill")
            .ghost()
            .small()
            .icon(IconName::Plus)
            .on_click(move |_, window, cx| open_picks("insp-skill-add".into(), picks.clone(), on_pick.clone(), view.clone(), window, cx));
        let empty = rows.is_empty();
        titled("Skills", Some(meta.into_any_element()))
            .gap(px(SPACE_1_5))
            .children(rows)
            .when(empty, |d| d.child(note("None picked: every installed skill is offered, and the model loads what it needs.", INNER)))
            .child(row().ml(px(-SPACE_2)).child(add.popover_anchor("insp-skill-add").probe("insp-skill-add")).child(grow()))
            .child(note(
                if owner == Owner::Agent && own.is_none() {
                    "From the library agent. Forced skills are in its context from the start and are passed to every agent it creates."
                } else {
                    "Forced skills are in its context from the start and are passed to every agent it creates."
                },
                INNER,
            ))
            .into_any_element()
    }

    /// "Can spawn sub-agents": the switch, depth, per run, budget, and the templates it spawns.
    pub(crate) fn spawn_section(&mut self, graph: &GraphDef, node: &NodeDef, own: Option<&SpawnPolicy>, inherited: &SpawnPolicy, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let policy = own.unwrap_or(inherited).clone();
        let edit = |base: SpawnPolicy, f: Rc<dyn Fn(&mut SpawnPolicy)>| {
            move |node: &mut NodeDef| {
                if let Some(agent) = ops::agent_mut(node) {
                    f(agent.spawn.get_or_insert_with(|| base.clone()));
                }
            }
        };
        let base = inherited.clone();
        let toggle = Toggle::new("insp-spawn", policy.enabled).on_change(self.on_value(cx, move |this, on: bool, cx| {
            this.edit_node("Toggle spawning", edit(base.clone(), Rc::new(move |p| p.enabled = on)), cx);
        }));
        let base = inherited.clone();
        let depth = Stepper::new("insp-spawn-depth", policy.max_depth as i32).range(1, 6).on_change(self.on_value(cx, move |this, v: i32, cx| {
            this.edit_node("Set spawn depth", edit(base.clone(), Rc::new(move |p| p.max_depth = v as u32)), cx);
        }));
        let base = inherited.clone();
        let per_run = Stepper::new("insp-spawn-per-run", policy.per_run as i32).range(1, 20).on_change(self.on_value(cx, move |this, v: i32, cx| {
            this.edit_node("Set spawns per run", edit(base.clone(), Rc::new(move |p| p.per_run = v as u32)), cx);
        }));
        let base = inherited.clone();
        let budget = self.line(
            Spec::new("insp-spawn-budget", usd_text(policy.budget_usd), "Set spawn budget", on_node(move |node, text| {
                if let Some(agent) = ops::agent_mut(node) {
                    agent.spawn.get_or_insert_with(|| base.clone()).budget_usd = usd_value(text);
                }
            }))
            .parse(parse_usd)
            .mono()
            .placeholder("No cap"),
            self.problem_at(&node.id, "spawn", cx),
            window,
            cx,
        );
        let chips: Vec<AnyElement> = graph
            .edges_from(&node.id)
            .filter(|e| e.kind == hub::model::EdgeKind::Spawn)
            .filter_map(|e| graph.node(&e.to))
            .enumerate()
            .map(|(ix, sub)| {
                let per = match &sub.kind {
                    NodeKind::Subagent(s) => format!(" · ≤{} per run", s.per_run),
                    _ => String::new(),
                };
                // A long name ends in `…` at the panel's width.
                div()
                    .flex()
                    .flex_none()
                    .max_w(px(INNER))
                    .debug_selector(move || format!("insp-spawn-chip-{ix}"))
                    .child(SpawnChip::new(format!("Spawns {}{per}", sub.name)).shrinks())
                    .into_any_element()
            })
            .collect();
        let section = titled("Can spawn sub-agents", Some(toggle.probe("insp-spawn").into_any_element())).gap(px(SPACE_1_5));
        if !policy.enabled {
            return section
                .child(note("Off: it works alone. On: it may create agents from its templates and give each a subset of its tools.", INNER))
                .into_any_element();
        }
        section
            .child(row_field("Max depth", depth.probe("insp-spawn-depth")))
            .child(row_field("Per run", per_run.probe("insp-spawn-per-run")))
            .child(
                row()
                    .gap(px(SPACE_3))
                    .child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Budget per created agent"))
                    .child(row().w(px(120.)).gap(px(SPACE_1)).child(div().text_color(TEXT_MUTED).child("$")).child(budget)),
            )
            .child(note(format!("Deeper than {} asks you first.", policy.max_depth), INNER))
            .when(!chips.is_empty(), |d| d.child(div().mt(px(SPACE_1)).child(tag_row(INNER, chips))))
            .into_any_element()
    }
}

/// Moves skill `name` between forced and auto.
fn flip_skill(policy: &mut SkillPolicy, name: &str) {
    if let Some(at) = policy.forced.iter().position(|s| s == name) {
        policy.forced.remove(at);
        policy.auto.push(name.to_string());
    } else if let Some(at) = policy.auto.iter().position(|s| s == name) {
        policy.auto.remove(at);
        policy.forced.push(name.to_string());
    }
}
