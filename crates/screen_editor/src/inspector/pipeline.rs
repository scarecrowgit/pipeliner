//! Nothing selected: the pipeline itself. Name, description, the run limits and the workdirs
//! its nodes work in.

use std::rc::Rc;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};
use hub::model::{GraphDef, WorkdirDef};
use ui::dev::Probe;
use ui::prelude::*;

use super::InspectorView;
use super::fields::{Spec, on_graph, parse_usd, parse_word, usd_text, usd_value};
use super::form::{INNER, field, note, psec, row_field, titled};
use super::ops;

/// Width of the fields inside a workdir card.
const CARD_INNER: f32 = INNER - 2. * SPACE_3;
/// The widest a workdir's id is drawn in its card's head before it ends in `…`, px.
const WORKDIR_ID_MAX: f32 = 96.0;

impl InspectorView {
    pub(super) fn pipeline_view(&mut self, graph: &GraphDef, window: &mut Window, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let header = self.header(IconName::Pipelines, TileTone::Neutral, graph.name.clone(), "Pipeline".into(), false, window, cx);
        let mut sections = Vec::new();
        let problems = self.problems_of(None, cx);
        if let Some(problems) = self.problem_list(problems, cx) {
            sections.push(problems);
        }
        let name = self.line(
            Spec::new("insp-name", graph.name.clone(), "Rename pipeline", on_graph(|g, text| g.name = text.to_string())),
            None,
            window,
            cx,
        );
        let description = self.area(
            Spec::new("insp-description", graph.description.clone(), "Edit description", on_graph(|g, text| g.description = text.to_string()))
                .area(2, 8)
                .placeholder("What it does, in a sentence"),
            None,
            window,
            cx,
        );
        sections.push(
            psec()
                .gap(px(SPACE_3))
                .child(field("Name", name))
                .child(field("Description", description))
                .child(field("Id", div().type_style(MONO).text_color(TEXT_SECONDARY).child(graph.id.to_string())))
                .into_any_element(),
        );
        sections.push(self.limits_section(graph, window, cx));
        sections.push(self.workdirs_section(graph, window, cx));
        (header, sections)
    }

    fn limits_section(&mut self, graph: &GraphDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let limits = &graph.limits;
        let parallel = Stepper::new("insp-max-parallel", limits.max_parallel as i32)
            .range(1, 32)
            .on_change(self.on_value(cx, |this, v: i32, cx| this.edit("Set max parallel", |g| g.limits.max_parallel = v as u32, cx)));
        let loops = Stepper::new("insp-max-loops", limits.max_loops as i32)
            .range(0, 20)
            .on_change(self.on_value(cx, |this, v: i32, cx| this.edit("Set max loops", |g| g.limits.max_loops = v as u32, cx)));
        let budget = self.line(
            Spec::new("insp-budget", usd_text(limits.budget_usd), "Set run budget", on_graph(|g, text| g.limits.budget_usd = usd_value(text)))
                .parse(parse_usd)
                .placeholder("No cap")
                .mono(),
            None,
            window,
            cx,
        );
        titled("Limits", None)
            .gap(px(SPACE_2))
            .child(row_field("Max parallel", parallel.probe("insp-max-parallel")))
            .child(note("Agent instances of one run working at the same time.", INNER))
            .child(row_field("Max loops", loops.probe("insp-max-loops")).mt(px(SPACE_2)))
            .child(note("How many times a review may send work back before the run stops and asks you.", INNER))
            .child(
                row()
                    .mt(px(SPACE_2))
                    .gap(px(SPACE_3))
                    .child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Budget per run"))
                    .child(row().w(px(120.)).gap(px(SPACE_1)).child(div().text_color(TEXT_MUTED).child("$")).child(budget)),
            )
            .into_any_element()
    }

    fn workdirs_section(&mut self, graph: &GraphDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let count = graph.workdirs.len();
        let add = Button::new("insp-workdir-add", "Add workdir")
            .ghost()
            .small()
            .icon(IconName::Plus)
            .on_click(self.on_click(cx, |this, _, cx| this.add_workdir(cx)))
            .probe("insp-workdir-add");
        let meta = row().gap(px(SPACE_2)).child(base::fmt::plural(count as u64, "workdir", "workdirs")).child(add);
        let mut section = titled("Workdirs", Some(meta.into_any_element())).gap(px(SPACE_2));
        if graph.workdirs.is_empty() {
            section = section.child(note("Folders the nodes work in. Agents, files and actions pick one by its id.", INNER));
        }
        for (index, workdir) in graph.workdirs.iter().enumerate() {
            section = section.child(self.workdir_card(graph, index, workdir, window, cx));
        }
        section.into_any_element()
    }

