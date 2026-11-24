//! The menus: "New" (`agents-new-menu.html`), a runnable's more menu (`agents-more-menu.html`),
//! and the small pickers of the panel (assign an issue, add a skill, add a team member).
//!
//! `ui` menu rows are probed by their element id, so every row's id is its probe name. A
//! disabled row that needs a reason sits in a wrapper with the reason as its tooltip.

use gpui::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*};
use hub::api::{IssueStatus, Runnable, RunnableKind};
use hub::model::RunnableRef;
use hub::Route;
use ui::dev::Probe;
use ui::prelude::*;

use crate::rows;
use crate::view::{AgentsView, NEW, open_editor};

/// A disabled menu row with `reason` as its tooltip. Rows outside the menu's item list are
/// skipped by the keyboard, which a disabled row is anyway.
fn with_reason(item: MenuItem, id: &str, reason: &'static str) -> AnyElement {
    let name: SharedString = format!("{id}-why").into();
    let selector = name.clone();
    div()
        .id(name)
        .debug_selector(move || selector.to_string())
        .child(item.disabled(true))
        .tooltip(Tooltip::text(reason))
        .probe(id.to_string())
        .into_any_element()
}

/// Adds `item` to `menu`, disabled with a reason when `blocked` is one.
fn add(menu: Menu, item: MenuItem, id: &str, blocked: Option<&'static str>) -> Menu {
    match blocked {
        Some(reason) => menu.child(with_reason(item, id, reason)),
        None => menu.item(item),
    }
}

