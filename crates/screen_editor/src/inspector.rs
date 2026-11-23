//! Right-hand inspector: pipeline settings when nothing is selected, otherwise the
//! editor for the selected node's kind (port of the mockup's `ins*` branches).

use gpui::{
    AnyElement, Context, ElementId, Entity, Focusable, FontWeight, IntoElement, MouseButton,
    SharedString, Window, anchored, deferred, div, point, prelude::*, px,
};
use ui::{
    Badge, Button, ButtonVariant, Stepper, TextInput, Tone, checkbox, field_box, icon, kbd,
    progress, rule_chip, select_box, spawn_chip, theme::*,
};

use crate::data::*;
use crate::widgets::*;
use crate::{Area, Menu, PipelineEditorScreen};

/// Inspector width (`flex: 1 1 320px`).
const INSPECTOR_W: f32 = 320.;
/// Full content width inside the 20px side padding.
const FULL_W: f32 = INSPECTOR_W - 40.;
/// One column of a two-column grid.
const HALF_W: f32 = (FULL_W - 12.) / 2.;

impl PipelineEditorScreen {
    pub(crate) fn render_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let node = self.selected().cloned();
        let body = match &node {
            None => self.ins_pipeline(cx),
            Some(n) => match n.kind() {
                Kind::Agent => self.ins_agent(n, cx),
                Kind::Spawn => self.ins_spawn(n, cx),
                Kind::Orch => self.ins_orch(n, cx),
                Kind::Pipe => self.ins_pipe(n, cx),
                Kind::Source => self.ins_source(n, cx),
                Kind::Router => self.ins_router(n),
                Kind::File => self.ins_file(n, cx),
                Kind::Gate => self.ins_gate(cx),
                Kind::Action => self.ins_action(n, cx),
            },
        };
        // Inner column keeps its natural height so the outer panel scrolls.
        let mut content = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .child(self.ins_header(node.as_ref()))
            .children(body);
        if node.is_some() {
            content = content.child(
                div().px(px(20.)).py(px(16.)).child(
                    Button::new("remove-node", "Remove node")
                        .variant(ButtonVariant::Danger)
                        .icon("x")
                        .on_click(cx.listener(|this, _, _, cx| this.remove_selected(cx))),
                ),
            );
        }
        if self.open_menu.is_some() {
            // Click-away backdrop below the open dropdown.
            let vp = window.viewport_size();
            content = content.child(
                deferred(
                    anchored().position(point(px(0.), px(0.))).child(
                        div()
                            .id("menu-backdrop")
                            .w(vp.width)
                            .h(vp.height)
                            .occlude()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.open_menu = None;
                                    cx.notify();
                                }),
                            ),
                    ),
                )
                .with_priority(1),
            );
        }
        div()
            .id("inspector-scroll")
            .w(px(INSPECTOR_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .bg(BG_SIDEBAR)
            .child(content)
    }

    fn ins_header(&self, node: Option<&Node>) -> impl IntoElement + use<> {
        let (title, kind_label, icon_name, bg, fg, status) = match node {
            None => (
                self.pipeline_name.clone(),
                "Pipeline · draft v15",
                "pipelines",
                ACCENT_SOFT,
                ACCENT_TEXT,
                None,
            ),
            Some(n) => {
                let info = n.kind().info();
                let st = self.status_of(&n.id);
                (
                    n.title.clone(),
                    info.label,
                    info.icon,
                    info.tile_bg,
                    info.tile_fg,
                    Some(st),
                )
            }
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(20.))
            .py(px(16.))
            .border_b_1()
            .border_color(LINE)
            .child(
                div()
                    .size(px(28.))
                    .flex_none()
                    .rounded(px(RADIUS_MD))
                    .bg(bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icon_name, 14., fg)),
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
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(TEXT_MUTED)
                            .child(kind_label),
                    ),
            )
            .when_some(status.filter(|s| !s.word().is_empty()), |d, s| {
                d.child(Badge::new(s.word()).dot(s.dot()))
            })
    }

    // ── Controls ────────────────────────────────────────────────────────

    /// Text field box hosting a `TextInput`; clicking anywhere in the box focuses it.
    fn input_field(
        &self,
        key: &'static str,
        input: &Entity<TextInput>,
        mono: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let focus = input.clone();
        field_box()
            .id(key)
            .cursor_text()
            .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
            .on_click(cx.listener(move |_, _, window, cx| focus.focus_handle(cx).focus(window)))
            .child(input.clone())
    }

    /// Multi-line field (`<textarea rows=N>`): shows the wrapped text; while editing it
    /// hosts the single-line `TextInput`.
    fn textarea(
        &self,
        area: Area,
        input: &Entity<TextInput>,
        text: &str,
        rows: usize,
        mono: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let editing = self.editing_area == Some(area);
        let focus = input.clone();
        div()
            .id(("textarea", area as usize))
            .min_h(px(rows as f32 * 20. + 22.))
            .px(px(12.))
            .py(px(10.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(if editing { ACCENT_STRONG } else { LINE_STRONG })
            .rounded(px(RADIUS_LG))
            .line_height(px(20.))
            .cursor_text()
            .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
            .when(editing, |d| {
                d.child(div().flex().h(px(20.)).child(input.clone()))
            })
            .when(!editing, |d| d.child(SharedString::from(text.to_string())))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.editing_area = Some(area);
                focus.focus_handle(cx).focus(window);
                cx.notify();
            }))
    }

    /// Select box with a dropdown of `options`; picking calls [`Self::pick`].
    #[allow(clippy::too_many_arguments)]
    fn select_field(
        &self,
        menu: Menu,
        value: impl Into<SharedString>,
        options: Vec<SharedString>,
        current: Option<usize>,
        mono: bool,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.open_menu == Some(menu);
        let list = div()
            .id(("menu", menu as usize))
            .w(px(width))
            .flex()
            .flex_col()
            .p(px(4.))
            .bg(SURFACE_RAISED)
            .border_1()
            .border_color(LINE_STRONG)
            .rounded(px(RADIUS_LG))
            .shadow(popover_shadow())
            .occlude()
            .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
            .children(options.into_iter().enumerate().map(|(i, label)| {
                let sel = current == Some(i);
                div()
                    .id(("option", i))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .h(px(32.))
                    .px(px(8.))
                    .rounded(px(RADIUS_MD))
                    .cursor_pointer()
                    .text_color(if sel { TEXT } else { TEXT_SECONDARY })
                    .hover(|s| s.bg(SURFACE_HOVER).text_color(TEXT))
                    .child(div().truncate().child(label))
                    .when(sel, |d| d.child(icon("check", 12., ACCENT_TEXT)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(menu, i, cx)))
            }));
        div()
            .flex()
            .flex_col()
            .child(
                select_box(ElementId::from(("select", menu as usize)), value)
                    .px(px(10.))
                    .when(open, |d| d.border_color(ACCENT_STRONG))
                    .when(mono, |d| d.font_family(FONT_MONO).text_size(px(12.)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_menu = if this.open_menu == Some(menu) {
                            None
                        } else {
                            Some(menu)
                        };
                        cx.notify();
                    })),
            )
            .when(open, |d| {
                d.child(
                    div().relative().h_0().child(
                        deferred(
                            anchored()
                                .offset(point(px(0.), px(4.)))
                                .snap_to_window()
                                .child(ui::motion::pop_in(
                                    ("menu-in", menu as usize),
                                    ui::motion::Pop::Below,
                                    list,
                                )),
                        )
                        .with_priority(2),
                    ),
                )
            })
            .into_any_element()
    }

    fn pick(&mut self, menu: Menu, i: usize, cx: &mut Context<Self>) {
        match menu {
            Menu::Library => {
                let title = LIBRARY[i].0.to_string();
                self.update_sel(|n| n.title = title.clone());
                self.name_input
                    .update(cx, |input, cx| input.set_text(title, cx));
            }
            Menu::Model => self.update_sel(|n| n.model = MODELS[i].into()),
            Menu::Every => self.update_sel(|n| n.every = POLL_EVERY[i].into()),
            Menu::PipeRef => {
                let r = PIPELINES[i].0.to_string();
                self.update_sel(|n| {
                    n.reference = r.clone();
                    n.title = r.clone();
                });
                self.name_input
                    .update(cx, |input, cx| input.set_text(r, cx));
            }
            Menu::Reviewers => self.reviewers = i,
            Menu::Nudge => self.nudge = i,
        }
        self.open_menu = None;
        cx.notify();
    }

    fn check_row(
        &self,
        key: &'static str,
        checked: bool,
        label: impl IntoElement,
        toggle: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(key)
            .flex()
            .items_center()
            .gap(px(10.))
            .min_h(px(28.))
            .cursor_pointer()
            .child(checkbox(
                ElementId::Name(format!("{key}-box").into()),
                checked,
            ))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                toggle(this);
                cx.notify();
            }))
            .into_any_element()
    }

    fn radio_row(
        &self,
        key: &'static str,
        checked: bool,
        label: impl IntoElement,
        set: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(key)
            .flex()
            .items_center()
            .gap(px(8.))
            .min_h(px(28.))
            .cursor_pointer()
            .child(radio(checked))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                set(this);
                cx.notify();
            }))
            .into_any_element()
    }

    fn name_field(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div().child(field_label("Name")).child(self.input_field(
            "name-field",
            &self.name_input.clone(),
            false,
            cx,
        ))
    }

    // ── Bodies per kind ─────────────────────────────────────────────────

    fn ins_pipeline(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            section(12.)
                .child(
                    div()
                        .child(field_label("Pipeline name"))
                        .child(self.input_field("pipeline-name", &self.pipeline_input.clone(), false, cx)),
                )
                .child(kv("Trigger", "Jira puller · every 2m", true))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(div().text_color(TEXT_MUTED).child("Agents at once"))
                        .child(Stepper::new("agents-at-once", self.agents_at_once).range(1, 10).on_change(
                            cx.listener(|this, v: &i32, _, cx| {
                                this.agents_at_once = *v;
                                cx.notify();
                            }),
                        )),
                )
                .into_any_element(),
            section(8.)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(strong("Budget today"))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_SECONDARY)
                                .child("$3.81 of $10.00"),
                        ),
                )
                .child(progress(0.381, ACCENT))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .mt(px(8.))
                        .child(strong("Spawn policy"))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(SPAWN_TEXT)
                                .child("depth ≤2 · ≤3 per agent"),
                        ),
                )
                .child(help(
                    "Agents may create sub-agents inside these limits. Anything deeper asks a person first.",
                ))
                .into_any_element(),
            section(8.)
                .child(strong("Use this pipeline as an agent"))
                .child(
                    chip_row()
                        .child(Badge::new("Chat").tone(Tone::Accent))
                        .child(Badge::new("Issues").tone(Tone::Accent))
                        .child(Badge::new("Inside pipelines").tone(Tone::Accent))
                        .child(Badge::new("Autopilots").tone(Tone::Accent)),
                )
                .child(help(
                    "In chat it sits next to Build and Plan. Payments lead has it in its team. Takes a message or an issue, returns variants to review.",
                ))
                .into_any_element(),
            section(8.)
                .border_b_0()
                .child(strong("Changes in v15"))
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(22.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(diff_add("+ Analyst variants 1 → 2"))
                        .child(diff_add("+ Fixer may spawn Test writer")),
                )
                .child(help("Select a node on the canvas to edit it."))
                .into_any_element(),
        ]
    }

    fn ins_agent(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let outs: Vec<&Node> = self
            .edges
            .iter()
            .filter(|e| e.from == n.id && e.port == -1)
            .filter_map(|e| self.by_id(&e.to))
            .collect();
        let ins: Vec<&Node> = self
            .edges
            .iter()
            .filter(|e| e.to == n.id)
            .filter_map(|e| self.by_id(&e.from))
            .collect();
        let mut inputs: Vec<String> = ins
            .iter()
            .filter(|m| m.kind() == Kind::File)
            .map(|m| m.glob.clone())
            .collect();
        inputs.push("issue.json".into());
        let out_files: Vec<String> = outs
            .iter()
            .filter(|m| m.kind() == Kind::File)
            .map(|m| m.glob.clone())
            .collect();
        let outputs: Vec<String> = if !out_files.is_empty() {
            out_files
        } else if n.id == "repro" {
            vec!["repro.md".into(), "logs.txt".into()]
        } else {
            vec!["patch.diff".into(), "summary.md".into()]
        };
        let out_to = if outs.is_empty() {
            "not connected".to_string()
        } else {
            outs.iter()
                .map(|m| m.title.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let children: Vec<String> = self
            .nodes
            .iter()
            .filter(|m| m.kind() == Kind::Spawn && m.parent.as_deref() == Some(n.id.as_str()))
            .map(|m| format!("{} · ≤{} per run", m.title, m.max))
            .collect();
        let lib_idx = LIBRARY
            .iter()
            .position(|(v, _)| *v == n.title)
            .unwrap_or(LIBRARY.len() - 1);
        let model_idx = MODELS.iter().position(|m| *m == n.model);

        let library = self.select_field(
            Menu::Library,
            LIBRARY[lib_idx].1,
            LIBRARY
                .iter()
                .map(|(_, l)| SharedString::from(*l))
                .collect(),
            Some(lib_idx),
            false,
            FULL_W,
            cx,
        );
        let model = self.select_field(
            Menu::Model,
            n.model.clone(),
            MODELS.iter().map(|m| SharedString::from(*m)).collect(),
            model_idx,
            true,
            HALF_W,
            cx,
        );
        let variants = Stepper::new("variants", n.variants)
            .range(1, 5)
            .on_change(cx.listener(|this, v: &i32, _, cx| {
                this.update_sel(|n| n.variants = *v);
                cx.notify();
            }));

        let spawn = section(12.)
            .child(self.check_row(
                "can-spawn",
                n.can_spawn,
                strong("Can spawn sub-agents"),
                |this| this.update_sel(|n| n.can_spawn = !n.can_spawn),
                cx,
            ))
            .when(n.can_spawn, |d| {
                d.child(two_cols(
                    div().child(field_label("Max depth")).child(
                        Stepper::new("depth", n.depth)
                            .range(1, 3)
                            .on_change(cx.listener(|this, v: &i32, _, cx| {
                                this.update_sel(|n| n.depth = *v);
                                cx.notify();
                            })),
                    ),
                    div().child(field_label("Per run")).child(
                        Stepper::new("kids", n.kids)
                            .range(1, 5)
                            .on_change(cx.listener(|this, v: &i32, _, cx| {
                                this.update_sel(|n| n.kids = *v);
                                cx.notify();
                            })),
                    ),
                ))
                .when(n.depth > 2, |d| {
                    d.child(
                        div()
                            .px(px(12.))
                            .py(px(8.))
                            .rounded(px(RADIUS_LG))
                            .bg(WARNING_SOFT)
                            .text_color(WARNING)
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .child(format!(
                                "depth {} exceeds policy 2 · will ask a person before spawning",
                                n.depth
                            )),
                    )
                })
                .child(chip_row().children(children.into_iter().map(spawn_chip)))
            });

        vec![
            section(12.)
                .child(
                    div().child(field_label("Agent from your library")).child(library).child(help(
                        "The same agent you assign on the board or talk to in chat. Changes here apply only inside this pipeline.",
                    ).mt(px(6.))),
                )
                .child(self.name_field(cx))
                .child(div().child(field_label("Role")).child(self.textarea(
                    Area::Role,
                    &self.role_input.clone(),
                    &n.role,
                    4,
                    false,
                    cx,
                )))
                .child(two_cols(
                    div().child(field_label("Model")).child(model),
                    div().child(field_label("Variants")).child(variants),
                ))
                .child(help("Each variant runs in its own sandbox. A person picks one in Review."))
                .into_any_element(),
            section(10.)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(strong("Files in"))
                        .child(Button::new("add-file", "Add file").small().ghost().icon("plus")),
                )
                .child(chip_row().children(inputs.into_iter().map(|f| chip(f, true))))
                .child(strong("Files out").mt(px(4.)))
                .child(
                    chip_row()
                        .children(outputs.into_iter().map(|f| chip(f, true)))
                        .child(div().text_size(px(12.)).text_color(TEXT_MUTED).child(format!("→ {out_to}"))),
                )
                .into_any_element(),
            section(10.)
                .child(strong("Tools"))
                .child(chip_row().children(TOOLS.iter().take(n.tools).map(|t| chip(*t, true))))
                .into_any_element(),
            spawn.into_any_element(),
            section(8.)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(strong("Budget per issue"))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_SECONDARY)
                                .child(format!("${:.2} of $0.60", n.budget as f32 / 100.)),
                        ),
                )
                .child(progress(n.budget as f32 / 60., ACCENT))
                .into_any_element(),
        ]
    }

    fn parent_name(&self, n: &Node) -> String {
        n.parent
            .as_deref()
            .and_then(|p| self.by_id(p))
            .map(|p| p.title.clone())
            .unwrap_or_else(|| "any agent".into())
    }

    fn ins_spawn(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let parent = self.parent_name(n);
        vec![
            section(12.)
                .child(
                    div()
                        .flex()
                        .child(spawn_chip(format!("Created at runtime by {parent}"))),
                )
                .child(self.name_field(cx))
                .child(
                    div()
                        .child(field_label("Role template"))
                        .child(self.textarea(
                            Area::Role,
                            &self.role_input.clone(),
                            &n.role,
                            3,
                            false,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(div().text_color(TEXT_MUTED).child("Up to, per run"))
                        .child(
                            Stepper::new("max", n.max)
                                .range(1, 5)
                                .on_change(cx.listener(|this, v: &i32, _, cx| {
                                    this.update_sel(|n| n.max = *v);
                                    cx.notify();
                                })),
                        ),
                )
                .child(kv(
                    "Depth",
                    div().text_color(SPAWN_TEXT).child("1 of 2"),
                    true,
                ))
                .into_any_element(),
            section(10.)
                .child(strong(format!("Tools · subset of {parent}")))
                .child(
                    chip_row()
                        .child(chip("repo.read", true))
                        .child(chip("test.run", true)),
                )
                .child(help(format!(
                    "Returns its result to {parent}, not to Review."
                )))
                .into_any_element(),
        ]
    }

    fn ins_orch(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            section(12.)
                .child(para(
                    "Decides at run time who does what. Unlike a router, it can split an issue, call several teammates in parallel and create helpers.",
                ))
                .child(div().child(field_label("Instructions")).child(self.textarea(
                    Area::Role,
                    &self.role_input.clone(),
                    &n.role,
                    3,
                    false,
                    cx,
                )))
                .child(strong("Team"))
                .child(
                    chip_row()
                        .children(n.team.iter().map(|m| chip(m.clone(), false)))
                        .child(Button::new("team-add", "Add").small().ghost().icon("plus")),
                )
                .child(self.check_row(
                    "can-create",
                    n.can_create,
                    strong("Can create its own agents"),
                    |this| this.update_sel(|n| n.can_create = !n.can_create),
                    cx,
                ))
                .when(n.can_create, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(div().text_color(TEXT_MUTED).child("Alive at once"))
                            .child(Stepper::new("alive", n.max_alive).range(1, 5).on_change(cx.listener(
                                |this, v: &i32, _, cx| {
                                    this.update_sel(|n| n.max_alive = *v);
                                    cx.notify();
                                },
                            ))),
                    )
                    .child(help("Agents it creates show in violet here, on the board and in the agent list.").text_color(SPAWN_TEXT))
                })
                .into_any_element(),
        ]
    }

    fn ins_pipe(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let idx = PIPELINES.iter().position(|(v, _)| *v == n.reference);
        let value = idx.map(|i| PIPELINES[i].1).unwrap_or(PIPELINES[0].1);
        vec![
            section(12.)
                .child(div().child(field_label("Pipeline to call")).child(self.select_field(
                    Menu::PipeRef,
                    value,
                    PIPELINES.iter().map(|(_, l)| SharedString::from(*l)).collect(),
                    idx,
                    false,
                    FULL_W,
                    cx,
                )))
                .child(para(
                    "Runs as one step here, the same way you call it from chat or assign it on the board. Its own reviews still go to the inbox.",
                ))
                .child(
                    link_button("open-pipe", 32.)
                        .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Agents, cx))
                        .child(format!("Open {}", n.reference)),
                )
                .into_any_element(),
        ]
    }

    fn ins_source(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let every_idx = POLL_EVERY.iter().position(|e| *e == n.every);
        vec![
            section(12.)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(Badge::new("Connected").dot(ui::Status::Live))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_SECONDARY)
                                .child("jira.acme.dev"),
                        ),
                )
                .child(self.name_field(cx))
                .child(div().child(field_label("JQL")).child(self.textarea(
                    Area::Jql,
                    &self.jql_input.clone(),
                    &n.jql,
                    3,
                    true,
                    cx,
                )))
                .child(two_cols(
                    div()
                        .child(field_label("Poll every"))
                        .child(self.select_field(
                            Menu::Every,
                            n.every.clone(),
                            POLL_EVERY.iter().map(|e| SharedString::from(*e)).collect(),
                            every_idx,
                            true,
                            HALF_W,
                            cx,
                        )),
                    div().child(field_label("Issues per pull")).child(
                        Stepper::new("per-pull", self.issues_per_pull)
                            .range(1, 10)
                            .on_change(cx.listener(|this, v: &i32, _, cx| {
                                this.issues_per_pull = *v;
                                cx.notify();
                            })),
                    ),
                ))
                .child(
                    self.check_row(
                        "claim",
                        n.claim,
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(4.))
                            .child("Move to")
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .child("In progress"),
                            )
                            .child("when claimed"),
                        |this| this.update_sel(|n| n.claim = !n.claim),
                        cx,
                    ),
                )
                .into_any_element(),
            div()
                .flex()
                .flex_wrap()
                .px(px(20.))
                .py(px(16.))
                .border_b_1()
                .border_color(LINE)
                .child(
                    div()
                        .pr(px(20.))
                        .child(ui::Stat::new("Queued", self.queued().to_string())),
                )
                .child(
                    div()
                        .pl(px(20.))
                        .border_l_1()
                        .border_color(LINE)
                        .child(ui::Stat::new("Pulled today", "112")),
                )
                .into_any_element(),
        ]
    }

    fn ins_router(&self, router: &Node) -> Vec<AnyElement> {
        let routes = ROUTES.iter().enumerate().map(|(i, chips)| {
            let targets: Vec<&str> = self
                .edges
                .iter()
                .filter(|e| e.from == router.id && e.port == i as i32)
                .filter_map(|e| self.by_id(&e.to))
                .map(|n| n.title.as_str())
                .collect();
            let target = if targets.is_empty() {
                "no target".to_string()
            } else {
                targets.join(", ")
            };
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(12.))
                .py(px(10.))
                .border_1()
                .border_color(LINE)
                .rounded(px(RADIUS_XL))
                .bg(SURFACE)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child(format!("rule {}", i + 1)),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("→")
                                .child(
                                    div()
                                        .text_color(TEXT)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(target),
                                ),
                        ),
                )
                .child(chip_row().children(chips.iter().map(|(f, o, v)| rule_chip(f, o, v, false))))
        });
        vec![
            section(12.)
                .child(para(
                    "Rules run top to bottom. The first match decides the agent.",
                ))
                .children(routes)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(10.))
                        .border_1()
                        .border_dashed()
                        .border_color(LINE_STRONG)
                        .rounded(px(RADIUS_XL))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("else"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("skip · 4 of last 100"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .child(Button::new("add-rule", "Add rule").small().icon("plus")),
                )
                .into_any_element(),
        ]
    }

    fn ins_file(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let is_read = n.mode == FileMode::Read;
        let linked: Vec<&str> = if is_read {
            self.edges
                .iter()
                .filter(|e| e.from == n.id && e.port == -1)
                .filter_map(|e| self.by_id(&e.to))
                .map(|m| m.title.as_str())
                .collect()
        } else {
            self.edges
                .iter()
                .filter(|e| e.to == n.id)
                .filter_map(|e| self.by_id(&e.from))
                .map(|m| m.title.as_str())
                .collect()
        };
        let link_names = if linked.is_empty() {
            "not connected".to_string()
        } else {
            linked.join(", ")
        };
        let mono_list = |lines: &[&'static str]| {
            div()
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .line_height(px(22.))
                .text_color(TEXT_SECONDARY)
                .children(lines.iter().map(|l| div().child(*l)))
        };
        let matched = if is_read {
            section(6.)
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .child(strong("Matched"))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(TEXT_MUTED)
                                .child("14 files · 212 KB"),
                        ),
                )
                .child(
                    mono_list(&[
                        "docs/runbooks/payments.md",
                        "docs/architecture.md",
                        "docs/adr/0012-retries.md",
                        "CODEOWNERS",
                    ])
                    .child(div().text_color(TEXT_MUTED).child("+10 more")),
                )
        } else {
            section(6.)
                .child(strong("One copy per variant"))
                .child(mono_list(&[
                    "out/PAY-1431/spec.v1.md",
                    "out/PAY-1431/spec.v2.md",
                ]))
        };
        vec![
            section(12.)
                .child(self.name_field(cx))
                .child(
                    div()
                        .child(field_label("Path or glob"))
                        .child(self.input_field("glob-field", &self.glob_input.clone(), true, cx)),
                )
                .child(
                    div().child(field_label("Mode")).child(
                        div()
                            .flex()
                            .gap(px(16.))
                            .child(self.radio_row(
                                "mode-read",
                                is_read,
                                "Read into agents",
                                |this| this.set_file_mode(FileMode::Read),
                                cx,
                            ))
                            .child(self.radio_row(
                                "mode-write",
                                !is_read,
                                "Write output",
                                |this| this.set_file_mode(FileMode::Write),
                                cx,
                            )),
                    ),
                )
                .into_any_element(),
            matched.into_any_element(),
            div()
                .flex()
                .justify_between()
                .gap(px(12.))
                .px(px(20.))
                .py(px(16.))
                .border_b_1()
                .border_color(LINE)
                .child(div().text_color(TEXT_MUTED).child(if is_read {
                    "Feeds"
                } else {
                    "Written by"
                }))
                .child(div().text_right().child(link_names))
                .into_any_element(),
        ]
    }

    fn ins_gate(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let keys = |keys: Vec<AnyElement>, text: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(keys)
                .child(text)
        };
        vec![
            section(12.)
                .child(para("Variants from agents wait here. A person ships one, or sends the issue back with notes."))
                .child(
                    div()
                        .child(field_label("Decision"))
                        .child(self.radio_row(
                            "pick-one",
                            !self.pick_each,
                            "Ship one variant per issue",
                            |this| this.pick_each = false,
                            cx,
                        ))
                        .child(self.radio_row(
                            "pick-each",
                            self.pick_each,
                            "Approve each variant",
                            |this| this.pick_each = true,
                            cx,
                        )),
                )
                .child(two_cols(
                    div().child(field_label("Reviewers")).child(self.select_field(
                        Menu::Reviewers,
                        REVIEWERS[self.reviewers],
                        REVIEWERS.iter().map(|r| SharedString::from(*r)).collect(),
                        Some(self.reviewers),
                        false,
                        HALF_W,
                        cx,
                    )),
                    div().child(field_label("Nudge after")).child(self.select_field(
                        Menu::Nudge,
                        NUDGE[self.nudge],
                        NUDGE.iter().map(|r| SharedString::from(*r)).collect(),
                        Some(self.nudge),
                        true,
                        HALF_W,
                        cx,
                    )),
                ))
                .into_any_element(),
            section(10.)
                .child(strong("Keyboard in review"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(px(12.))
                        .gap_y(px(6.))
                        .text_size(px(12.))
                        .text_color(TEXT_MUTED)
                        .child(keys(vec![kbd("J").into_any_element(), kbd("K").into_any_element()], "move"))
                        .child(keys(
                            vec![kbd("1").into_any_element(), div().child("–").into_any_element(), kbd("3").into_any_element()],
                            "pick",
                        ))
                        .child(keys(vec![kbd("R").into_any_element()], "send back")),
                )
                .child(
                    link_button("open-review-queue", 40.)
                        .on_click(|_, _, cx| ui::nav::navigate(ui::nav::Screen::Inbox, cx))
                        .child("Open review queue"),
                )
                .into_any_element(),
        ]
    }

    fn ins_action(&self, n: &Node, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            section(12.)
                .child(self.name_field(cx))
                .child(kv("Does", n.meta.clone(), true))
                .children(n.rows.iter().map(|(l, v)| kv(l.clone(), v.clone(), true)))
                .child(help(
                    "Runs only after a person approves a variant in Review.",
                ))
                .into_any_element(),
        ]
    }
}