    fn workdir_card(&mut self, graph: &GraphDef, index: usize, workdir: &WorkdirDef, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let open = self.open_workdir == Some(index);
        let users = ops::workdir_users(graph, &workdir.id);
        let used = match users.len() {
            0 => "not used".to_string(),
            n => base::fmt::plural(n as u64, "node", "nodes"),
        };
        let toggle = IconButton::new(SharedString::from(format!("insp-wd-{index}-edit")), if open { IconName::ChevronDown } else { IconName::Edit })
            .small()
            .tooltip(if open { "Close" } else { "Edit workdir" })
            .on_click(self.on_click(cx, move |this, _, cx| {
                this.open_workdir = if this.open_workdir == Some(index) { None } else { Some(index) };
                cx.notify();
            }))
            .probe(format!("insp-wd-{index}-edit"));
        let remove = IconButton::new(SharedString::from(format!("insp-wd-{index}-remove")), IconName::Trash)
            .small()
            .tooltip("Remove workdir")
            .on_click(self.on_click(cx, move |this, _, cx| this.remove_workdir(index, cx)))
            .probe(format!("insp-wd-{index}-remove"));
        let head = row()
            .debug_selector(move || format!("insp-wd-{index}-head"))
            .gap(px(SPACE_2))
            .child(icon(IconName::Folder, 14., TEXT_MUTED))
            .child(div().flex_shrink().min_w_0().type_style(BODY_STRONG).child(truncate(workdir.name.clone())))
            .child(div().flex_shrink().min_w_0().max_w(px(WORKDIR_ID_MAX)).type_style(MONO_SM).text_color(TEXT_MUTED).child(truncate_lh(workdir.id.clone(), MONO_SM.line)))
            .child(grow())
            .child(div().flex_none().type_style(LABEL).text_color(TEXT_MUTED).child(used))
            .child(toggle)
            .child(remove);
        let path = hub::display_path(&workdir.path, cx);
        let summary = row()
            .debug_selector(move || format!("insp-wd-{index}-summary"))
            .gap(px(SPACE_2))
            .type_style(MONO_SM)
            .text_color(TEXT_SECONDARY)
            .child(truncate(path.clone()))
            .when(workdir.worktree, |d| d.child(Tag::new("worktree").mono()));
        let mut card = col()
            .gap(px(SPACE_2))
            .p(px(SPACE_3))
            .rounded(px(RADIUS_XL))
            .border_1()
            .border_color(LINE)
            .bg(SURFACE)
            .child(head)
            .child(summary);
        if let Some((blocked, message)) = self.blocked_workdir.clone().filter(|(i, _)| *i == index) {
            let _ = blocked;
            let message = self.texts.text("workdir-blocked", message, LABEL, DANGER, cx);
            card = card.child(div().w(px(CARD_INNER)).child(message));
        }
        if open {
            card = card.child(self.workdir_fields(graph, index, workdir, path, window, cx));
        }
        card.probe(format!("insp-wd-{index}")).into_any_element()
    }

