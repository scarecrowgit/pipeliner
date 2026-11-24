//! The detail panel of an agent or orchestrator, as `agents-pipelines.html` draws it: the head
//! (tile, name, state, more menu, description, "Chat with it", "Assign an issue", facts), then
//! "Use it from", "Skills", "Team it runs", "Can create its own agents", "How it routes", and
//! below the mockup's sections the instructions, what uses it and recent chats.
//!
//! Controls that change the definition save through `Commands::save_runnable`; definitions
//! that cannot be changed here (built-in, Claude Code, created at run time) draw them inert.

use gpui::{AnyElement, App, ClickEvent, Context, ElementId, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::Route;
use hub::api::{Origin, Runnable, RunnableKind, RunnableState, Skill, ToolAccess, ToolGrants};
use hub::model::RunnableRef;
use ui::dev::Probe;
use ui::prelude::*;

use crate::list::{PLACES, fit, kind_icon, state_cell, uses_at};
use crate::rows;
use crate::view::{AgentsView, PANEL_W};

/// Width of text inside a panel section (the panel less its 20px padding each side), px.
/// Wrapping text gets a definite width so it measures its height right.
pub(crate) const TEXT_W: f32 = PANEL_W - 2. * SPACE_5;

/// "Read only", "Read and run", "Full access", with extra and removed tools counted.
pub(crate) fn tools_summary(tools: &ToolGrants) -> String {
    let mut text = match tools.access {
        ToolAccess::ReadOnly => "Read only",
        ToolAccess::ReadAndRun => "Read and run",
        ToolAccess::Full => "Full access",
    }
    .to_string();
    if !tools.allow.is_empty() {
        text.push_str(&format!(" · +{}", base::fmt::plural(tools.allow.len() as u64, "tool", "tools")));
    }
    if !tools.deny.is_empty() {
        text.push_str(&format!(" · −{}", tools.deny.len()));
    }
    text
}

/// Where a definition lives, in words, with its file when it has one.
pub(crate) fn lives_in(runnable: &Runnable, cx: &App) -> String {
    let place = match &runnable.origin {
        Origin::Builtin => "Built in".to_string(),
        Origin::Library => "Library".to_string(),
        Origin::Project { repo } => format!("Project · {}", hub::display_path(repo, cx)),
        Origin::ClaudeCode { path } => format!("Claude Code · {}", hub::display_path(path, cx)),
        Origin::Spawned { .. } => "Created at run time".to_string(),
    };
    match &runnable.path {
        Some(path) => hub::display_path(path, cx),
        None => place,
    }
}

/// A panel section with a definite text width (`.psec`).
pub(crate) fn section(title: impl Into<SharedString>) -> Section {
    Section::new(title)
}

/// Wrapping secondary text at the panel's text width.
pub(crate) fn para(text: impl Into<SharedString>) -> gpui::Div {
    div().w(px(TEXT_W)).text_color(TEXT_SECONDARY).child(text.into())
}

/// A small muted label above a list (`.flabel`).
fn flabel(text: impl Into<SharedString>) -> gpui::Div {
    div().type_style(LABEL_STRONG).text_color(TEXT_MUTED).child(text.into())
}

/// A bordered card for a skill (`.card`, 10/12 padding).
fn card() -> gpui::Div {
    col().px(px(SPACE_3)).py(px(SPACE_2_5)).border_1().border_color(LINE).rounded(px(RADIUS_LG)).bg(SURFACE)
}

impl AgentsView {
    /// The panel's sections, top to bottom, for the selected runnable.
    pub(crate) fn render_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Vec<AnyElement>> {
        let runnable = self.current(cx)?.clone();
        Some(if runnable.kind == RunnableKind::Pipeline {
            self.pipeline_panel(&runnable, window, cx)
        } else {
            self.agent_panel(&runnable, window, cx)
        })
    }

    /// The head of every panel: tile, name, kind and state, the more button, the description,
    /// `buttons`, and `facts` under them.
    pub(crate) fn panel_head(
        &mut self,
        runnable: &Runnable,
        buttons: Vec<AnyElement>,
        facts: KeyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let catalog = self.catalog.read(cx);
        let mark = rows::state_mark(runnable, catalog.activity(&runnable.reference));
        let spawned = rows::is_spawned(runnable);
        let kind: SharedString = match &runnable.origin {
            Origin::Spawned { by, .. } => {
                format!("Created by {}", catalog.runnable(by).map_or_else(|| by.id().to_string(), |r| r.name.clone())).into()
            }
            _ => rows::kind_word(runnable.kind).into(),
        };
        let more_id: SharedString = format!("agents-more-{}", runnable.reference).into();
        let more_open = ui::overlays(window, cx).is_open(more_id.clone(), cx);
        let reference = runnable.reference.clone();
        let more = IconButton::new(more_id.clone(), IconName::More)
            .small()
            .current(more_open)
            .tooltip("More actions")
            .on_click(cx.listener(move |this, _, window, cx| this.open_more_menu(&reference, window, cx)))
            .popover_anchor(more_id.clone())
            .probe(more_id);
        let read_only = rows::read_only_reason(runnable);
        col()
            .flex_none()
            .px(px(SPACE_5))
            .py(px(SPACE_4))
            .border_b_1()
            .border_color(LINE)
            .child(
                row()
                    .items_start()
                    .gap(px(SPACE_3))
                    .child(
                        // The tile and the name: a double-click opens it as the list's row does.
                        // The more button is beside this box, not in it.
                        row()
                            .id("agents-head")
                            .debug_selector(|| "agents-head".into())
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .gap(px(SPACE_3))
                            .child(Tile::new(kind_icon(runnable)).tone(if spawned { TileTone::Spawn } else { TileTone::Neutral }).large())
                            .child(
                                col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(truncate_block(runnable.name.clone(), TITLE.line).type_style(TITLE))
                                    .child(
                                        row()
                                            .min_w_0()
                                            .gap(px(SPACE_1_5))
                                            .type_style(LABEL)
                                            .child(fit(kind).text_color(if spawned { SPAWN_TEXT } else { TEXT_MUTED }))
                                            .child(StatusDot::new(crate::list::status(mark.dot)).small())
                                            .child(truncate_lh(mark.text.clone(), LABEL.line).text_color(crate::list::tone_color(mark.tone))),
                                    ),
                            )
                            .probe("agents-head")
                            .on_click(cx.listener({
                                let reference = runnable.reference.clone();
                                move |this, event: &ClickEvent, window, cx| {
                                    if event.click_count() >= 2 {
                                        this.open_edit(&reference, window, cx);
                                    }
                                }
                            })),
                    )
                    .child(more),
            )
            .when(!runnable.description.is_empty(), |d| d.child(para(runnable.description.clone()).mt(px(SPACE_2_5))))
            .when_some(read_only, |d, why| {
                d.child(row().mt(px(SPACE_2)).gap(px(SPACE_1_5)).child(icon(IconName::Lock, 12., TEXT_MUTED)).child(Note::new(why)))
            })
            .when(!buttons.is_empty(), |d| d.child(row().mt(px(SPACE_3)).gap(px(SPACE_2)).children(buttons)))
            .child(div().mt(px(SPACE_3_5)).child(facts))
            .into_any_element()
    }

    fn agent_panel(&mut self, runnable: &Runnable, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let agent = runnable.agent.clone().unwrap_or_default();
        let activity = self.catalog.read(cx).activity(&runnable.reference).cloned();
        let runtime_icon = match agent.runtime.as_str() {
            "cloud" => IconName::Cloud,
            "laptop" => IconName::Laptop,
            _ => IconName::Server,
        };
        let mut facts = KeyValue::new()
            .row("Model", div().font_family(FONT_MONO).child(if agent.model.is_empty() { "default".to_string() } else { agent.model.short().to_string() }));
        if !agent.runtime.is_empty() {
            facts = facts.row("Runtime", row().gap(px(SPACE_1_5)).child(icon(runtime_icon, 12., TEXT_SECONDARY)).child(agent.runtime.clone()));
        }
        facts = facts
            .row("Activity", div().child(rows::runs_today(activity.as_ref())))
            .row("Tools", div().child(tools_summary(&agent.tools)))
            .row("Lives in", div().min_w_0().child(truncate_middle(lives_in(runnable, cx), BODY.line)));
        let buttons = self.agent_buttons(runnable, cx);
        let head = self.panel_head(runnable, buttons, facts, window, cx);
        let editable = rows::read_only_reason(runnable).is_none();

        let mut out = vec![head, self.uses_section(runnable, editable, cx), self.skills_section(runnable, editable, cx)];
        if runnable.kind == RunnableKind::Orchestrator {
            out.push(self.team_section(runnable, editable, cx));
        }
        out.push(self.spawn_section(runnable, editable, cx));
        if runnable.kind == RunnableKind::Orchestrator && !agent.routing.is_empty() {
            out.push(section("How it routes").child(para(agent.routing.clone())).into_any_element());
        }
        out.push(self.role_section(runnable, editable, cx));
        out.push(self.used_by_section(runnable, cx));
        out.push(self.recent_chats(runnable, cx));
        out
    }

    /// "Chat with it" and "Assign an issue".
    pub(crate) fn agent_buttons(&mut self, runnable: &Runnable, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let chat = runnable.reference.clone();
        let usable = !runnable.archived;
        let mut out = Vec::new();
        if runnable.kind != RunnableKind::Pipeline {
            out.push(
                Button::new("agents-chat", "Chat with it")
                    .secondary()
                    .small()
                    .icon(IconName::Chat)
                    .disabled(!usable || !runnable.uses.chat)
                    .on_click(cx.listener(move |this, _, _, cx| this.chat_with(&chat, cx)))
                    .probe("agents-chat")
                    .into_any_element(),
            );
        }
        let assign = runnable.reference.clone();
        out.push(
            Button::new("agents-assign", "Assign an issue")
                .ghost()
                .small()
                .icon(IconName::Issues)
                .disabled(!usable || !runnable.uses.issues)
                .on_click(cx.listener(move |this, _, window, cx| this.open_assign_menu(&assign, window, cx)))
                .popover_anchor("agents-assign")
                .probe("agents-assign")
                .into_any_element(),
        );
        out
    }

    /// "Use it from": a chip per place, pressed when it is on.
    pub(crate) fn uses_section(&mut self, runnable: &Runnable, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let chip = |ix: usize, cx: &mut Context<Self>| {
            let (icon_name, label) = PLACES[ix];
            let on = uses_at(&runnable.uses, ix);
            let reference = runnable.reference.clone();
            let id: SharedString = format!("agents-use-{}", ["chat", "issues", "pipelines", "autopilots"][ix]).into();
            Chip::new(id.clone(), label)
                .pressed(on)
                .icon(if on { IconName::Check } else { icon_name })
                .when(editable, |c| {
                    c.on_click(cx.listener(move |this, _, _, cx| {
                        this.save(&reference, cx, |r| match ix {
                            0 => r.uses.chat = !on,
                            1 => r.uses.issues = !on,
                            2 => r.uses.pipelines = !on,
                            _ => r.uses.autopilots = !on,
                        })
                    }))
                })
                .probe(id)
        };
        // Two rows instead of a wrapping one: a wrapping row would squeeze the note below.
        section("Use it from")
            .child(row().gap(px(SPACE_1_5)).child(chip(0, cx)).child(chip(1, cx)).child(chip(2, cx)))
            .child(row().mt(px(SPACE_1_5)).gap(px(SPACE_1_5)).child(chip(3, cx)))
            .child(div().mt(px(SPACE_2)).w(px(TEXT_W)).child(Note::new("The same agent, with the same tools and limits, wherever it runs.")))
            .into_any_element()
    }

    fn skills_section(&mut self, runnable: &Runnable, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let policy = runnable.agent.as_ref().map(|a| a.skills.clone()).unwrap_or_default();
        let installed: Vec<Skill> = self.catalog.read(cx).skills().value().map(|s| s.as_ref().clone()).unwrap_or_default();
        let find = |name: &str| installed.iter().find(|s| s.name == name);
        let meta = format!("{} forced · {} auto", policy.forced.len(), policy.auto.len());
        let mut cards: Vec<AnyElement> = Vec::new();
        for name in &policy.forced {
            let skill = find(name);
            cards.push(
                card()
                    .child(
                        row()
                            .child(div().type_style(BODY_STRONG).font_family(FONT_MONO).child(name.clone()))
                            .child(grow())
                            .child(Badge::new("Forced").tone(BadgeTone::Accent).icon(IconName::Lock)),
                    )
                    .when_some(skill, |d, s| {
                        d.child(div().mt(px(SPACE_0_5)).type_style(LABEL).text_color(TEXT_SECONDARY).child(s.description.clone()))
                            .child(
                                div()
                                    .type_style(LABEL)
                                    .text_color(TEXT_MUTED)
                                    .child(format!("always in context · {} tokens", base::fmt::tokens(s.tokens))),
                            )
                    })
                    .into_any_element(),
            );
        }
        for (ix, name) in policy.auto.iter().enumerate() {
            let skill = find(name);
            let (reference, skill_name) = (runnable.reference.clone(), name.clone());
            let id: SharedString = format!("agents-force-{name}").into();
            cards.push(
                card()
                    .child(
                        row()
                            .child(div().type_style(BODY_STRONG).font_family(FONT_MONO).child(name.clone()))
                            .child(grow())
                            .when(editable, |d| {
                                d.child(
                                    Button::new(id.clone(), "Force load")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let skill = skill_name.clone();
                                            this.save(&reference, cx, move |r| {
                                                if let Some(agent) = r.agent.as_mut() {
                                                    agent.skills.auto.retain(|s| *s != skill);
                                                    agent.skills.forced.push(skill);
                                                }
                                            })
                                        }))
                                        .probe(id.clone()),
                                )
                            }),
                    )
                    .when_some(skill, |d, s| {
                        d.child(div().mt(px(SPACE_0_5)).type_style(LABEL).text_color(TEXT_SECONDARY).child(s.description.clone()))
                            .child(
                                div()
                                    .type_style(LABEL)
                                    .text_color(TEXT_MUTED)
                                    .child(format!("auto · loads when relevant · {}", base::fmt::tokens(s.tokens))),
                            )
                    })
                    .id(("agents-skill", ix))
                    .into_any_element(),
            );
        }
        let reference = runnable.reference.clone();
        section("Skills")
            .meta(div().child(meta))
            .child(col().gap(px(SPACE_1_5)).children(cards))
            .when(policy.auto.is_empty(), |d| {
                d.child(div().mt(px(SPACE_1_5)).w(px(TEXT_W)).child(Note::new("Every installed skill is offered by name and loads when relevant.")))
            })
            .when(editable, |d| {
                d.child(
                    row().mt(px(SPACE_2)).child(
                        Button::new("agents-add-skill", "Add skill")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(move |this, _, window, cx| this.open_add_skill(&reference, window, cx)))
                            .popover_anchor("agents-add-skill")
                            .probe("agents-add-skill"),
                    ),
                )
            })
            .child(
                div()
                    .mt(px(SPACE_1))
                    .w(px(TEXT_W))
                    .child(Note::new("Forced skills are in its context from the start and are passed to every agent it creates.")),
            )
            .into_any_element()
    }

    /// A compact row for a runnable inside a section: tile, name, kind, state at the right.
    /// Clicking selects it; a double-click opens it as the list's row does.
    pub(crate) fn member_row(&self, id: ElementId, reference: &RunnableRef, cx: &mut Context<Self>) -> AnyElement {
        let catalog = self.catalog.read(cx);
        let Some(member) = catalog.runnable(reference) else {
            return row()
                .id(id)
                .h(px(32.))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(format!("{reference} · not in the library"))
                .into_any_element();
        };
        let mark = rows::state_mark(member, catalog.activity(reference));
        let target = reference.clone();
        row()
            .id(id)
            .h(px(32.))
            .gap(px(SPACE_2))
            .cursor_pointer()
            .child(Tile::new(kind_icon(member)))
            .child(fit(member.name.clone()).type_style(BODY_STRONG))
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(rows::kind_word(member.kind)))
            .child(grow())
            .child(div().flex_initial().min_w_0().child(state_cell(&mark)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| select_or_open(this, &target, event, window, cx)))
            .into_any_element()
    }

    fn team_section(&mut self, runnable: &Runnable, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let team = runnable.agent.as_ref().map(|a| a.team.clone()).unwrap_or_default();
        let members: Vec<AnyElement> =
            team.iter().enumerate().map(|(ix, member)| self.member_row(("agents-member", ix).into(), member, cx)).collect();
        let reference = runnable.reference.clone();
        section("Team it runs")
            .meta(div().child(base::fmt::plural(team.len() as u64, "member", "members")))
            .child(col().children(members))
            .when(team.is_empty(), |d| d.child(Note::new("Nobody yet: it does the work itself.")))
            .when(editable, |d| {
                d.child(
                    row().mt(px(SPACE_1_5)).child(
                        Button::new("agents-add-member", "Add agent or pipeline")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(move |this, _, window, cx| this.open_add_member(&reference, window, cx)))
                            .popover_anchor("agents-add-member")
                            .probe("agents-add-member"),
                    ),
                )
            })
            .into_any_element()
    }

    fn spawn_section(&mut self, runnable: &Runnable, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let spawn = runnable.agent.as_ref().map(|a| a.spawn.clone()).unwrap_or_default();
        let reference = runnable.reference.clone();
        let view = cx.entity();
        let toggle = {
            let (view, reference) = (view.clone(), reference.clone());
            Toggle::new("agents-spawn-toggle", spawn.enabled).disabled(!editable).on_change(move |on, _, cx| {
                view.update(cx, |this, cx| this.save(&reference, cx, move |r| set_spawn(r, |s| s.enabled = on)))
            })
        };
        let stepper = |id: &'static str, value: u32, max: i32, set: fn(&mut hub::api::SpawnPolicy, u32)| {
            let (view, reference) = (view.clone(), reference.clone());
            Stepper::new(id, value as i32).range(1, max).on_change(move |v, _, cx| {
                let v = v.max(1) as u32;
                view.update(cx, |this, cx| this.save(&reference, cx, move |r| set_spawn(r, |s| set(s, v))))
            })
        };
        let budget = spawn.budget_usd.map(|usd| format!(" and ≤{} per issue", base::fmt::cost(usd))).unwrap_or_default();
        let note = format!(
            "Each created agent gets a subset of its tools{budget}. Deeper than {} asks you first.",
            spawn.max_depth
        );
        let catalog = self.catalog.read(cx);
        let created: Vec<(String, rows::StateMark, bool)> = catalog
            .runnables()
            .iter()
            .filter(|r| matches!(&r.origin, Origin::Spawned { by, .. } if *by == runnable.reference))
            .map(|r| {
                let activity = catalog.activity(&r.reference);
                let alive = matches!(activity.map(|a| &a.state), Some(RunnableState::Alive { .. }));
                let mut mark = rows::state_mark(r, activity);
                if !alive {
                    mark.text = format!("{} · {}", mark.text, base::fmt::cost(activity.map_or(0.0, |a| a.cost_today_usd)));
                }
                (r.name.clone(), mark, alive)
            })
            .collect();
        let created_list = created.into_iter().enumerate().map(|(ix, (name, mark, alive))| {
            row()
                .id(("agents-created", ix))
                .h(px(28.))
                .gap(px(SPACE_2))
                .child(StatusDot::new(if alive { Status::Spawn } else { Status::Idle }).small())
                .child(fit(name).text_color(SPAWN_TEXT).when(alive, |d| d.type_style(BODY_STRONG)))
                .child(grow())
                .child(
                    fit(mark.text)
                        .type_style(LABEL)
                        .text_color(if alive { TEXT_SECONDARY } else { TEXT_MUTED }),
                )
        });
        let has_created = catalog.runnables().iter().any(|r| matches!(&r.origin, Origin::Spawned { by, .. } if *by == runnable.reference));
        section("Can create its own agents")
            .meta(toggle.probe("agents-spawn-toggle"))
            .when(spawn.enabled, |d| {
                d.child(
                    row()
                        .gap(px(SPACE_5))
                        .when(!editable, |d| d.opacity(0.6))
                        .child(
                            row()
                                .gap(px(SPACE_2))
                                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Alive at once"))
                                .child(stepper("agents-spawn-alive", spawn.alive_at_once, 10, |s, v| s.alive_at_once = v)),
                        )
                        .child(
                            row()
                                .gap(px(SPACE_2))
                                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Depth"))
                                .child(stepper("agents-spawn-depth", spawn.max_depth, 5, |s, v| s.max_depth = v)),
                        ),
                )
                .child(div().mt(px(SPACE_2)).w(px(TEXT_W)).child(Note::new(note)))
            })
            .when(!spawn.enabled, |d| d.child(div().w(px(TEXT_W)).child(Note::new("It works alone: it can't create agents."))))
            .when(has_created, |d| {
                d.child(flabel(format!("Created by {}", runnable.name)).mt(px(SPACE_3)))
                    .child(col().mt(px(SPACE_1)).children(created_list))
            })
            .into_any_element()
    }

    fn role_section(&mut self, runnable: &Runnable, editable: bool, cx: &mut Context<Self>) -> AnyElement {
        let reference = runnable.reference.clone();
        let md = self.role.as_ref().filter(|(r, _)| *r == runnable.reference).map(|(_, md)| md.clone());
        let empty = runnable.agent.as_ref().is_none_or(|a| a.role.trim().is_empty());
        section("Instructions")
            .when(editable, |d| {
                d.meta(
                    Link::new("agents-edit-role", "Edit")
                        .on_click(cx.listener(move |this, _, window, cx| this.open_edit(&reference, window, cx))),
                )
            })
            .when(empty, |d| d.child(Note::new("No instructions yet.")))
            .when_some(md.filter(|_| !empty), |d, md| d.child(div().w(px(TEXT_W)).child(md)))
            .into_any_element()
    }

    /// What uses it: attached issues and pipelines it sits in.
    pub(crate) fn used_by_section(&mut self, runnable: &Runnable, cx: &mut Context<Self>) -> AnyElement {
        let used = self.used_by.get(&runnable.reference).cloned();
        let body: AnyElement = match used {
            None => Note::new("Looking…").into_any_element(),
            Some(used) if used.issues.is_empty() && used.pipelines.is_empty() => {
                Note::new("Nothing uses it yet.").into_any_element()
            }
            Some(used) => {
                let catalog = self.catalog.read(cx);
                let pipelines = used.pipelines.iter().enumerate().map(|(ix, id)| {
                    let name = catalog.runnable(&RunnableRef::pipeline(id.as_str())).map_or_else(|| id.to_string(), |r| r.name.clone());
                    let target = RunnableRef::pipeline(id.as_str());
                    row()
                        .id(("agents-used-pipeline", ix))
                        .h(px(28.))
                        .gap(px(SPACE_2))
                        .cursor_pointer()
                        .child(icon(IconName::Pipelines, 14., TEXT_MUTED))
                        .child(truncate(name))
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| select_or_open(this, &target, event, window, cx)))
                });
                let issues = used.issues.iter().enumerate().map(|(ix, key)| {
                    let target = key.clone();
                    row()
                        .id(("agents-used-issue", ix))
                        .h(px(28.))
                        .gap(px(SPACE_2))
                        .cursor_pointer()
                        .child(icon(IconName::Issues, 14., TEXT_MUTED))
                        .child(div().font_family(FONT_MONO).child(key.to_string()))
                        .on_click(move |_, _, cx| hub::navigate(Route::Issues { issue: Some(target.clone()) }, cx))
                });
                col().children(pipelines.map(IntoElement::into_any_element)).children(issues.map(IntoElement::into_any_element)).into_any_element()
            }
        };
        section("Used by").child(body).into_any_element()
    }

    fn recent_chats(&mut self, runnable: &Runnable, cx: &mut Context<Self>) -> AnyElement {
        let index = hub::chat_index(cx);
        let index = index.read(cx);
        let chats: Vec<_> = index
            .pinned()
            .iter()
            .chain(index.recent())
            .filter(|s| s.runnable == runnable.reference)
            .take(5)
            .cloned()
            .collect();
        let rows_ = chats.into_iter().enumerate().map(|(ix, session)| {
            let id = session.id.clone();
            row()
                .id(("agents-chat-row", ix))
                .h(px(28.))
                .gap(px(SPACE_2))
                .cursor_pointer()
                .child(icon(IconName::Chat, 14., TEXT_MUTED))
                .child(div().flex_1().min_w_0().child(truncate(session.title.clone())))
                .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(base::fmt::cost(session.cost_usd)))
                .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: id.clone() }, cx))
        });
        let empty = rows_.len() == 0;
        section("Recent chats")
            .last()
            .when(empty, |d| d.child(Note::new("No chats with it yet.")))
            .child(col().children(rows_))
            .into_any_element()
    }
}

/// A click on a runnable named in the panel selects it; a double-click opens it as Enter on
/// its row does.
pub(crate) fn select_or_open(view: &mut AgentsView, target: &RunnableRef, event: &ClickEvent, window: &mut Window, cx: &mut Context<AgentsView>) {
    view.select(Some(target.clone()), cx);
    if event.click_count() >= 2 {
        view.open_edit(target, window, cx);
    }
}

/// Changes the spawn policy of an agent definition.
fn set_spawn(runnable: &mut Runnable, f: impl FnOnce(&mut hub::api::SpawnPolicy)) {
    if let Some(agent) = runnable.agent.as_mut() {
        f(&mut agent.spawn);
    }
}
