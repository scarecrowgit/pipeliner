//! Detail sections of agents: a pipeline's model steps and the sub-agents they lead (what
//! they do, model / workdir / max steps / skills editing, skills, usage, tasks, runs), the
//! built-in chat agents and custom chat agents (tools, prompt, skills, recent chats).

use gpui::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*, px};
use pipeline::{StepDef, StepKind};
use ui::Button;
use ui::nav::{ChatRequest, request_chat};
use ui::theme::*;

use crate::detail::{body_text, facts, heading_count, muted_12, section, usage_stats};
use crate::looks::{chip, heading};
use crate::entries::{pipeline_id, step_id, sub_id};
use crate::model::{ChatKind, StepRef, StepRole, Usage, plural};
use crate::skills::SkillMode;
use crate::{AgentsScreen, fmt};

fn chat_button(id: &'static str) -> Button {
    Button::new(id, "Chat with it")
        .small()
        .on_click(|_, _, cx| request_chat(ChatRequest::New, cx))
}

impl AgentsScreen {
    /// Every discovered skill, offered on demand (what chat agents get).
    fn all_skills_on_demand(&self) -> Vec<(String, SkillMode)> {
        self.catalog
            .skills
            .iter()
            .map(|s| (s.name.clone(), SkillMode::OnDemand))
            .collect()
    }

    fn skills_hint_chat(&self) -> String {
        format!(
            "Found in ~/.claude/skills, ~/.config/harness/skills and {}/.claude/skills. Force \
             one into the prompt from the chat's agent picker.",
            fmt::path_label(&self.catalog.root)
        )
    }

    // ── Pipeline steps and sub-agents ───────────────────────────────────

    fn gone(&self) -> Vec<AnyElement> {
        vec![section(6., true).child(muted_12("This step is gone.")).into_any_element()]
    }

