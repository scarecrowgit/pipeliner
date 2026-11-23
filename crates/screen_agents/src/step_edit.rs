//! Editing a model step or its sub-agents from the detail panel: model, workdir (one of the
//! pipeline's workdirs), max steps and skills. Each change is saved with
//! [`pipeline::Pipelines::save_pipeline`] on a modified copy of the pipeline; runs in progress
//! keep the version they started with.

use gpui::{
    App, Context, Div, Entity, IntoElement, MouseButton, SharedString, Subscription, Window,
    deferred, div, prelude::*, px,
};
use pipeline::{PipelineDef, Pipelines, StepDef};
use ui::theme::*;
use ui::{TextInput, TextInputEvent, icon, motion};

use crate::model::Kind;
use crate::{AgentsScreen, fmt};

/// What the fields edit: a step (`sub: false`) or the sub-agents it leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub pipeline: String,
    pub step: String,
    pub sub: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Model,
    MaxSteps,
    Skills,
    OnDemand,
}

impl Field {
    const ALL: [Field; 4] = [Field::Model, Field::MaxSteps, Field::Skills, Field::OnDemand];

    fn placeholder(self) -> &'static str {
        match self {
            Field::Model => "provider/model",
            Field::MaxSteps => "60",
            Field::Skills => "None",
            Field::OnDemand => "None",
        }
    }
}

/// The field values of a target, as text.
#[derive(Clone, Debug, Default, PartialEq)]
struct Values([String; 4]);

impl Values {
    fn of(def: &PipelineDef, t: &Target) -> Option<Values> {
        let step = def.step(&t.step)?;
        let (model, max, skills, on_demand) = if t.sub {
            let d = step.delegate.as_ref()?;
            (&d.model, d.max_steps, &d.skills, &d.skills_on_demand)
        } else {
            (&step.model, step.max_steps, &step.skills, &step.skills_on_demand)
        };
        Some(Values([
            model.clone(),
            max.to_string(),
            skills.join(", "),
            on_demand.join(", "),
        ]))
    }

