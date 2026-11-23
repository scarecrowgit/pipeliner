//! Inspector for the selected step: header with move / duplicate / delete, then the fields
//! its kind uses (workdir, model, instructions, tools, skills, max steps, sub-agents,
//! decision question and "no" target, render files, approval target, command, pull request
//! options). Text fields save on Enter / blur; toggles and dropdowns save right away.

use gpui::{AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, prelude::*, px};
use pipeline::{PipelineDef, StageStatus, StepDef, StepKind, Tools};
use ui::{Badge, Button, ButtonVariant, Stepper, icon, spawn_chip, theme::*};

use crate::inspector::FULL_W;
use crate::layout::{kind_line, kind_look, target_name};
use crate::menu::Item;
use crate::snap::Snap;
use crate::widgets::*;
use crate::{Confirm, Field, Menu, PipelineEditorScreen, Tab};

/// Whose skills a chip edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Skills {
    Step,
    Delegate,
}

fn lists(s: &mut StepDef, who: Skills) -> Option<(&mut Vec<String>, &mut Vec<String>)> {
    match who {
        Skills::Step => Some((&mut s.skills, &mut s.skills_on_demand)),
        Skills::Delegate => s.delegate.as_mut().map(|d| (&mut d.skills, &mut d.skills_on_demand)),
    }
}

impl PipelineEditorScreen {
    /// Move a skill between "in the prompt" and "on demand".
    fn toggle_skill(&mut self, who: Skills, name: String, cx: &mut Context<Self>) {
        self.edit_step(cx, |s| {
            let Some((prompt, demand)) = lists(s, who) else { return };
            if let Some(i) = prompt.iter().position(|x| *x == name) {
                prompt.remove(i);
                demand.push(name);
            } else if let Some(i) = demand.iter().position(|x| *x == name) {
                demand.remove(i);
                prompt.push(name);
            }
        });
    }

    fn remove_skill(&mut self, who: Skills, name: String, cx: &mut Context<Self>) {
        self.edit_step(cx, |s| {
            if let Some((prompt, demand)) = lists(s, who) {
                prompt.retain(|x| *x != name);
                demand.retain(|x| *x != name);
            }
        });
    }

    fn add_skill(&mut self, who: Skills, name: String, cx: &mut Context<Self>) {
        self.edit_step(cx, |s| {
            if let Some((_, demand)) = lists(s, who) {
                demand.push(name);
            }
        });
    }

