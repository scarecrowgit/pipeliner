//! Settings tab: name, description, color, workdirs; archive / delete with confirmation.
//! Name, description and color save with the draft; workdirs are part of the graph.

use std::collections::{BTreeMap, HashMap};

use gpui::{Animation, AnimationExt as _, Hsla, Rgba};
use gpui_component::input::InputState;
use harness_core::Workdir;

use super::edit;
use super::inspector::{area_input, field_input, toggle};
use super::{Confirm, PipelinesScreen};
use crate::screens::prelude::*;
use crate::ui::input::InputEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WdF {
    Id,
    Name,
    Path,
    Base,
    Prefix,
    Shell,
    Setup,
    Env,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SF {
    Name,
    Desc,
    Wd(usize, WdF),
}

pub(super) struct SettingsForm {
    pipeline: Id,
    rev: u64,
    inputs: HashMap<SF, Entity<InputState>>,
    _subs: Vec<Subscription>,
}

/// Pipeline colors: the theme's palette tokens.
const SWATCHES: [Hsla; 6] = [
    theme::ACCENT,
    theme::INFO,
    theme::SUCCESS,
    theme::WARNING,
    theme::DANGER,
    theme::SPAWN,
];

/// A theme color as `0xRRGGBB` (what `Pipeline::color` stores).
fn to_u32(c: Hsla) -> u32 {
    let r = Rgba::from(c);
    let ch = |v: f32| ((v.clamp(0., 1.) * 255.).round() as u32) & 0xff;
    (ch(r.r) << 16) | (ch(r.g) << 8) | ch(r.b)
}

/// Distance between two `0xRRGGBB` colors (swatch matching tolerates rounding).
fn color_dist(a: u32, b: u32) -> u32 {
    let d = |s: u32| ((a >> s) & 0xff).abs_diff((b >> s) & 0xff);
    d(16) + d(8) + d(0)
}

fn env_text(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

impl PipelinesScreen {
    fn ensure_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_ref() else { return };
        if ed
            .settings
            .as_ref()
            .is_some_and(|s| s.pipeline == ed.pipeline_id && s.rev == ed.settings_rev)
        {
            return;
        }
        let mut inputs = HashMap::new();
        let mut subs = Vec::new();
        let mut add = |f: SF,
                       value: String,
                       placeholder: &str,
                       rows: Option<usize>,
                       window: &mut Window,
                       cx: &mut Context<Self>| {
            let placeholder = placeholder.to_string();
            let state = cx.new(|cx| {
                let s = InputState::new(window, cx);
                let s = match rows {
                    Some(r) => s.auto_grow(r, 10),
                    None => s,
                };
                s.placeholder(placeholder).default_value(value)
            });
            subs.push(
                cx.subscribe_in(&state, window, move |this, st, ev: &InputEvent, _, cx| {
                    if let InputEvent::Change = ev {
                        let v = st.read(cx).value().to_string();
                        this.on_settings_field(f, v, cx);
                    }
                }),
            );
            inputs.insert(f, state);
        };
        add(SF::Name, ed.name.clone(), "Pipeline name", None, window, cx);
        add(
            SF::Desc,
            ed.description.clone(),
            "What this pipeline is for",
            Some(2),
            window,
            cx,
        );
        for (i, w) in ed.graph.workdirs.clone().into_iter().enumerate() {
            add(SF::Wd(i, WdF::Id), w.id, "id", None, window, cx);
            add(SF::Wd(i, WdF::Name), w.name, "Name", None, window, cx);
            add(
                SF::Wd(i, WdF::Path),
                w.path,
                "~/code/project",
                None,
                window,
                cx,
            );
            add(
                SF::Wd(i, WdF::Base),
                w.base_branch,
                "main",
                None,
                window,
                cx,
            );
            add(
                SF::Wd(i, WdF::Prefix),
                w.branch_prefix,
                "pipeliner/",
                None,
                window,
                cx,
            );
            add(
                SF::Wd(i, WdF::Shell),
                w.shell,
                "e.g. nix-shell shell.nix --run",
                None,
                window,
                cx,
            );
            add(
                SF::Wd(i, WdF::Setup),
                w.setup,
                "Runs once after the worktree is created",
                None,
                window,
                cx,
            );
            add(
                SF::Wd(i, WdF::Env),
                env_text(&w.env),
                "KEY=value, one per line",
                Some(2),
                window,
                cx,
            );
        }
        let (pipeline, rev) = (ed.pipeline_id.clone(), ed.settings_rev);
        if let Some(ed) = self.ed.as_mut() {
            ed.settings = Some(SettingsForm {
                pipeline,
                rev,
                inputs,
                _subs: subs,
            });
        }
    }

    fn on_settings_field(&mut self, f: SF, v: String, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        match f {
            SF::Name if ed.name != v => ed.name = v,
            SF::Desc if ed.description != v => ed.description = v,
            SF::Name | SF::Desc => return,
            SF::Wd(i, field) => {
                let Some(w) = ed.graph.workdirs.get(i).cloned() else {
                    return;
                };
                match field {
                    WdF::Id => {
                        let new = v.trim().to_string();
                        if new.is_empty() || new == w.id || ed.graph.workdir(&new).is_some() {
                            return;
                        }
                        edit::rename_workdir(&mut ed.graph, &w.id, &new);
                        ed.rev += 1;
                    }
                    other => {
                        let w = &mut ed.graph.workdirs[i];
                        match other {
                            WdF::Name => w.name = v,
                            WdF::Path => w.path = v.trim().to_string(),
                            WdF::Base => w.base_branch = v.trim().to_string(),
                            WdF::Prefix => w.branch_prefix = v.trim().to_string(),
                            WdF::Shell => w.shell = v,
                            WdF::Setup => w.setup = v,
                            WdF::Env => w.env = parse_env(&v),
                            WdF::Id => {}
                        }
                        if other == WdF::Name {
                            ed.rev += 1;
                        }
                    }
                }
            }
        }
        self.touch(cx);
    }

    fn edit_workdirs(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut harness_core::PipelineGraph),
    ) {
        let Some(ed) = self.ed.as_mut() else { return };
        f(&mut ed.graph);
        ed.rev += 1;
        ed.settings_rev += 1;
        self.touch(cx);
    }

    fn archive_or_delete(&mut self, what: Confirm, cx: &mut Context<Self>) {
        let Some(ed) = self.ed.as_mut() else { return };
        ed.debounce = None;
        ed.saved_seq = ed.edit_seq;
        let id = ed.pipeline_id.clone();
        let name = ed.name.clone();
        let job = state::run_engine(cx, move |e| {
            let store = e.store();
            match what {
                Confirm::Archive => {
                    let mut p = store
                        .pipeline(&id)
                        .ok_or_else(|| anyhow::anyhow!("pipeline not found"))?;
                    p.archived = true;
                    p.updated_at = harness_core::now();
                    store.put_pipeline(&p)?;
                    store.log(
                        "pipeline.archived",
                        None,
                        None,
                        format!("Pipeline {} archived", p.name),
                    );
                }
                Confirm::Delete => {
                    store.delete_pipeline(&id)?;
                    store.log(
                        "pipeline.deleted",
                        None,
                        None,
                        format!("Pipeline {name} deleted"),
                    );
                }
            }
            Ok(())
        });
        cx.spawn(async move |_, cx| {
            if job.await.is_some() {
                let _ = cx.update(|cx| {
                    state::toast(
                        ToastKind::Success,
                        if what == Confirm::Archive {
                            "Pipeline archived"
                        } else {
                            "Pipeline deleted"
                        },
                        cx,
                    );
                    state::replace_route(Route::Pipelines { pipeline: None }, cx);
                });
            }
        })
        .detach();
    }

    pub(super) fn render_settings_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_settings(window, cx);
        let (Some(ed), Some(d)) = (self.ed.as_ref(), self.detail.as_ref()) else {
            return div().into_any_element();
        };
        let Some(form) = ed.settings.as_ref() else {
            return div().into_any_element();
        };
        let input = |f: SF| {
            form.inputs
                .get(&f)
                .map(|s| field_input(s).into_any_element())
        };
        let area = |f: SF| {
            form.inputs
                .get(&f)
                .map(|s| area_input(s).into_any_element())
        };
        let current = ed.color;
        let closest = SWATCHES
            .iter()
            .map(|c| color_dist(to_u32(*c), current))
            .min()
            .unwrap_or(u32::MAX);

        let general = card()
            .gap(theme::S4)
            .child(SectionHeader::new("Pipeline"))
            .child(row("Name", input(SF::Name)))
            .child(row("Description", area(SF::Desc)))
            .child(row(
                "Color",
                Some(
                    div()
                        .flex()
                        .gap(theme::S2)
                        .children(SWATCHES.iter().enumerate().map(|(ix, c)| {
                            let value = to_u32(*c);
                            let on = color_dist(value, current) == closest && closest < 8;
                            div()
                                .id(("pl-swatch", ix))
                                .size(px(28.))
                                .rounded(theme::RADIUS_FULL)
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_2()
                                .border_color(if on { theme::TEXT } else { theme::TRANSPARENT })
                                .cursor_pointer()
                                .child(div().size(px(20.)).rounded(theme::RADIUS_FULL).bg(*c))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(ed) = this.ed.as_mut() {
                                        ed.color = value;
                                    }
                                    this.touch(cx);
                                }))
                        }))
                        .into_any_element(),
                ),
            ))
            .child(div().text_label().fg_muted().child(format!(
                "Version v{} live{} · created {} ago",
                d.pipeline.version,
                if d.pipeline.draft.is_some() {
                    format!(", draft v{}", d.pipeline.version + 1)
                } else {
                    String::new()
                },
                fmt_ago(d.pipeline.created_at)
            )));

        let mut wds = card().gap(theme::S4).child(
            SectionHeader::new("Workdirs")
                .meta("folders agents work in · part of the draft")
                .child(
                    Button::new("pl-wd-add", "Add workdir")
                        .small()
                        .icon(AppIcon::Plus)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.edit_workdirs(cx, |g| {
                                let id = edit::unique_workdir_id(g, "work");
                                g.workdirs.push(Workdir {
                                    id,
                                    name: "Workdir".into(),
                                    path: String::new(),
                                    worktree: true,
                                    base_branch: "main".into(),
                                    branch_prefix: String::new(),
                                    setup: String::new(),
                                    shell: String::new(),
                                    env: Default::default(),
                                });
                            })
                        })),
                ),
        );
        if ed.graph.workdirs.is_empty() {
            wds = wds.child(
                EmptyState::new(
                    AppIcon::Folder,
                    "No workdirs",
                    "Add one so agent, render and pull request steps have a folder to work in.",
                )
                .compact()
                .id("pl-wd-empty"),
            );
        }
        for (i, w) in ed.graph.workdirs.iter().enumerate() {
            let on = w.worktree;
            let used = ed
                .graph
                .nodes
                .iter()
                .filter(|n| match &n.kind {
                    harness_core::NodeKind::Agent(a) => a.workdir.as_deref() == Some(w.id.as_str()),
                    harness_core::NodeKind::Decision(x) => {
                        x.workdir.as_deref() == Some(w.id.as_str())
                    }
                    harness_core::NodeKind::Render(x) => {
                        x.workdir.as_deref() == Some(w.id.as_str())
                    }
                    harness_core::NodeKind::Tool(harness_core::ToolNode::Command {
                        workdir,
                        ..
                    })
                    | harness_core::NodeKind::Tool(harness_core::ToolNode::PullRequest {
                        workdir,
                        ..
                    }) => workdir.as_deref() == Some(w.id.as_str()),
                    _ => false,
                })
                .count();
            wds = wds.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme::S3)
                    .p(theme::S4)
                    .rounded(theme::RADIUS_LG)
                    .border_1()
                    .border_color(theme::LINE_STRONG)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme::S2)
                            .child(
                                icon_sized(AppIcon::Folder, theme::ICON_SM)
                                    .text_color(theme::TEXT_MUTED),
                            )
                            .child(div().flex_1().text_body_strong().fg().child(w.name.clone()))
                            .child(div().text_label().fg_muted().child(format!(
                                "used by {used} node{}",
                                if used == 1 { "" } else { "s" }
                            )))
                            .child(
                                IconButton::new(("pl-wd-del", i), AppIcon::Trash)
                                    .small()
                                    .tooltip(if used > 0 {
                                        "Remove (its nodes lose their workdir)"
                                    } else {
                                        "Remove"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.edit_workdirs(cx, |g| edit::remove_workdir(g, i))
                                    })),
                            ),
                    )
                    .child(pair(
                        row("Id", input(SF::Wd(i, WdF::Id))),
                        row("Name", input(SF::Wd(i, WdF::Name))),
                    ))
                    .child(row("Path", input(SF::Wd(i, WdF::Path))))
                    .child(pair(
                        row("Base branch", input(SF::Wd(i, WdF::Base))),
                        row("Branch prefix", input(SF::Wd(i, WdF::Prefix))),
                    ))
                    .child(row("Shell wrapper", input(SF::Wd(i, WdF::Shell))))
                    .child(row("Setup", input(SF::Wd(i, WdF::Setup))))
                    .child(row("Environment", area(SF::Wd(i, WdF::Env))))
                    .child(toggle(
                        ("pl-wd-wt", i),
                        on,
                        "Git worktree per run",
                        "Each run (and each variant) gets its own branch and folder",
                        cx.listener(move |this, _, _, cx| {
                            if let Some(ed) = this.ed.as_mut() {
                                if let Some(w) = ed.graph.workdirs.get_mut(i) {
                                    w.worktree = !w.worktree;
                                }
                            }
                            this.touch(cx);
                        }),
                    )),
            );
        }

        let confirm = self.confirm;
        let danger = card()
            .gap(theme::S3)
            .child(SectionHeader::new("Archive or delete"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S3)
                    .child(div().flex_1().text_body().fg_secondary().child(
                        "Archiving hides the pipeline; its runs stay on the Board and Timeline. Deleting removes it for good.",
                    ))
                    .when(confirm.is_none(), |el| {
                        el.child(Button::new("pl-archive", "Archive").small().on_click(cx.listener(|this, _, _, cx| {
                            this.confirm = Some(Confirm::Archive);
                            cx.notify();
                        })))
                        .child(Button::new("pl-delete", "Delete").danger().small().icon(AppIcon::Trash).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.confirm = Some(Confirm::Delete);
                                cx.notify();
                            },
                        )))
                    }),
            )
            .when_some(confirm, |el, what| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme::S3)
                        .p(theme::S3)
                        .rounded(theme::RADIUS_LG)
                        .bg(theme::DANGER_SOFT)
                        .child(icon_sized(AppIcon::Alert, theme::ICON_SM).text_color(theme::DANGER))
                        .child(div().flex_1().text_body().fg().child(match what {
                            Confirm::Archive => format!("Archive {}? New runs can't start from it.", d.pipeline.name),
                            Confirm::Delete => format!("Delete {} permanently? This can't be undone.", d.pipeline.name),
                        }))
                        .child(Button::new("pl-confirm-no", "Keep it").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                            this.confirm = None;
                            cx.notify();
                        })))
                        .child(
                            Button::new("pl-confirm-yes", if what == Confirm::Archive { "Archive" } else { "Delete" })
                                .danger()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.confirm = None;
                                    this.archive_or_delete(what, cx);
                                })),
                        )
                        .with_animation(
                            "pl-confirm-in",
                            Animation::new(theme::DUR_ENTER).with_easing(theme::ease_out),
                            |el, t| el.opacity(t),
                        ),
                )
            });

        body()
            .size_full()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(760.))
                    .flex()
                    .flex_col()
                    .gap(theme::S6)
                    .child(general)
                    .child(wds)
                    .child(danger),
            )
            .into_any_element()
    }
}

/// Label above a control.
fn row(label_text: &str, control: Option<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(theme::S1)
        .flex_1()
        .min_w_0()
        .child(label(label_text.to_string()))
        .children(control)
}

/// Two rows side by side.
fn pair(a: Div, b: Div) -> Div {
    div().flex().gap(theme::S3).child(a).child(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_round_trip() {
        let env = parse_env("A=1\n  B = two words \nbad line\n=x");
        assert_eq!(env.len(), 2);
        assert_eq!(env["B"], "two words");
        assert_eq!(parse_env(&env_text(&env)), env);
    }

    #[test]
    fn swatch_colors_round_trip() {
        assert!(color_dist(to_u32(theme::INFO), 0x4ea7fc) < 4);
    }
}