    /// "Open pipeline" / "Open in editor" (+ one more button).
    fn step_actions(&self, pipeline: &str, extra: Option<Button>, cx: &mut Context<Self>) -> AnyElement {
        let id = pipeline_id(pipeline);
        let editor_id = pipeline.to_string();
        let mut buttons = vec![
            Button::new("agents-step-pipeline", "Open pipeline")
                .small()
                .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx))),
            Button::new("agents-step-editor", "Open in editor")
                .small()
                .on_click(move |_, _, cx| ui::nav::open_pipeline(editor_id.clone(), cx)),
        ];
        buttons.extend(extra);
        self.actions(buttons).into_any_element()
    }

    fn usage_section(&self, u: &Usage) -> AnyElement {
        section(10., false)
            .child(heading("Usage"))
            .child(usage_stats(u.runs, u.tokens, u.cost))
            .when(!u.models.is_empty(), |d| {
                d.child(facts(
                    vec![(
                        if u.models.len() == 1 { "Model used" } else { "Models used" },
                        u.models.join(", ").into(),
                    )],
                    true,
                ))
            })
            .when(u.tokens > 0, |d| {
                d.child(muted_12("Over the latest run of each issue."))
            })
            .into_any_element()
    }

    fn skill_list(skills: &[String], on_demand: &[String]) -> Vec<(String, SkillMode)> {
        skills
            .iter()
            .map(|s| (s.clone(), SkillMode::Prompt))
            .chain(on_demand.iter().map(|s| (s.clone(), SkillMode::OnDemand)))
            .collect()
    }

    pub(crate) fn step_sections(
        &self,
        r: StepRef,
        role: StepRole,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let snap = &self.snap;
        let Some((def, step)) = snap.step(r) else {
            return self.gone();
        };
        let (def, step): (_, StepDef) = (def.clone(), step.clone());
        let sub = step.delegate.as_ref().map(|d| {
            let id = sub_id(&def.id, &step.id);
            Button::new("agents-step-sub", format!("{}s", d.name))
                .small()
                .on_click(cx.listener(move |this, _, _, cx| this.select(id.clone(), cx)))
        });
        let actions = self.step_actions(&def.id, sub, cx);

        let what = match step.kind {
            StepKind::Decision => step.question.trim().to_string(),
            _ => fmt::plain_markdown(&step.instructions),
        };
        let mut rows = vec![
            ("Pipeline", SharedString::from(def.name.clone())),
            (
                "Position",
                format!(
                    "Step {} of {}",
                    r.step + 1,
                    def.steps.len()
                )
                .into(),
            ),
            ("Tools", step.tools.label().into()),
        ];
        if step.kind == StepKind::Decision && !step.on_no.is_empty() {
            let to = if step.on_no == "end" {
                "Ends the run".to_string()
            } else {
                def.step(&step.on_no)
                    .map_or_else(|| step.on_no.clone(), |s| format!("Jumps to {}", s.name))
            };
            rows.push(("On “no”", to.into()));
        }
        if let Some(d) = &step.delegate {
            rows.push(("Leads", format!("{}s ({})", d.name, fmt::short_model(&d.model)).into()));
        }
        let about = section(8., false)
            .child(heading(if role == StepRole::Decision { "Question" } else { "Instructions" }))
            .child(if what.is_empty() {
                muted_12("None written; it works from the issue alone.")
            } else {
                div()
                    .max_h(px(10. * 20.))
                    .overflow_hidden()
                    .child(body_text(what))
            })
            .child(facts(rows, false));

        let mut settings = section(12., false)
            .child(heading("Settings"))
            .children(self.settings_fields(window, cx).into_iter().take(2));
        settings = settings.child(self.workdir_field(&def, &step, cx));
        let settings = settings
            .children(self.settings_fields(window, cx).into_iter().skip(2))
            .when_some(self.editor.error(), |d, e| {
                d.child(div().text_size(px(12.)).text_color(DANGER).child(e))
            })
            .child(muted_12(format!(
                "Saved to {}; new runs use it.",
                fmt::path_label(&def.path())
            )));

        let skills = self.detail_skills(
            Self::skill_list(&step.skills, &step.skills_on_demand),
            "Edit the lists in Settings above.",
            "No skills. It only knows what its instructions say.",
            cx,
        );

        let mut out = vec![
            actions,
            about.into_any_element(),
            settings.into_any_element(),
            skills.into_any_element(),
            self.usage_section(&snap.step_usage(r)),
        ];
        if let Some(d) = &step.delegate {
            out.push(
                self.tasks_section(
                    "Tasks it handed out",
                    snap.tasks_of(r),
                    if d.name.is_empty() { "No tasks yet." } else { "No tasks handed out yet." },
                    cx,
                )
                .into_any_element(),
            );
        }
        out.push(
            self.runs_section(
                "agents-step-run",
                snap.runs_of_step(r),
                "It has not worked on an issue yet.",
            )
            .border_b_0()
            .into_any_element(),
        );
        out
    }

    pub(crate) fn delegate_sections(
        &self,
        r: StepRef,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let snap = &self.snap;
        let Some((def, step, d)) = snap.delegate(r) else {
            return self.gone();
        };
        let (def, step, d) = (def.clone(), step.clone(), d.clone());
        let lead = step_id(&def.id, &step.id);
        let lead_button = Button::new("agents-sub-lead", format!("Open {}", step.name))
            .small()
            .on_click(cx.listener(move |this, _, _, cx| this.select(lead.clone(), cx)));
        let actions = self.step_actions(&def.id, Some(lead_button), cx);

        let workdir = def
            .workdir(&step.workdir)
            .map_or_else(|| "No workdir".to_string(), |w| w.name.clone());
        let instr = fmt::plain_markdown(&d.instructions);
        let about = section(8., false)
            .child(body_text(format!(
                "{} hands it one task at a time; each task gets a fresh {} ({} 1, {} 2…).",
                step.name, d.name, d.name, d.name
            )))
            .when(!instr.is_empty(), |s| {
                s.child(
                    div()
                        .max_h(px(10. * 20.))
                        .overflow_hidden()
                        .child(body_text(instr)),
                )
            })
            .child(facts(
                vec![
                    ("Pipeline", def.name.clone().into()),
                    ("Led by", step.name.clone().into()),
                    ("Works in", format!("{workdir} (the lead's workdir)").into()),
                    ("Tools", d.tools.label().into()),
                ],
                false,
            ));

        let settings = section(12., false)
            .child(heading("Settings"))
            .children(self.settings_fields(window, cx))
            .when_some(self.editor.error(), |s, e| {
                s.child(div().text_size(px(12.)).text_color(DANGER).child(e))
            })
            .child(muted_12(format!(
                "Saved to {}; new runs use it.",
                fmt::path_label(&def.path())
            )));

        let skills = self.detail_skills(
            Self::skill_list(&d.skills, &d.skills_on_demand),
            "Edit the lists in Settings above.",
            "No skills. It only knows what its task says.",
            cx,
        );

        vec![
            actions,
            about.into_any_element(),
            settings.into_any_element(),
            skills.into_any_element(),
            self.usage_section(&snap.delegate_usage(r)),
            self.tasks_section("Tasks it worked on", snap.tasks_of(r), "No tasks yet.", cx)
                .into_any_element(),
            self.runs_section(
                "agents-sub-run",
                snap.runs_of_delegate(r),
                "It has not worked on an issue yet.",
            )
            .border_b_0()
            .into_any_element(),
        ]
    }

    // ── Chat agents ─────────────────────────────────────────────────────

    pub(crate) fn chat_sections(&self, kind: ChatKind, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let actions = self.actions(vec![chat_button("agents-chat")]);
        let about = section(8., false).child(body_text(kind.duty())).child(facts(
            vec![
                ("Tools", kind.tools().into()),
                ("Model", "Picked in the chat".into()),
            ],
            false,
        ));
        let skills = self.detail_skills(
            self.all_skills_on_demand(),
            self.skills_hint_chat(),
            "No skills found in the skill folders.",
            cx,
        );
        vec![
            actions.into_any_element(),
            about.into_any_element(),
            skills.into_any_element(),
            self.chats_section(kind.name())
                .border_b_0()
                .into_any_element(),
        ]
    }

    pub(crate) fn custom_sections(&self, ix: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(a) = self.catalog.agents.get(ix) else {
            return vec![
                section(6., true)
                    .child(muted_12("This agent is gone."))
                    .into_any_element(),
            ];
        };
        let actions = self.actions(vec![chat_button("agents-custom-chat")]);

        let about = section(8., false)
            .when(!a.description.is_empty(), |d| {
                d.child(body_text(a.description.clone()))
            })
            .child(facts(
                vec![
                    (
                        "Model",
                        a.model
                            .clone()
                            .unwrap_or_else(|| "The chat's model".into())
                            .into(),
                    ),
                    ("File", fmt::path_label(&a.path).into()),
                ],
                false,
            ));

        let tools = section(8., false)
            .child(heading_count(
                "Tools",
                a.tools
                    .as_ref()
                    .map(|t| format!("{} tool{}", t.len(), plural(t.len())))
                    .unwrap_or_default(),
            ))
            .child(match &a.tools {
                None => muted_12("No tools list, so it gets the chat's default tools."),
                Some(t) if t.is_empty() => muted_12("An empty tools list: no tools."),
                Some(t) => div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.))
                    .children(t.iter().map(|name| chip(name.clone(), true))),
            });

        // Skills come with the `skill` tool: always without a tools list, else when granted.
        let has_skills = a
            .tools
            .as_ref()
            .is_none_or(|t| agent_tools::agents::map_tools(t).has(agent_tools::skills::SKILL_TOOL));
        let skills = if has_skills {
            self.detail_skills(
                self.all_skills_on_demand(),
                self.skills_hint_chat(),
                "No skills found in the skill folders.",
                cx,
            )
            .into_any_element()
        } else {
            section(8., false)
                .child(heading("Skills"))
                .child(muted_12("Its tools list has no Skill tool, so it loads no skills."))
                .into_any_element()
        };

        let prompt = section(8., false)
            .child(heading("Instructions"))
            .child(if a.prompt.is_empty() {
                muted_12("The file has no instructions after the frontmatter.")
            } else {
                div()
                    .max_h(px(20. * 16.))
                    .overflow_hidden()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(TEXT_SECONDARY)
                    .child(SharedString::from(fmt::plain_markdown(&a.prompt)))
            });

        vec![
            actions.into_any_element(),
            about.into_any_element(),
            tools.into_any_element(),
            skills,
            prompt.into_any_element(),
            self.chats_section(&a.name).border_b_0().into_any_element(),
        ]
    }
}