impl AgentsView {
    /// "New ▾": a blank or templated agent, orchestrator or pipeline, an autopilot, any
    /// template, or a file to import.
    pub(crate) fn open_new_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of(NEW, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_menu(
            anchor,
            Placement::below().end(),
            move |_, cx| {
                let count = view.read(cx).templates.len();
                let it = |id: &'static str, icon_name: IconName, name: &'static str, desc: &'static str, key: &'static str| {
                    MenuItem::new(id, name).tile(Tile::new(icon_name).medium()).subline(desc).kbd(key)
                };
                let pick = |kind: RunnableKind| {
                    let view = view.clone();
                    move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                        view.update(cx, |this, cx| this.open_templates(Some(kind), window, cx))
                    }
                };
                let (all, import) = (view.clone(), view.clone());
                Menu::new("agents-new-menu")
                    .width(384.)
                    .item(
                        it("agents-new-agent", IconName::Agent, "Agent", "One model with tools, skills and limits", "A")
                            .on_click(pick(RunnableKind::Agent)),
                    )
                    .item(
                        it("agents-new-orchestrator", IconName::Orchestrator, "Orchestrator", "Splits work across a team, can create agents", "O")
                            .on_click(pick(RunnableKind::Orchestrator)),
                    )
                    .item(
                        it("agents-new-pipeline", IconName::Pipelines, "Pipeline", "Sources, routers and agents on a canvas", "P")
                            .on_click(pick(RunnableKind::Pipeline)),
                    )
                    .item(
                        it("agents-new-autopilot", IconName::Clock, "Autopilot", "Runs on a schedule or when something happens", "T")
                            .on_click(|_, _, cx| hub::navigate(Route::Issues { issue: None }, cx)),
                    )
                    .separator()
                    .item(
                        MenuItem::new("agents-new-template", "From a template…")
                            .icon(IconName::File)
                            .hint(base::fmt::plural(count as u64, "template", "templates"))
                            .on_click(move |_, window, cx| all.update(cx, |this, cx| this.open_templates(None, window, cx))),
                    )
                    .item(
                        MenuItem::new("agents-new-import", "Import from file…")
                            .icon(IconName::Upload)
                            .hint(".yaml · AGENTS.md")
                            .on_click(move |_, window, cx| import.update(cx, |this, cx| this.open_import(window, cx))),
                    )
            },
            window,
            cx,
        );
    }

    /// The more menu of `reference`, under its "…" button in the panel head.
    pub(crate) fn open_more_menu(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let key: SharedString = format!("agents-more-{reference}").into();
        let anchor = Anchor::of(key, window, cx);
        let view = cx.entity();
        let reference = reference.clone();
        self.load_used_by(&reference, cx);
        ui::overlays(window, cx).open_menu(
            anchor,
            Placement::below().end(),
            move |_, cx| {
                let this = view.read(cx);
                let Some(runnable) = this.catalog.read(cx).runnable(&reference).cloned() else {
                    return Menu::new("agents-more-menu").width(280.);
                };
                more_menu(&view, &runnable, this, cx)
            },
            window,
            cx,
        );
    }

    /// Open issues with nothing attached, to attach this runnable to one.
    pub(crate) fn open_assign_menu(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of("agents-assign", window, cx);
        let issues = hub::issues(cx);
        let reference = reference.clone();
        ui::overlays(window, cx).open_menu(
            anchor,
            Placement::below().start(),
            move |_, cx| {
                let free: Vec<_> = issues
                    .read(cx)
                    .iter()
                    .filter(|i| i.runner.is_none() && !matches!(i.status, IssueStatus::Done))
                    .take(8)
                    .cloned()
                    .collect();
                let mut menu = Menu::new("agents-assign-menu").width(320.).label("Unattached issues");
                if free.is_empty() {
                    menu = menu.item(MenuItem::new("agents-assign-none", "No unattached issues").disabled(true));
                }
                for issue in free {
                    let (key, target) = (issue.key.clone(), reference.clone());
                    menu = menu.item(
                        MenuItem::new(SharedString::from(format!("agents-assign-{}", issue.key)), issue.title.clone())
                            .hint(issue.key.to_string())
                            .on_click(move |_, window, cx| {
                                drop(hub::commands(cx).attach_runner(&key, Some(target.clone()), cx));
                                ui::overlays(window, cx).toast(Toast::new(format!("{key} attached")), cx);
                            }),
                    );
                }
                menu
            },
            window,
            cx,
        );
    }

    /// Installed skills the agent does not list yet, to offer by name.
    pub(crate) fn open_add_skill(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of("agents-add-skill", window, cx);
        let view = cx.entity();
        let reference = reference.clone();
        ui::overlays(window, cx).open_menu(
            anchor,
            Placement::below().start(),
            move |_, cx| {
                let this = view.read(cx);
                let catalog = this.catalog.read(cx);
                let listed = catalog
                    .runnable(&reference)
                    .and_then(|r| r.agent.as_ref())
                    .map(|a| [a.skills.forced.clone(), a.skills.auto.clone()].concat())
                    .unwrap_or_default();
                let skills: Vec<_> = catalog
                    .skills()
                    .value()
                    .map(|s| s.iter().filter(|s| !listed.contains(&s.name)).cloned().collect())
                    .unwrap_or_default();
                let mut menu = Menu::new("agents-add-skill-menu").width(320.).max_height(360.);
                if skills.is_empty() {
                    menu = menu.item(MenuItem::new("agents-skill-none", "Every installed skill is listed").disabled(true));
                }
                for skill in skills {
                    let (view, reference, name) = (view.clone(), reference.clone(), skill.name.clone());
                    menu = menu.item(
                        MenuItem::new(SharedString::from(format!("agents-skill-{}", skill.name)), skill.name.clone())
                            .subline(skill.description.clone())
                            .hint(base::fmt::tokens(skill.tokens))
                            .on_click(move |_, _, cx| {
                                let name = name.clone();
                                view.update(cx, |this, cx| {
                                    this.save(&reference, cx, move |r| {
                                        if let Some(agent) = r.agent.as_mut() {
                                            agent.skills.auto.push(name);
                                        }
                                    })
                                })
                            }),
                    );
                }
                menu
            },
            window,
            cx,
        );
    }

    /// Agents and pipelines an orchestrator could add to its team.
    pub(crate) fn open_add_member(&mut self, reference: &RunnableRef, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of("agents-add-member", window, cx);
        let view = cx.entity();
        let reference = reference.clone();
        ui::overlays(window, cx).open_menu(
            anchor,
            Placement::below().start(),
            move |_, cx| {
                let this = view.read(cx);
                let catalog = this.catalog.read(cx);
                let team = catalog.runnable(&reference).and_then(|r| r.agent.as_ref()).map(|a| a.team.clone()).unwrap_or_default();
                let candidates: Vec<Runnable> = catalog
                    .runnables()
                    .iter()
                    .filter(|r| {
                        r.reference != reference && !r.archived && !rows::is_spawned(r) && !team.contains(&r.reference)
                    })
                    .cloned()
                    .collect();
                let mut menu = Menu::new("agents-add-member-menu").width(320.).max_height(360.);
                for candidate in candidates {
                    let (view, reference, member) = (view.clone(), reference.clone(), candidate.reference.clone());
                    menu = menu.item(
                        MenuItem::new(SharedString::from(format!("agents-add-{}", candidate.reference)), candidate.name.clone())
                            .tile(Tile::new(crate::list::kind_icon(&candidate)).medium())
                            .hint(rows::kind_word(candidate.kind))
                            .on_click(move |_, _, cx| {
                                let member = member.clone();
                                view.update(cx, |this, cx| {
                                    this.save(&reference, cx, move |r| {
                                        if let Some(agent) = r.agent.as_mut() {
                                            agent.team.push(member);
                                        }
                                    })
                                })
                            }),
                    );
                }
                menu
            },
            window,
            cx,
        );
    }
}