    fn workdir_fields(
        &mut self,
        graph: &GraphDef,
        index: usize,
        workdir: &WorkdirDef,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = |part: &str| format!("insp-wd-{index}-{part}");
        let problem = graph
            .workdirs
            .get(index)
            .and_then(|w| self.problems_of(None, cx).into_iter().find(|(_, m)| m.starts_with(&format!("Workdir {} ", w.name))))
            .map(|(_, m)| SharedString::from(m));
        let edit = move |f: fn(&mut WorkdirDef, &str)| {
            on_graph(move |g, text| {
                if let Some(workdir) = g.workdirs.get_mut(index) {
                    f(workdir, text);
                }
            })
        };
        let id_graph = Rc::new(graph.clone());
        let id = self.line(
            Spec::new(
                key("id"),
                workdir.id.clone(),
                "Rename workdir id",
                on_graph(move |g, text| {
                    if let Some(old) = g.workdirs.get(index).map(|w| w.id.clone()) {
                        ops::rename_workdir(g, &old, text);
                    }
                }),
            )
            .parse(move |text| match ops::workdir_id_error(&id_graph, index, text) {
                Some(error) => Err(error.into()),
                None => Ok(text.to_string()),
            })
            .mono(),
            None,
            window,
            cx,
        );
        let name = self.line(Spec::new(key("name"), workdir.name.clone(), "Rename workdir", edit(|w, t| w.name = t.to_string())), None, window, cx);
        let paths = hub::hub(cx).read(cx).paths().clone();
        let folder = self.line(
            Spec::new(key("path"), path, "Set workdir folder", {
                on_graph(move |g, text| {
                    if let Some(workdir) = g.workdirs.get_mut(index) {
                        workdir.path = paths.expand(text.trim());
                    }
                })
            })
            .parse({
                let paths = hub::hub(cx).read(cx).paths().clone();
                move |text| Ok(paths.display(&paths.expand(text.trim())))
            })
            .mono()
            .placeholder("~/code/repo"),
            problem,
            window,
            cx,
        );
        let worktree = Toggle::new(SharedString::from(key("worktree")), workdir.worktree).on_change(self.on_value(cx, move |this, on: bool, cx| {
            this.edit(
                "Toggle worktree",
                |g| {
                    if let Some(workdir) = g.workdirs.get_mut(index) {
                        workdir.worktree = on;
                    }
                },
                cx,
            )
        }));
        let base_branch = self.line(
            Spec::new(key("base"), workdir.base_branch.clone(), "Set base branch", edit(|w, t| w.base_branch = t.to_string()))
                .parse(parse_word)
                .mono()
                .placeholder("main"),
            None,
            window,
            cx,
        );
        let prefix = self.line(
            Spec::new(key("prefix"), workdir.branch_prefix.clone(), "Set branch prefix", edit(|w, t| w.branch_prefix = t.to_string()))
                .parse(parse_word)
                .mono()
                .placeholder("fix/"),
            None,
            window,
            cx,
        );
        let setup = self.line(
            Spec::new(key("setup"), workdir.setup.clone(), "Set setup command", edit(|w, t| w.setup = t.to_string()))
                .mono()
                .placeholder("bun install"),
            None,
            window,
            cx,
        );
        let env = self.area(
            Spec::new(key("env"), ops::env_text(&workdir.env), "Set environment", {
                on_graph(move |g, text| {
                    if let (Some(workdir), Ok(env)) = (g.workdirs.get_mut(index), ops::parse_env(text)) {
                        workdir.env = env;
                    }
                })
            })
            .parse(|text| ops::parse_env(text).map(|env| ops::env_text(&env)).map_err(SharedString::from))
            .area(1, 6)
            .mono()
            .placeholder("KEY=value, one per line"),
            None,
            window,
            cx,
        );
        col()
            .gap(px(SPACE_3))
            .pt(px(SPACE_2))
            .child(row().gap(px(SPACE_2)).child(div().w(px(96.)).child(field("Id", id))).child(div().flex_1().min_w_0().child(field("Name", name))))
            .child(field("Folder", folder))
            .child(row_field("Fresh worktree per run", worktree.probe(key("worktree"))))
            .child(note("Each run and each variant gets its own git worktree and branch. Off: the folder is used as is.", CARD_INNER))
            .when(workdir.worktree, |d| {
                d.child(row().gap(px(SPACE_2)).child(div().flex_1().min_w_0().child(field("Base branch", base_branch))).child(div().flex_1().min_w_0().child(field("Branch prefix", prefix))))
            })
            .child(field("Setup command", setup))
            .child(note("Runs once in a new worktree: a warm-up build, an install.", CARD_INNER))
            .child(field("Environment", env))
            .into_any_element()
    }

    fn add_workdir(&mut self, cx: &mut Context<Self>) {
        let mut index = 0;
        self.edit(
            "Add workdir",
            |g| {
                let id = ops::unique("workdir", |id| g.workdirs.iter().any(|w| w.id == id));
                g.workdirs.push(WorkdirDef { name: id.clone(), id, base_branch: "main".into(), ..WorkdirDef::default() });
                index = g.workdirs.len() - 1;
            },
            cx,
        );
        self.open_workdir = Some(index);
        self.focus_field_when_drawn(format!("insp-wd-{index}-name"), cx);
    }

    /// Removes workdir `index` unless a node works in it; then says which ones do.
    fn remove_workdir(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(graph) = self.draft.read(cx).graph() else { return };
        let Some(workdir) = graph.workdirs.get(index) else { return };
        let users = ops::workdir_users(graph, &workdir.id);
        if !users.is_empty() {
            let message = format!("Used by {}. Point them at another workdir first.", users.join(", "));
            self.blocked_workdir = Some((index, message));
            cx.notify();
            return;
        }
        self.blocked_workdir = None;
        self.open_workdir = None;
        self.edit(
            "Remove workdir",
            |g| {
                if index < g.workdirs.len() {
                    g.workdirs.remove(index);
                }
            },
            cx,
        );
    }
}