    /// Skill chip: click the name to move it to the other list, × removes it.
    fn skill_chip(&self, who: Skills, ix: usize, name: &str, in_prompt: bool, cx: &mut Context<Self>) -> AnyElement {
        let missing = !self.skills.iter().any(|s| s == name);
        let (toggle_name, remove_name) = (name.to_string(), name.to_string());
        let (border, bg, fg) = if missing {
            (WARNING, WARNING_SOFT, WARNING)
        } else if in_prompt {
            (ACCENT_RING, ACCENT_SOFT, ACCENT_TEXT)
        } else {
            (LINE_STRONG, SURFACE_RAISED, TEXT_SECONDARY)
        };
        let base = if who == Skills::Step { 0 } else { 1000 };
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(24.))
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(border)
            .bg(bg)
            .font_family(FONT_MONO)
            .text_size(px(12.))
            .text_color(fg)
            .child(
                div()
                    .id(("skill", base + ix))
                    .pl(px(8.))
                    .pr(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.text_color(TEXT))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_skill(who, toggle_name.clone(), cx)))
                    .child(SharedString::from(if missing { format!("{name} · missing") } else { name.to_string() })),
            )
            .child(
                div()
                    .id(("skill-x", base + ix))
                    .size(px(18.))
                    .mr(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(SURFACE_HOVER))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_skill(who, remove_name.clone(), cx)))
                    .child(icon("x", 10., TEXT_MUTED)),
            )
            .into_any_element()
    }

    /// "Skills in the prompt" / "on demand" chips and the "Add skill" menu.
    fn skills_block(&self, who: Skills, prompt: &[String], demand: &[String], cx: &mut Context<Self>) -> gpui::Div {
        let mut ix = 0;
        let mut chips = |list: &[String], in_prompt: bool, cx: &mut Context<Self>| {
            let els: Vec<_> = list
                .iter()
                .map(|s| {
                    ix += 1;
                    self.skill_chip(who, ix, s, in_prompt, cx)
                })
                .collect();
            if els.is_empty() {
                chip_row().child(div().text_size(px(12.)).text_color(TEXT_MUTED).child("None"))
            } else {
                chip_row().children(els)
            }
        };
        let prompt_chips = chips(prompt, true, cx);
        let demand_chips = chips(demand, false, cx);
        let addable: Vec<String> = self
            .skills
            .iter()
            .filter(|s| !prompt.contains(s) && !demand.contains(s))
            .cloned()
            .collect();
        let menu = if who == Skills::Step { Menu::Skill } else { Menu::DelegateSkill };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(strong("Skills in the prompt"))
            .child(prompt_chips)
            .child(strong("Skills on demand").mt(px(4.)))
            .child(demand_chips)
            .child(self.select_field(
                menu,
                "Add skill…",
                true,
                FULL_W,
                move || {
                    addable
                        .into_iter()
                        .map(|name| {
                            let n = name.clone();
                            Item::new(name, move |this, _, cx| this.add_skill(who, n.clone(), cx))
                        })
                        .collect()
                },
                cx,
            ))
    }

    fn tools_select(&self, menu: Menu, current: Tools, cx: &mut Context<Self>) -> AnyElement {
        let delegate = menu == Menu::DelegateTools;
        self.select_field(
            menu,
            current.label(),
            false,
            FULL_W,
            move || {
                Tools::ALL
                    .into_iter()
                    .map(|t| {
                        Item::new(t.label(), move |this, _, cx| {
                            this.edit_step(cx, |s| {
                                if delegate {
                                    if let Some(d) = s.delegate.as_mut() {
                                        d.tools = t;
                                    }
                                } else {
                                    s.tools = t;
                                }
                            })
                        })
                        .detail(t.names().join(", "))
                        .checked(t == current)
                    })
                    .collect()
            },
            cx,
        )
    }

    fn max_steps_row(&self, value: usize, cx: &mut Context<Self>) -> gpui::Div {
        let steps = Stepper::new("max-steps", value as i32)
            .range(1, 500)
            .on_change(cx.listener(move |this, v: &i32, _, cx| {
                let v = (*v).max(1) as usize;
                this.edit_step(cx, |s| s.max_steps = v);
            }));
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(div().flex().flex_col().child(strong("Max steps")).child(help("Tool calls before it stops")))
            .child(steps)
    }

    fn step_header(&self, def: &PipelineDef, s: &StepDef, snap: &Snap, cx: &mut Context<Self>) -> AnyElement {
        let (ic, bg, fg) = kind_look(s.kind, s.delegate.is_some());
        let counts = snap.counts(Some(&s.id));
        let word = counts.word("done").map(|(w, _)| (w, counts.dot()));
        let i = def.step_index(&s.id).unwrap_or(0);
        let last = def.steps.len().saturating_sub(1);
        let confirming = self.confirm == Some(Confirm::Step(s.id.clone()));
        let refs: Vec<&str> = def
            .steps
            .iter()
            .filter(|x| x.id != s.id && (x.on_no == s.id || x.back_to == s.id))
            .map(|x| x.name.as_str())
            .collect();
        let confirm_text = if refs.is_empty() {
            format!("Delete the step “{}”?", s.name)
        } else {
            format!(
                "Delete the step “{}”? {} {} to it; {} cleared.",
                s.name,
                refs.join(", "),
                if refs.len() == 1 { "jumps" } else { "jump" },
                if refs.len() == 1 { "that jump is" } else { "those jumps are" }
            )
        };
        let id = s.id.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded(px(RADIUS_MD))
                            .bg(bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(ic, 14., fg)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(SharedString::from(s.name.clone())),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(18.))
                                    .text_color(TEXT_MUTED)
                                    .truncate()
                                    .child(SharedString::from(format!("Step {} of {} · {}", i + 1, def.steps.len(), kind_line(s)))),
                            ),
                    )
                    .when_some(word, |d, (w, dot)| d.child(Badge::new(SharedString::from(w)).dot(dot))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        Button::new("step-left", "")
                            .small()
                            .icon("arrow-left")
                            .disabled(i == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.move_selected(-1, cx))),
                    )
                    .child(
                        Button::new("step-right", "")
                            .small()
                            .icon("arrow-right")
                            .disabled(i >= last)
                            .on_click(cx.listener(|this, _, _, cx| this.move_selected(1, cx))),
                    )
                    .child(
                        Button::new("step-duplicate", "Duplicate")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.duplicate_selected(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("step-delete", "Delete…")
                            .small()
                            .variant(ButtonVariant::Danger)
                            .on_click(cx.listener(move |this, _, _, cx| this.ask_confirm(Confirm::Step(id.clone()), cx))),
                    ),
            )
            .when(confirming, |d| {
                let id = s.id.clone();
                d.child(self.confirm_strip(
                    "confirm-step",
                    confirm_text,
                    "Delete step",
                    move |this, cx| this.delete_step(id.clone(), cx),
                    cx,
                ))
            })
            .into_any_element()
    }

    pub(crate) fn ins_step(
        &self,
        def: &PipelineDef,
        s: &StepDef,
        snap: &Snap,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = vec![self.step_header(def, s, snap, cx)];

        // Name, id, workdir.
        let mut basics = section(12.)
            .child(help(s.kind.about()))
            .child(div().child(field_label("Name")).child(self.text_field(Field::StepName, false, window, cx)))
            .child(kv("Id", SharedString::from(s.id.clone()), true));
        if s.kind.uses_workdir() {
            let wd = def.workdir(&s.workdir);
            let current = s.workdir.clone();
            let workdirs = def.workdirs.clone();
            let value = match wd {
                Some(w) => w.name.clone(),
                None if s.workdir.is_empty() => "Choose a workdir…".into(),
                None => format!("{} · missing", s.workdir),
            };
            let hint = match wd {
                Some(w) if w.path.as_os_str().is_empty() => "This workdir has no folder yet.".to_string(),
                Some(w) if w.worktree => format!("A fresh worktree of {} per run.", pipeline::def::tilde(&w.path)),
                Some(w) => format!("{} as is, shared by all runs.", pipeline::def::tilde(&w.path)),
                None => "Steps that work in a folder need a workdir.".into(),
            };
            let select = self.select_field(
                Menu::Workdir,
                value,
                false,
                FULL_W,
                move || {
                    let mut items: Vec<Item> = workdirs
                        .iter()
                        .map(|w| {
                            let id = w.id.clone();
                            let path = if w.path.as_os_str().is_empty() {
                                "no folder".to_string()
                            } else {
                                pipeline::def::tilde(&w.path)
                            };
                            Item::new(w.name.clone(), move |this, _, cx| {
                                let id = id.clone();
                                this.edit_step(cx, |s| s.workdir = id)
                            })
                            .detail(path)
                            .icon("folder", SURFACE_HOVER, TEXT_SECONDARY)
                            .checked(w.id == current)
                        })
                        .collect();
                    items.push(
                        Item::new("Manage workdirs…", |this, _, cx| this.set_tab(Tab::Workdirs, cx))
                            .icon("plus", SURFACE_HOVER, TEXT_MUTED),
                    );
                    items
                },
                cx,
            );
            basics = basics.child(div().child(field_label("Works in")).child(select).child(help(hint).mt(px(6.))));
        }
        out.push(basics.into_any_element());

        match s.kind {
            StepKind::Agent | StepKind::Decision => {
                let mut model = section(12.)
                    .child(div().child(field_label("Model")).child(self.text_field(Field::Model, true, window, cx)));
                if s.kind == StepKind::Decision {
                    let after: Vec<(String, String)> = def
                        .steps
                        .iter()
                        .skip(def.step_index(&s.id).map(|i| i + 1).unwrap_or(0))
                        .map(|x| (x.id.clone(), x.name.clone()))
                        .collect();
                    let current = s.on_no.clone();
                    let value = match s.on_no.as_str() {
                        "" => "Continue with the next step".to_string(),
                        "end" => "End the run".to_string(),
                        id => target_name(def, id),
                    };
                    let on_no = self.select_field(
                        Menu::OnNo,
                        value,
                        false,
                        FULL_W,
                        move || {
                            let pick = |id: String| {
                                move |this: &mut PipelineEditorScreen, _: &mut Window, cx: &mut Context<PipelineEditorScreen>| {
                                    let id = id.clone();
                                    this.edit_step(cx, |s| s.on_no = id)
                                }
                            };
                            let mut items = vec![
                                Item::new("Continue with the next step", pick(String::new())).checked(current.is_empty()),
                            ];
                            for (id, name) in after {
                                let checked = current == id;
                                items.push(Item::new(format!("Skip to {name}"), pick(id)).checked(checked));
                            }
                            items.push(Item::new("End the run", pick("end".into())).checked(current == "end"));
                            items
                        },
                        cx,
                    );
                    model = model
                        .child(
                            div()
                                .child(field_label("Question"))
                                .child(self.long_field(Field::Question, 3., false, &s.question, cx)),
                        )
                        .child(
                            div()
                                .child(field_label("If the answer is no"))
                                .child(on_no)
                                .child(help("Steps in between are skipped.").mt(px(6.))),
                        );
                }
                model = model
                    .child(
                        div()
                            .child(field_label("Instructions"))
                            .child(self.long_field(Field::Instructions, 5., false, &s.instructions, cx)),
                    )
                    .child(div().child(field_label("Tools")).child(self.tools_select(Menu::Tools, s.tools, cx)))
                    .child(self.max_steps_row(s.max_steps, cx));
                out.push(model.into_any_element());
                out.push(
                    section(8.)
                        .child(self.skills_block(Skills::Step, &s.skills, &s.skills_on_demand, cx))
                        .child(help(
                            "Prompt skills are pasted into the system prompt; on-demand skills are listed and loaded with load_skill. Click a skill to move it.",
                        ))
                        .into_any_element(),
                );
                if s.kind == StepKind::Agent {
                    out.push(self.delegate_section(s, snap, window, cx));
                }
            }
            StepKind::Render => out.push(
                section(12.)
                    .child(div().child(field_label("Files")).child(self.text_field(Field::Files, true, window, cx)))
                    .child(help("Changed files matching this glob are rendered to PNG before and after (* within a folder, ** across folders)."))
                    .into_any_element(),
            ),
            StepKind::Approval => {
                let before: Vec<(String, String)> = def
                    .steps
                    .iter()
                    .take(def.step_index(&s.id).unwrap_or(0))
                    .map(|x| (x.id.clone(), x.name.clone()))
                    .collect();
                let current = s.back_to.clone();
                let value = if s.back_to.is_empty() { "Nowhere (approve only)".to_string() } else { target_name(def, &s.back_to) };
                let back = self.select_field(
                    Menu::BackTo,
                    value,
                    false,
                    FULL_W,
                    move || {
                        let pick = |id: String| {
                            move |this: &mut PipelineEditorScreen, _: &mut Window, cx: &mut Context<PipelineEditorScreen>| {
                                let id = id.clone();
                                this.edit_step(cx, |s| s.back_to = id)
                            }
                        };
                        let mut items = vec![Item::new("Nowhere (approve only)", pick(String::new())).checked(current.is_empty())];
                        for (id, name) in before {
                            let checked = current == id;
                            items.push(Item::new(name, pick(id)).checked(checked));
                        }
                        items
                    },
                    cx,
                );
                out.push(
                    section(12.)
                        .child(help("The run stops here until you approve it on the issue. Requesting changes sends your notes back to an earlier step."))
                        .child(div().child(field_label("Send back to")).child(back))
                        .child(self.check_row(
                            "skip-empty",
                            s.skip_if_nothing_to_review,
                            "Skip when there is nothing to review",
                            Some("When the step before rendered nothing, the run goes on."),
                            |this, cx| this.edit_step(cx, |s| s.skip_if_nothing_to_review = !s.skip_if_nothing_to_review),
                            cx,
                        ))
                        .into_any_element(),
                );
            }
            StepKind::Command => out.push(
                section(12.)
                    .child(
                        div()
                            .child(field_label("Command"))
                            .child(self.long_field(Field::Command, 2., true, &s.command, cx)),
                    )
                    .child(help("Runs with bash in the workdir, with its environment."))
                    .child(two_cols(
                        div().child(field_label("Timeout, seconds")).child(self.text_field(Field::Timeout, true, window, cx)),
                        div(),
                    ))
                    .child(self.check_row(
                        "allow-failure",
                        s.allow_failure,
                        "Failure does not stop the run",
                        Some("The output is kept for later steps either way."),
                        |this, cx| this.edit_step(cx, |s| s.allow_failure = !s.allow_failure),
                        cx,
                    ))
                    .into_any_element(),
            ),
            StepKind::Merge => {
                let text = match def.workdir(&s.workdir) {
                    Some(w) if w.worktree => format!(
                        "Commits the run's branch in {} and merges it into {}.",
                        w.name,
                        if w.base_branch.is_empty() { "its base branch" } else { &w.base_branch }
                    ),
                    Some(w) => format!("{} is not a worktree, so there is no run branch to merge.", w.name),
                    None => "Merges the run's branch of a worktree into its base branch.".into(),
                };
                out.push(section(8.).child(para(text)).child(help("Nothing else to configure.")).into_any_element());
            }
            StepKind::PullRequest => out.push(
                section(12.)
                    .child(para("Commits the run's changes, pushes the branch to origin and opens a pull request with gh. The link is posted to your Inbox."))
                    .child(self.check_row(
                        "push",
                        s.push,
                        strong("Push and open a pull request"),
                        Some("Off: the branch stays in the local worktree."),
                        |this, cx| this.edit_step(cx, |s| s.push = !s.push),
                        cx,
                    ))
                    .child(div().when(!s.push, |d| d.opacity(0.45)).child(self.check_row(
                        "draft",
                        s.draft,
                        "Open it as a draft",
                        None,
                        |this, cx| this.edit_step(cx, |s| s.draft = !s.draft),
                        cx,
                    )))
                    .into_any_element(),
            ),
            StepKind::Report => out.push(
                section(8.)
                    .child(para("Posts a summary of the run to your Inbox: what each step reported, the checks, renders and links."))
                    .child(help("Nothing to configure."))
                    .into_any_element(),
            ),
        }

        // Runs at this step right now.
        let here: Vec<_> = snap
            .runs
            .iter()
            .filter(|r| {
                r.stages.iter().any(|st| {
                    st.id == s.id
                        && (matches!(st.status, StageStatus::Waiting) || (st.status == StageStatus::Running && r.active))
                })
            })
            .collect();
        if !here.is_empty() {
            out.push(
                section(4.)
                    .border_b_0()
                    .child(strong("Here now").mb(px(4.)))
                    .children(here.iter().enumerate().map(|(i, r)| {
                        let k = r.key.clone();
                        let (status, color) = r.status_label();
                        div()
                            .id(("here", i))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(28.))
                            .px(px(8.))
                            .mx(px(-8.))
                            .rounded(px(RADIUS_MD))
                            .cursor_pointer()
                            .hover(|st| st.bg(SURFACE_HOVER))
                            .on_click(move |_, _, cx| ui::nav::open_issue(k.clone(), cx))
                            .child(div().flex_none().font_family(FONT_MONO).text_size(px(12.)).text_color(TEXT_MUTED).child(SharedString::from(r.key.clone())))
                            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(r.title.clone())))
                            .child(div().flex_none().text_size(px(12.)).text_color(color).child(SharedString::from(status)))
                    }))
                    .into_any_element(),
            );
        }
        out
    }

    fn delegate_section(&self, s: &StepDef, snap: &Snap, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut sec = section(12.).border_b_0().child(self.check_row(
            "delegate",
            s.delegate.is_some(),
            strong("Sub-agents"),
            Some("The agent hands tasks to sub-agents with delegate and reviews their work instead of editing itself."),
            |this, cx| this.toggle_delegate(cx),
            cx,
        ));
        if let Some(d) = &s.delegate {
            let tasks = snap.task_counts(&s.id);
            sec = sec
                .child(div().flex().child(spawn_chip(format!(
                    "One {} per delegated task{}",
                    d.name,
                    if tasks.done + tasks.running > 0 {
                        format!(" · {} so far", tasks.done + tasks.running)
                    } else {
                        String::new()
                    }
                ))))
                .child(two_cols(
                    div().child(field_label("Name")).child(self.text_field(Field::DelegateName, false, window, cx)),
                    div().child(field_label("Max steps")).child(self.max_steps_row_compact(d.max_steps, cx)),
                ))
                .child(div().child(field_label("Model")).child(self.text_field(Field::DelegateModel, true, window, cx)))
                .child(div().child(field_label("Tools")).child(self.tools_select(Menu::DelegateTools, d.tools, cx)))
                .child(
                    div()
                        .child(field_label("Instructions"))
                        .child(self.long_field(Field::DelegateInstructions, 4., false, &d.instructions, cx)),
                )
                .child(self.skills_block(Skills::Delegate, &d.skills, &d.skills_on_demand, cx));
        }
        sec.into_any_element()
    }

    fn max_steps_row_compact(&self, value: usize, cx: &mut Context<Self>) -> AnyElement {
        Stepper::new("delegate-max-steps", value as i32)
            .range(1, 500)
            .on_change(cx.listener(move |this, v: &i32, _, cx| {
                let v = (*v).max(1) as usize;
                this.edit_step(cx, |s| {
                    if let Some(d) = s.delegate.as_mut() {
                        d.max_steps = v;
                    }
                });
            }))
            .into_any_element()
    }
}