/// The rows of the more menu, in the mockup's order: edit, duplicate, (rename), run history,
/// export; pause, archive, delete.
fn more_menu(view: &gpui::Entity<AgentsView>, runnable: &Runnable, this: &AgentsView, cx: &gpui::App) -> Menu {
    let catalog = this.catalog.read(cx);
    let activity = catalog.activity(&runnable.reference);
    let read_only = rows::read_only_reason(runnable);
    let removal = rows::removal_blocked(runnable);
    let pipeline = runnable.kind == RunnableKind::Pipeline;
    let reference = runnable.reference.clone();
    let act = |f: fn(&mut AgentsView, &RunnableRef, &mut Window, &mut Context<AgentsView>)| {
        let (view, reference) = (view.clone(), reference.clone());
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
            view.update(cx, |this, cx| f(this, &reference, window, cx))
        }
    };

    let mut menu = Menu::new("agents-more-menu").width(280.);
    let edit = if pipeline {
        let target = reference.clone();
        MenuItem::new("agents-more-edit", "Open in editor").icon(IconName::Edit).hint("E").on_click(move |_, _, cx| open_editor(&target, cx))
    } else {
        MenuItem::new("agents-more-edit", "Edit prompt and tools")
            .icon(IconName::Edit)
            .hint("E")
            .on_click(act(|this, r, window, cx| this.open_edit(r, window, cx)))
    };
    menu = add(menu, edit, "agents-more-edit", if pipeline { None } else { read_only });
    menu = menu.item(
        MenuItem::new("agents-more-duplicate", "Duplicate")
            .icon(IconName::Fork)
            .hint("⌘D")
            .on_click(act(|this, r, _, cx| this.duplicate(r, cx))),
    );
    let rename = MenuItem::new("agents-more-rename", "Rename…")
        .icon(IconName::Edit)
        .on_click(act(|this, r, window, cx| this.open_rename(r, window, cx)));
    menu = add(menu, rename, "agents-more-rename", read_only);
    let runs = activity.map_or(0, |a| a.runs_today);
    menu = menu.item(
        MenuItem::new("agents-more-history", "Run history")
            .icon(IconName::Clock)
            .hint(format!("{runs} today"))
            .on_click(act(|this, r, _, cx| this.show_history(r, cx))),
    );
    menu = menu.item(
        MenuItem::new("agents-more-export", "Export as YAML")
            .icon(IconName::Download)
            .on_click(act(|this, r, window, cx| this.open_export(r, window, cx))),
    );
    menu = menu.separator();

    if !runnable.archived {
        let pause = if runnable.paused {
            MenuItem::new("agents-more-pause", "Resume")
                .icon(IconName::Play)
                .subline("Takes new work again")
                .on_click(act(|this, r, _, cx| this.set_paused(r, false, cx)))
        } else {
            let sub = match rows::running_issue(activity) {
                Some(key) => format!("Finishes {key}, takes no new work"),
                None => "Takes no new work".to_string(),
            };
            MenuItem::new("agents-more-pause", "Pause")
                .icon(IconName::Pause)
                .subline(sub)
                .on_click(act(|this, r, _, cx| this.set_paused(r, true, cx)))
        };
        menu = add(menu, pause, "agents-more-pause", removal);
    }
    let archive = if runnable.archived {
        MenuItem::new("agents-more-archive", "Restore").icon(IconName::Archive).on_click(act(|this, r, window, cx| this.restore(r, window, cx)))
    } else {
        MenuItem::new("agents-more-archive", "Archive").icon(IconName::Archive).on_click(act(|this, r, window, cx| this.archive(r, window, cx)))
    };
    menu = add(menu, archive, "agents-more-archive", removal);
    let delete = MenuItem::new("agents-more-delete", "Delete…")
        .icon(IconName::Trash)
        .danger()
        .subline(this.used_by_line(&runnable.reference, cx))
        .on_click(act(|this, r, window, cx| this.open_delete(r, window, cx)));
    add(menu, delete, "agents-more-delete", removal)
}