    fn get(&self, f: Field) -> &str {
        &self.0[f as usize]
    }
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', ' ', '\n'])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) struct StepEditor {
    pub pipes: Entity<Pipelines>,
    inputs: Vec<(Field, Entity<TextInput>)>,
    /// What the inputs were loaded for, and the values loaded.
    target: Option<Target>,
    loaded: Values,
    /// The workdir menu is open.
    wd_menu: bool,
    /// Last save error.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

fn label(text: &'static str) -> Div {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(TEXT_MUTED)
        .child(text)
}

fn hint(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .text_color(TEXT_MUTED)
        .child(text.into())
}

impl StepEditor {
    pub fn new(window: &mut Window, cx: &mut Context<AgentsScreen>) -> Self {
        let pipes = pipeline::global(cx).unwrap_or_else(|| pipeline::init(cx));
        let mut subscriptions = vec![cx.observe(&pipes, |this: &mut AgentsScreen, _, cx| {
            this.refresh(cx);
        })];
        let inputs: Vec<(Field, Entity<TextInput>)> = Field::ALL
            .iter()
            .map(|&f| (f, cx.new(|cx| TextInput::new(f.placeholder(), cx))))
            .collect();
        for (field, input) in &inputs {
            let field = *field;
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this: &mut AgentsScreen, input, ev: &TextInputEvent, window, cx| match ev {
                    TextInputEvent::Submit | TextInputEvent::Blur => {
                        let text = input.read(cx).text().trim().to_string();
                        this.save_field(field, text, cx);
                        if *ev == TextInputEvent::Submit {
                            window.blur();
                        }
                    }
                    TextInputEvent::Cancel => {
                        let text = this.editor.loaded.get(field).to_string();
                        input.update(cx, |i, cx| i.set_text(text, cx));
                        window.blur();
                    }
                    _ => {}
                },
            ));
        }
        Self {
            pipes,
            inputs,
            target: None,
            loaded: Values::default(),
            wd_menu: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn input(&self, f: Field) -> Entity<TextInput> {
        self.inputs[f as usize].1.clone()
    }

    /// Close the workdir menu (selection changed; the caller re-renders).
    pub fn close_menu(&mut self) {
        self.wd_menu = false;
    }

    pub fn menu_open(&self) -> bool {
        self.wd_menu
    }

    pub fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

impl AgentsScreen {
    /// Point the fields at the selected entry; when it is the same one, follow changes made
    /// elsewhere except in a field the user has edited.
    pub(crate) fn sync_editor(&mut self, cx: &mut App) {
        let target = self.selected().and_then(|e| {
            let sub = matches!(e.kind, Kind::Delegate(_));
            let (def, step) = self.snap.step(e.kind.step_ref()?)?;
            Some(Target {
                pipeline: def.id.clone(),
                step: step.id.clone(),
                sub,
            })
        });
        let values = target.as_ref().and_then(|t| {
            let def = self.snap.pipelines.iter().find(|d| d.id == t.pipeline)?;
            Values::of(def, t)
        });
        let ed = &mut self.editor;
        let same = ed.target == target;
        if !same {
            ed.error = None;
            ed.wd_menu = false;
        }
        let values = values.unwrap_or_default();
        for (f, input) in &ed.inputs {
            let now = values.get(*f).to_string();
            let edited = same && input.read(cx).text() != ed.loaded.get(*f);
            if !edited && input.read(cx).text() != now {
                input.update(cx, |i, cx| i.set_text(now, cx));
            }
        }
        ed.target = target;
        ed.loaded = values;
    }

    /// Change the target step in a copy of its pipeline and save it.
    fn edit_step(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut StepDef, bool)) {
        let Some(t) = self.editor.target.clone() else {
            return;
        };
        let pipes = self.editor.pipes.clone();
        let saved = pipes.update(cx, |p, cx| {
            let Some(mut def) = p.pipeline(&t.pipeline).cloned() else {
                return Err("the pipeline is gone".to_string());
            };
            let before = def.clone();
            let Some(step) = def.steps.iter_mut().find(|s| s.id == t.step) else {
                return Err("the step is gone".to_string());
            };
            f(step, t.sub);
            if def == before {
                return Ok(());
            }
            p.save_pipeline(def, cx).map_err(|e| format!("{e:#}"))
        });
        self.editor.error = saved
            .err()
            .map(|e| format!("Could not save the pipeline: {e}").into());
        cx.notify();
    }

    fn save_field(&mut self, field: Field, text: String, cx: &mut Context<Self>) {
        if self.editor.target.is_none() || text == self.editor.loaded.get(field) {
            return;
        }
        let problem = match field {
            Field::Model if !text.contains('/') => Some("The model must be provider/model; it was not saved."),
            Field::MaxSteps if text.parse::<usize>().map_or(true, |n| n == 0 || n > 1000) => {
                Some("Max steps must be a number from 1 to 1000; it was not saved.")
            }
            _ => None,
        };
        if let Some(p) = problem {
            self.editor.error = Some(p.into());
            cx.notify();
            return;
        }
        self.edit_step(cx, move |step, sub| {
            let (model, max, skills, on_demand) = match (sub, step.delegate.as_mut()) {
                (true, Some(d)) => (
                    &mut d.model,
                    &mut d.max_steps,
                    &mut d.skills,
                    &mut d.skills_on_demand,
                ),
                (true, None) => return,
                (false, _) => (
                    &mut step.model,
                    &mut step.max_steps,
                    &mut step.skills,
                    &mut step.skills_on_demand,
                ),
            };
            match field {
                Field::Model => *model = text,
                Field::MaxSteps => *max = text.parse().unwrap_or(*max),
                Field::Skills => *skills = split_list(&text),
                Field::OnDemand => *on_demand = split_list(&text),
            }
        });
    }

    fn set_workdir(&mut self, id: String, cx: &mut Context<Self>) {
        self.editor.wd_menu = false;
        self.edit_step(cx, move |step, _| step.workdir = id);
    }

    fn toggle_wd_menu(&mut self, cx: &mut Context<Self>) {
        self.editor.wd_menu = !self.editor.wd_menu;
        cx.notify();
    }

    pub(crate) fn close_wd_menu(&mut self, cx: &mut Context<Self>) {
        if self.editor.wd_menu {
            self.editor.wd_menu = false;
            cx.notify();
        }
    }

    // ── Pieces for the detail panel ────────────────────────────────────

    fn text_field(&self, field: Field, title: &'static str, note: &'static str, mono: bool, window: &Window, cx: &App) -> Div {
        let input = self.editor.input(field);
        let focused = input.read(cx).is_focused(window);
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label(title))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(CONTROL_HEIGHT_SM + 4.))
                    .px(px(10.))
                    .rounded(px(RADIUS_LG))
                    .border_1()
                    .border_color(if focused { ACCENT_STRONG } else { LINE_STRONG })
                    .bg(SURFACE_RAISED)
                    .when(mono, |d| d.font_family(FONT_MONO).text_size(px(TEXT_MONO)))
                    .when(!mono, |d| d.text_size(px(12.)))
                    .child(input),
            )
            .when(!note.is_empty(), |d| d.child(hint(note)))
    }

    /// Model, max steps and skills fields of the target.
    pub(crate) fn settings_fields(&self, window: &Window, cx: &App) -> Vec<Div> {
        vec![
            self.text_field(
                Field::Model,
                "Model",
                "provider/model · Enter saves, Esc restores",
                true,
                window,
                cx,
            ),
            self.text_field(Field::MaxSteps, "Max steps", "Model turns before it must finish", true, window, cx),
            self.text_field(
                Field::Skills,
                "Skills in the prompt",
                "Skill names, comma separated; their full text goes into the prompt",
                true,
                window,
                cx,
            ),
            self.text_field(
                Field::OnDemand,
                "Skills on demand",
                "Listed; the agent loads one when the task needs it",
                true,
                window,
                cx,
            ),
        ]
    }

    /// "Works in": one of the pipeline's workdirs.
    pub(crate) fn workdir_field(&self, def: &PipelineDef, step: &StepDef, cx: &mut Context<Self>) -> Div {
        let open = self.editor.wd_menu;
        let current = def.workdir(&step.workdir);
        let text = match current {
            Some(w) => w.name.clone(),
            None if step.workdir.is_empty() => "No workdir".into(),
            None => format!("Missing workdir `{}`", step.workdir),
        };
        let path = current.map(|w| fmt::path_label(&w.path));
        let items: Vec<(String, String, String)> = def
            .workdirs
            .iter()
            .map(|w| (w.id.clone(), w.name.clone(), fmt::path_label(&w.path)))
            .collect();
        let selected = step.workdir.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(label("Works in"))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .id("wd-select")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .h(px(CONTROL_HEIGHT_SM + 4.))
                            .px(px(10.))
                            .rounded(px(RADIUS_LG))
                            .border_1()
                            .border_color(if open { ACCENT_STRONG } else { LINE_STRONG })
                            .bg(SURFACE_RAISED)
                            .cursor_pointer()
                            .hover(|s| s.bg(SURFACE_HOVER))
                            .child(icon("folder", 12., TEXT_MUTED))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .line_clamp(1)
                                    .text_size(px(12.))
                                    .text_color(if current.is_some() { TEXT } else { WARNING })
                                    .child(SharedString::from(text)),
                            )
                            .child(icon("chevron-down", 12., TEXT_MUTED))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_wd_menu(cx))),
                    )
                    .when(open, |d| d.child(self.wd_menu(items, selected, cx))),
            )
            .child(hint(match path {
                Some(p) => format!("{p} · workdirs are set in the pipeline editor"),
                None => "Pick one of the pipeline's workdirs".into(),
            }))
    }

    fn wd_menu(
        &self,
        items: Vec<(String, String, String)>,
        selected: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = items.is_empty();
        deferred(motion::pop_in(
            "wd-menu-in",
            motion::Pop::Below,
            div()
                .id("wd-menu")
                .absolute()
                .top(px(CONTROL_HEIGHT_SM + 8.))
                .left_0()
                .right_0()
                .p(px(4.))
                .flex()
                .flex_col()
                .rounded(px(RADIUS_LG))
                .border_1()
                .border_color(LINE_STRONG)
                .bg(SURFACE_RAISED)
                .shadow(popover_shadow())
                .occlude()
                .when(empty, |d| {
                    d.child(
                        div()
                            .p(px(8.))
                            .text_size(px(12.))
                            .text_color(TEXT_MUTED)
                            .child("The pipeline has no workdirs; add one in the editor."),
                    )
                })
                .children(items.into_iter().enumerate().map(|(i, (id, name, path))| {
                    let on = id == selected;
                    let row_id = SharedString::from(format!("wd-item-{i}"));
                    let row = div()
                        .id(row_id.clone())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(RADIUS_SM))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .child(icon("folder", 12., TEXT_MUTED))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(SharedString::from(name))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .line_clamp(1)
                                        .font_family(FONT_MONO)
                                        .text_size(px(TEXT_MONO_SM))
                                        .text_color(TEXT_MUTED)
                                        .child(SharedString::from(path)),
                                ),
                        )
                        .when(on, |d| d.child(icon("check", 12., ACCENT_TEXT)))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_workdir(id.clone(), cx)));
                    motion::fade_row(row, row_id, false, SURFACE_HOVER)
                })),
        ))
        .with_priority(2)
    }

    /// Closes the workdir menu on any press outside it.
    pub(crate) fn render_wd_backdrop(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        self.editor.menu_open().then(|| {
            deferred(
                div()
                    .id("wd-menu-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close_wd_menu(cx)),
                    ),
            )
            .with_priority(1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_and_lists() {
        let def = pipeline::config::legacy_default();
        let step = Target { pipeline: def.id.clone(), step: "build".into(), sub: false };
        let sub = Target { sub: true, ..step.clone() };
        let v = Values::of(&def, &step).unwrap();
        assert_eq!(v.get(Field::MaxSteps), "80");
        assert!(v.get(Field::OnDemand).contains("gpui-elements, "));
        assert!(Values::of(&def, &sub).is_some());
        let triage = Target { step: "triage".into(), sub: true, ..step };
        assert!(Values::of(&def, &triage).is_none());
        assert_eq!(split_list("a, b  c,,"), ["a", "b", "c"]);
    }
}
