//! Settings › Skills: get a skill (link, install command or a request in words), review what
//! was fetched, install the picks, and the list of installed skills.
//! Design: design/screens/settings.html, section `#skills`.
//!
//! Fetching, running and installing happen on background threads (`Engine::stage_skill`,
//! `install_skills`, ...); nothing is installed before the user reviews it. This is a child
//! module of `settings` so it can use the screen's private state.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Local, Utc};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::InputState;
use gpui_component::Sizable as _;
use harness_core::skills::{
    self, classify, parse_github, Clash, Provenance, Skill, SourceKind, StagedSet, StagedSkill,
};

use super::{block_plain, hint, SettingsScreen};
use crate::screens::prelude::*;
use crate::ui::input::{self, InputEvent};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One installed skill with what is derived from disk and agents once per load.
pub(super) struct InstalledRow {
    skill: Skill,
    prov: Option<Provenance>,
    /// `(agent name, always)`: agents that use the skill (`false` = on demand).
    users: Vec<(String, bool)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Fetch,
    Run,
    Ask,
}

impl Busy {
    fn label(self) -> &'static str {
        match self {
            Busy::Fetch => "Fetching…",
            Busy::Run => "Running…",
            Busy::Ask => "Asking…",
        }
    }
}

/// What to do with a staged skill whose name is already installed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum How {
    Skip,
    Replace,
    Rename,
}

/// The user's choice for one staged skill. `on` is the checkbox for skills without a
/// clash; for a clash the choice in `how` decides (`Skip` = not picked).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Pick {
    on: bool,
    how: How,
}

type SetPicks = HashMap<String, Pick>;

pub(super) struct SkillsUi {
    input: Entity<InputState>,
    busy: Option<Busy>,
    /// The command text the user already clicked "Run" on once.
    confirm: Option<String>,
    error: Option<String>,
    pub(super) rows: Vec<InstalledRow>,
    staged: Vec<StagedSet>,
    picks: HashMap<String, SetPicks>,
    /// Staged-set ids being installed or discarded right now.
    working: HashSet<String>,
    load_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

enum Done {
    Staged(StagedSet),
    Chat(Id),
}

impl SkillsUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsScreen>) -> SkillsUi {
        let input = input::text_input_with("", "", window, cx);
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::Change => {
                    let text = input.read(cx).value().trim().to_string();
                    if this.sk.confirm.as_deref() != Some(text.as_str()) {
                        this.sk.confirm = None;
                    }
                    this.sk.error = None;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.skills_go(window, cx),
                _ => {}
            }
        });
        SkillsUi {
            input,
            busy: None,
            confirm: None,
            error: None,
            rows: vec![],
            staged: vec![],
            picks: HashMap::new(),
            working: HashSet::new(),
            load_task: None,
            _subs: vec![sub],
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

fn default_pick(s: &StagedSkill) -> Pick {
    Pick {
        on: s.clash.is_none(),
        how: How::Skip,
    }
}

fn is_picked(s: &StagedSkill, p: &Pick) -> bool {
    if s.clash.is_some() {
        p.how != How::Skip
    } else {
        p.on
    }
}

/// What a clashing skill is renamed with: the repo owner, else "2".
fn rename_suffix(set: &StagedSet) -> String {
    parse_github(&set.source)
        .map(|g| g.owner)
        .filter(|o| !o.is_empty())
        .unwrap_or_else(|| "2".into())
}

fn renamed(name: &str, set: &StagedSet) -> String {
    format!("{name}-{}", rename_suffix(set))
}

/// The `(name, what to do)` list for `Engine::install_skills`: only picked skills.
fn installs(set: &StagedSet, picks: &SetPicks) -> Vec<(String, Clash)> {
    set.skills
        .iter()
        .filter_map(|s| {
            let p = picks.get(&s.name).copied().unwrap_or_else(|| default_pick(s));
            if !is_picked(s, &p) {
                return None;
            }
            let how = match (s.clash.is_some(), p.how) {
                (true, How::Rename) => Clash::Rename(renamed(&s.name, set)),
                // No clash: installs normally. A clash picked as Replace replaces.
                _ => Clash::Replace,
            };
            Some((s.name.clone(), how))
        })
        .collect()
}

fn fmt_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{:.1} MB", b / KB / KB)
    }
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// "Fixer", "3 agents" and "always" / "on demand" for the agents that use a skill.
fn usage_label(users: &[(String, bool)]) -> Option<(String, &'static str)> {
    let first = users.first()?;
    let who = if users.len() == 1 {
        first.0.clone()
    } else {
        format!("{} agents", users.len())
    };
    let always = users.iter().filter(|u| u.1).count();
    let when = if always == users.len() {
        "always"
    } else if always == 0 {
        "on demand"
    } else {
        "always / on demand"
    };
    Some((who, when))
}

/// Icon and text for where an installed skill came from.
fn source_line(skill: &Skill, prov: Option<&Provenance>) -> (AppIcon, String) {
    let date = |p: &Provenance| p.installed_at.with_timezone(&Local).format("%b %-d").to_string();
    match prov {
        Some(p) if p.actor.is_some() => {
            let mut s = p.actor.clone().unwrap_or_default();
            if let Some(t) = &p.chat_title {
                s.push_str(&format!(" · chat “{t}”"));
            }
            s.push_str(&format!(" · {}", date(p)));
            (AppIcon::Chat, s)
        }
        Some(p) if p.kind == SourceKind::Link => {
            let mut s = p.label.clone();
            if let Some(c) = &p.commit {
                s.push_str(&format!(" @ {c}"));
            }
            (AppIcon::Link, s)
        }
        Some(p) if p.kind == SourceKind::Command => {
            (AppIcon::Terminal, format!("{} · {}", p.label, date(p)))
        }
        _ => (AppIcon::Folder, skill.dir_label.clone()),
    }
}

fn local_hm(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M").to_string()
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

impl SettingsScreen {
    /// Re-reads installed skills, staged sets and which agents use what (background).
    pub(super) fn load_skills(&mut self, cx: &mut Context<Self>) {
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move {
            let list = engine.skills();
            let staged = engine.staged_skills();
            let agents = engine.store().agents();
            let rows: Vec<InstalledRow> = list
                .into_iter()
                .map(|skill| {
                    let users = agents
                        .iter()
                        .filter_map(|a| {
                            if a.skills.contains(&skill.name) {
                                Some((a.name.clone(), true))
                            } else if a.skills_on_demand.contains(&skill.name) {
                                Some((a.name.clone(), false))
                            } else {
                                None
                            }
                        })
                        .collect();
                    InstalledRow {
                        prov: skills::provenance(&skill),
                        skill,
                        users,
                    }
                })
                .collect();
            (rows, staged)
        });
        self.sk.load_task = Some(cx.spawn(async move |this, cx| {
            let (rows, staged) = job.await;
            let _ = this.update(cx, |this, cx| {
                let sk = &mut this.sk;
                sk.rows = rows;
                sk.staged = staged;
                sk.picks.retain(|id, _| sk.staged.iter().any(|s| &s.id == id));
                for set in &sk.staged {
                    let e = sk.picks.entry(set.id.clone()).or_default();
                    e.retain(|n, _| set.skills.iter().any(|s| &s.name == n));
                    for s in &set.skills {
                        e.entry(s.name.clone()).or_insert_with(|| default_pick(s));
                    }
                }
                sk.working.retain(|id| sk.staged.iter().any(|s| &s.id == id));
                cx.notify();
            });
        }));
    }

    /// The primary button of step 1: Fetch / Run (after a confirm) / Ask agent.
    pub(super) fn skills_go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sk.busy.is_some() {
            return;
        }
        let text = self.sk.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let (kind, _) = classify(&text);
        self.sk.error = None;
        if kind == SourceKind::Command && self.sk.confirm.as_deref() != Some(text.as_str()) {
            self.sk.confirm = Some(text);
            cx.notify();
            return;
        }
        self.sk.confirm = None;
        self.sk.busy = Some(match kind {
            SourceKind::Link => Busy::Fetch,
            SourceKind::Command => Busy::Run,
            SourceKind::Words => Busy::Ask,
        });
        cx.notify();
        let engine = state::engine(cx);
        let job = cx.background_spawn(async move {
            match kind {
                SourceKind::Words => engine.ask_agent_for_skill(&text).map(Done::Chat),
                _ => engine.stage_skill(&text).map(Done::Staged),
            }
            .map_err(|e| format!("{e:#}"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let r = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.sk.busy = None;
                match r {
                    Ok(done) => {
                        this.sk
                            .input
                            .update(cx, |s, cx| s.set_value("", window, cx));
                        this.sk.error = None;
                        match done {
                            Done::Staged(set) => {
                                state::toast(
                                    ToastKind::Success,
                                    format!(
                                        "Fetched {} from {}. Review it below.",
                                        plural(set.skills.len(), "skill"),
                                        set.label
                                    ),
                                    cx,
                                );
                                this.load_skills(cx);
                            }
                            Done::Chat(id) => {
                                state::toast(
                                    ToastKind::Info,
                                    "Asked the Orchestrator. Anything it finds shows up here for review.",
                                    cx,
                                );
                                state::navigate(Route::Chat { chat: Some(id) }, cx);
                            }
                        }
                    }
                    Err(e) => this.sk.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn with_pick(
        &mut self,
        set_id: &str,
        name: &str,
        cx: &mut Context<Self>,
        f: impl FnOnce(&StagedSkill, &mut Pick),
    ) {
        let Some(skill) = self
            .sk
            .staged
            .iter()
            .find(|s| s.id == set_id)
            .and_then(|s| s.skills.iter().find(|k| k.name == name))
            .cloned()
        else {
            return;
        };
        let pick = self
            .sk
            .picks
            .entry(set_id.to_string())
            .or_default()
            .entry(name.to_string())
            .or_insert_with(|| default_pick(&skill));
        f(&skill, pick);
        cx.notify();
    }

    fn install_set(&mut self, set_id: &str, cx: &mut Context<Self>) {
        let Some(set) = self.sk.staged.iter().find(|s| s.id == set_id) else {
            return;
        };
        let picks = installs(set, self.sk.picks.get(set_id).unwrap_or(&SetPicks::new()));
        if picks.is_empty() || self.sk.working.contains(set_id) {
            return;
        }
        let into = skills::install_dir(&self.saved);
        let id = set_id.to_string();
        self.sk.working.insert(id.clone());
        cx.notify();
        let engine = state::engine(cx);
        let job = {
            let id = id.clone();
            cx.background_spawn(async move {
                engine
                    .install_skills(&id, &picks, &into)
                    .map_err(|e| format!("{e:#}"))
            })
        };
        cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                this.sk.working.remove(&id);
                match r {
                    Ok(paths) => state::toast(
                        ToastKind::Success,
                        format!("Installed {}", plural(paths.len(), "skill")),
                        cx,
                    ),
                    Err(e) => state::toast(ToastKind::Danger, e, cx),
                }
                this.load_skills(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn discard_set(&mut self, set_id: &str, cx: &mut Context<Self>) {
        if self.sk.working.contains(set_id) {
            return;
        }
        let id = set_id.to_string();
        self.sk.working.insert(id.clone());
        cx.notify();
        let engine = state::engine(cx);
        let job = {
            let id = id.clone();
            cx.background_spawn(async move {
                engine.discard_staged(&id).map_err(|e| format!("{e:#}"))
            })
        };
        cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |this, cx| {
                this.sk.working.remove(&id);
                match r {
                    Ok(()) => state::toast(ToastKind::Info, "Discarded", cx),
                    Err(e) => state::toast(ToastKind::Danger, e, cx),
                }
                this.load_skills(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn kind_icon(kind: SourceKind) -> AppIcon {
    match kind {
        SourceKind::Link => AppIcon::Link,
        SourceKind::Command => AppIcon::Terminal,
        SourceKind::Words => AppIcon::Chat,
    }
}

fn step_num(n: &'static str) -> Div {
    div()
        .w(px(18.))
        .h(px(18.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_FULL)
        .bg(theme::ACCENT_SOFT)
        .text_color(theme::ACCENT_TEXT)
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(n)
}

fn step_head(n: &'static str, title: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(theme::S2)
        .child(step_num(n))
        .child(
            div()
                .flex_none()
                .text_body_strong()
                .fg()
                .whitespace_nowrap()
                .child(title),
        )
}

fn mono_box(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .h(theme::CONTROL_H_SM)
        .flex()
        .items_center()
        .px(theme::S2)
        .rounded(theme::RADIUS_MD)
        .bg(theme::SURFACE_RAISED)
        .border_1()
        .border_color(theme::LINE)
        .text_mono()
        .fg()
        .ellipsis()
        .child(text.into())
}

impl SettingsScreen {
    pub(super) fn render_skills(&self, cx: &mut Context<Self>) -> Div {
        let sk = &self.sk;
        let header = div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .min_h(px(28.))
            .child(icon_tile(
                AppIcon::Sparkles,
                theme::SPAWN_SOFT,
                theme::SPAWN_TEXT,
            ))
            .child(
                div()
                    .flex_none()
                    .text_heading()
                    .fg()
                    .whitespace_nowrap()
                    .child("Skills"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_label()
                    .fg_muted()
                    .ellipsis()
                    .child(
                        "folders with a SKILL.md · agents get them always or on demand (load_skill)",
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .text_label()
                    .fg_muted()
                    .whitespace_nowrap()
                    .child(format!(
                        "{} installed · {} waiting for you",
                        sk.rows.len(),
                        sk.staged.len()
                    )),
            );

        let skbox = div()
            .flex()
            .flex_col()
            .rounded(theme::RADIUS_LG)
            .border_1()
            .border_color(theme::LINE_STRONG)
            .bg(theme::BG_SIDEBAR)
            .overflow_hidden()
            .child(self.skills_get_step(cx))
            .child(self.skills_review_step(cx));

        card()
            .gap(theme::S3)
            .child(header)
            .child(skbox)
            .child(self.skills_installed(cx))
    }

    // ---- step 1 ----------------------------------------------------------------

    fn skills_get_step(&self, cx: &mut Context<Self>) -> Div {
        let sk = &self.sk;
        let text = sk.input.read(cx).value().trim().to_string();
        let (kind, kind_label) = classify(&text);
        let empty = text.is_empty();
        let busy = sk.busy;
        let confirming = kind == SourceKind::Command && sk.confirm.as_deref() == Some(text.as_str());

        let button_label = match busy {
            Some(b) => b.label(),
            None => match kind {
                SourceKind::Link => "Fetch",
                SourceKind::Command if confirming => "Confirm run",
                SourceKind::Command => "Run",
                SourceKind::Words => "Ask agent",
            },
        };
        let badge = Badge::new(kind_label)
            .accent()
            .leading(icon_sized(kind_icon(kind), px(12.)));
        let field = input::field(&sk.input)
            .font_family(theme::mono())
            .text_size(px(12.))
            .min_h(px(32.))
            .max_h(px(32.))
            .disabled(busy.is_some())
            .when(!empty, |i| i.suffix(badge));

        let way = |id: &'static str, k: SourceKind, name: &'static str, body: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .py(px(6.))
                .when(k != SourceKind::Link, |d| {
                    d.border_t_1().border_color(theme::LINE_SUBTLE)
                })
                .text_label()
                .child(
                    div()
                        .flex_none()
                        .text_color(theme::TEXT_MUTED)
                        .child(icon_sized(kind_icon(k), theme::ICON_SM)),
                )
                .child(div().w(px(72.)).flex_none().fg().child(name))
                .child(div().flex_1().min_w_0().fg_muted().child(body))
                .child(div().flex_none().min_w(px(60.)).flex().justify_end().when(
                    !empty && kind == k,
                    |d| d.child(Badge::new("detected").accent()),
                ))
                .id(id)
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .child(
                step_head("1", "Get a skill").child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_label()
                        .fg_muted()
                        .ellipsis()
                        .child("one field for links, commands and requests; nothing installs before you review"),
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_label()
                            .fg_muted()
                            .child("Paste a link or an install command, or say what you need in words"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(div().flex_1().min_w_0().child(field))
                            .child(
                                Button::new("sk-go", button_label)
                                    .primary()
                                    .small()
                                    .disabled(busy.is_some() || empty)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.skills_go(window, cx)
                                    })),
                            ),
                    )
                    .when(confirming && busy.is_none(), |d| {
                        d.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_label()
                                .text_color(theme::WARNING)
                                .child(Clamped::new(
                                    "sk-confirm",
                                    format!(
                                        "This runs the command in an empty temp folder: {text}. Click Confirm run to go ahead, or edit the text to cancel."
                                    ),
                                    3,
                                )),
                        )
                    })
                    .when_some(sk.error.clone(), |d, e| {
                        d.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_label()
                                .text_color(theme::DANGER)
                                .child(Clamped::new("sk-error", e, 4)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(way(
                        "sk-way-link",
                        SourceKind::Link,
                        "Link",
                        div()
                            .ellipsis()
                            .child("GitHub repo or folder, .zip, raw SKILL.md · downloaded, nothing runs")
                            .into_any_element(),
                    ))
                    .child(way(
                        "sk-way-command",
                        SourceKind::Command,
                        "Command",
                        div()
                            .flex()
                            .gap(theme::S1)
                            .child(
                                div()
                                    .flex_none()
                                    .text_mono_sm()
                                    .text_color(theme::TEXT_SECONDARY)
                                    .child("npx skills add owner/repo"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .ellipsis()
                                    .child("· runs in an empty temp folder, you confirm first"),
                            )
                            .into_any_element(),
                    ))
                    .child(way(
                        "sk-way-words",
                        SourceKind::Words,
                        "Words",
                        div()
                            .ellipsis()
                            .child("“a skill for release notes” · the Orchestrator searches in a chat and stages what it finds")
                            .into_any_element(),
                    )),
            )
    }

    // ---- step 2 ----------------------------------------------------------------

    fn skills_review_step(&self, cx: &mut Context<Self>) -> Div {
        let sk = &self.sk;
        let into = skills::display_path(&skills::install_dir(&self.saved));
        let n = sk.staged.len();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .border_t_1()
            .border_color(theme::LINE)
            .child(
                step_head("2", "Review and install")
                    .when(n > 0, |d| {
                        d.child(Badge::new(format!("{n} waiting")).accent())
                    })
                    .child(div().flex_1())
                    .child(div().flex_none().text_label().fg_muted().child("into"))
                    .child(mono_box(into).max_w(px(300.))),
            )
            .when(n == 0, |d| {
                d.child(hint("Nothing waiting. Fetched skills show up here for review."))
            })
            .children(sk.staged.iter().map(|set| self.staged_card(set, cx)))
    }

    fn staged_card(&self, set: &StagedSet, cx: &mut Context<Self>) -> Div {
        let empty = SetPicks::new();
        let picks = self.sk.picks.get(&set.id).unwrap_or(&empty);
        let n_picked = installs(set, picks).len();
        let working = self.sk.working.contains(&set.id);
        let suffix = rename_suffix(set);

        let origin = match (&set.actor, &set.chat_title, set.kind) {
            (Some(actor), Some(title), _) => format!("staged by {actor} in “{title}”"),
            (Some(actor), None, _) => format!("staged by {actor}"),
            (None, _, SourceKind::Command) => "from command".to_string(),
            (None, _, _) => "from link".to_string(),
        };
        let info = format!(
            "{origin} · {} · {} · {} · {}",
            local_hm(set.created_at),
            plural(set.skills.len(), "skill"),
            plural(set.files, "file"),
            fmt_size(set.bytes)
        );
        let src_dir = skills::staged_src(&set.id);

        let head = div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .child(
                div()
                    .flex_none()
                    .text_color(theme::TEXT_MUTED)
                    .child(icon_sized(kind_icon(set.kind), theme::ICON_SM)),
            )
            .child(
                div()
                    .min_w_0()
                    .max_w(px(260.))
                    .text_mono()
                    .fg()
                    .ellipsis()
                    .child(set.label.clone()),
            )
            .when_some(set.commit.clone(), |d, c| {
                d.child(
                    div()
                        .flex_none()
                        .text_mono()
                        .fg_muted()
                        .child(format!("@ {c}")),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_label()
                    .fg_muted()
                    .ellipsis()
                    .child(info),
            )
            .child(
                Button::new(SharedString::from(format!("skf-view-{}", set.id)), "View files")
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_with_system(&src_dir))),
            );

        let mut rows = Vec::new();
        for (ix, s) in set.skills.iter().enumerate() {
            let pick = picks.get(&s.name).copied().unwrap_or_else(|| default_pick(s));
            let checked = is_picked(s, &pick);
            let (set_id, name) = (set.id.clone(), s.name.clone());
            let check = Checkbox::new(SharedString::from(format!("skf-{}-{}", set.id, s.name)))
                .small()
                .checked(checked)
                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.with_pick(&set_id, &name, cx, |sk, p| {
                        if sk.clash.is_some() {
                            p.how = if on { How::Rename } else { How::Skip };
                        } else {
                            p.on = on;
                        }
                    });
                }));

            let mut body = div().flex().flex_col().gap(theme::S1).flex_1().min_w_0();
            let mut right = div().flex_none();
            match &s.clash {
                Some(clash) => {
                    body = body
                        .child(div().fg_muted().child(Clamped::new(
                            SharedString::from(format!("skf-hint-{}-{}", set.id, s.name)),
                            format!(
                                "You already have {} in {clash}; that one keeps winning unless you replace it.",
                                s.name
                            ),
                            3,
                        )))
                        .child(div().flex().child(self.clash_choice(set, s, pick.how, &suffix, cx)));
                }
                None => {
                    body = body.child(div().fg_muted().child(Clamped::new(
                        SharedString::from(format!("skf-desc-{}-{}", set.id, s.name)),
                        s.description.clone(),
                        3,
                    )));
                    right = right.child(if s.scripts > 0 {
                        Badge::new(format!("has scripts · {}", s.scripts)).warning()
                    } else {
                        Badge::new("new").success()
                    });
                }
            }
            let row = div()
                .flex()
                .items_start()
                .gap(px(10.))
                .py(px(7.))
                .when(ix > 0, |d| d.border_t_1().border_color(theme::LINE_SUBTLE))
                .text_label()
                .child(div().flex_none().w(px(16.)).mt(px(1.)).child(check))
                .child(
                    div()
                        .w(px(116.))
                        .flex_none()
                        .text_mono()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .fg()
                        .ellipsis()
                        .child(s.name.clone()),
                )
                .child(body)
                .child(right);
            rows.push(row);
        }

        let (install_id, discard_id) = (set.id.clone(), set.id.clone());
        let footer = div()
            .flex()
            .items_center()
            .gap(theme::S2)
            .pt(theme::S2)
            .border_t_1()
            .border_color(theme::LINE_SUBTLE)
            .child(
                div().flex_1().min_w_0().child(hint(
                    "Scripts inside a skill run only through an agent’s shell tool, like any other command. Installing doesn’t run anything.",
                )),
            )
            .child(
                Button::new(SharedString::from(format!("skf-discard-{}", set.id)), "Discard")
                    .ghost()
                    .small()
                    .disabled(working)
                    .on_click(cx.listener(move |this, _, _, cx| this.discard_set(&discard_id, cx))),
            )
            .child(
                Button::new(
                    SharedString::from(format!("skf-install-{}", set.id)),
                    if working {
                        "Installing…".to_string()
                    } else {
                        format!("Install {}", plural(n_picked, "skill"))
                    },
                )
                .primary()
                .small()
                .icon(AppIcon::Download)
                .disabled(working || n_picked == 0)
                .on_click(cx.listener(move |this, _, _, cx| this.install_set(&install_id, cx))),
            );

        div()
            .flex()
            .flex_col()
            .gap(theme::S2)
            .px(theme::S3)
            .py(px(10.))
            .rounded(theme::RADIUS_LG)
            .border_1()
            .border_color(theme::LINE)
            .bg(theme::SURFACE)
            .child(head)
            .child(div().flex().flex_col().children(rows))
            .child(footer)
    }

    /// Skip / Replace mine / Install as <name>-<suffix>.
    fn clash_choice(
        &self,
        set: &StagedSet,
        s: &StagedSkill,
        current: How,
        suffix: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let items = [
            (How::Skip, "Skip".to_string()),
            (How::Replace, "Replace mine".to_string()),
            (How::Rename, format!("Install as {}-{suffix}", s.name)),
        ];
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .p(px(2.))
            .rounded(theme::RADIUS_MD)
            .border_1()
            .border_color(theme::LINE_STRONG)
            .bg(theme::SURFACE_RAISED)
            .children(items.into_iter().map(|(how, label)| {
                let active = how == current;
                let (set_id, name) = (set.id.clone(), s.name.clone());
                div()
                    .id(SharedString::from(format!(
                        "skf-how-{}-{}-{how:?}",
                        set.id, s.name
                    )))
                    .h(px(22.))
                    .px(theme::S2)
                    .flex()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme::SURFACE_HOVER).text_color(theme::TEXT))
                    .when(!active, |d| {
                        d.text_color(theme::TEXT_MUTED)
                            .hover(|s| s.text_color(theme::TEXT))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.with_pick(&set_id, &name, cx, |_, p| p.how = how);
                    }))
            }))
    }

    // ---- installed ---------------------------------------------------------------

    fn skills_installed(&self, cx: &mut Context<Self>) -> Div {
        let sk = &self.sk;
        let dir_rows = self.form.skill_dirs.iter().map(|(uid, e)| {
            let uid = *uid;
            div()
                .flex()
                .items_center()
                .gap(theme::S2)
                .mb(theme::S1)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(block_plain(input::field_sm(e), None)),
                )
                .child(
                    IconButton::new(("dir-del", uid), AppIcon::X)
                        .small()
                        .tooltip("Remove folder")
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_skill_dir(uid, cx)),
                        ),
                )
        });

        let rows = sk.rows.iter().enumerate().map(|(ix, r)| {
            let (icon, source) = source_line(&r.skill, r.prov.as_ref());
            let dir = r.skill.path.parent().map(|p| p.to_path_buf());
            let usage = match usage_label(&r.users) {
                Some((who, when)) => div()
                    .flex()
                    .min_w_0()
                    .gap(px(4.))
                    .child(
                        div()
                            .min_w_0()
                            .text_color(theme::TEXT_SECONDARY)
                            .ellipsis()
                            .child(who),
                    )
                    .child(div().flex_none().child(format!("· {when}")))
                    .into_any_element(),
                None => div()
                    .text_color(theme::TEXT_DISABLED)
                    .child("no agent yet")
                    .into_any_element(),
            };
            div()
                .id(("sk-inst", ix))
                .flex()
                .items_center()
                .gap(px(12.))
                .py(theme::S2)
                .border_t_1()
                .border_color(theme::LINE_SUBTLE)
                .text_label()
                .hover(|s| s.bg(theme::SURFACE_RAISED))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme::S2)
                                .min_w_0()
                                .child(
                                    div()
                                        .flex_none()
                                        .text_mono()
                                        .text_size(px(12.))
                                        .line_height(px(18.))
                                        .fg()
                                        .child(r.skill.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .gap(px(4.))
                                        .fg_muted()
                                        .child(div().flex_none().child(icon_sized(icon, px(12.))))
                                        .child(div().flex_1().min_w_0().ellipsis().child(source)),
                                ),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .ellipsis()
                                .child(r.skill.description.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .max_w(px(220.))
                        .min_w_0()
                        .fg_muted()
                        .child(usage),
                )
                .child(
                    Button::new(("sk-open", ix), "Open")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(d) = dir.clone() {
                                cx.open_with_system(&d);
                            }
                        })),
                )
        });

        let dirs: Vec<String> = if self.saved.skill_dirs.is_empty() {
            vec![skills::display_path(&skills::install_dir(&self.saved))]
        } else {
            self.saved.skill_dirs.clone()
        };
        let folders: PathBuf = PathBuf::from(dirs.join(", "));

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme::S2)
                    .mb(theme::S1)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_label()
                            .fg_muted()
                            .ellipsis()
                            .child("Installed · earlier folders win on name clashes"),
                    )
                    .child(
                        Button::new("dir-add", "Add folder")
                            .ghost()
                            .small()
                            .icon(AppIcon::Plus)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_skill_dir(window, cx)),
                            ),
                    ),
            )
            .children(dir_rows)
            .when(sk.rows.is_empty(), |d| {
                d.child(hint("No skills found in the saved folders."))
            })
            .children(rows)
            .child(
                div()
                    .pt(theme::S2)
                    .border_t_1()
                    .border_color(theme::LINE_SUBTLE)
                    .text_label()
                    .fg_muted()
                    .child(format!(
                        "Folders: {} (installs go into the first). Pick which agent gets which skill in Pipeline editor › node.",
                        folders.display()
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, clash: Option<&str>) -> StagedSkill {
        StagedSkill {
            name: name.into(),
            description: String::new(),
            rel_dir: name.into(),
            files: 1,
            scripts: 0,
            clash: clash.map(String::from),
        }
    }

    fn set(source: &str, skills: Vec<StagedSkill>) -> StagedSet {
        StagedSet {
            id: "s1".into(),
            source: source.into(),
            kind: SourceKind::Link,
            label: "acme/agent-skills".into(),
            commit: None,
            created_at: Utc::now(),
            chat_id: None,
            chat_title: None,
            actor: None,
            files: 3,
            bytes: 2048,
            skills,
        }
    }

    #[test]
    fn defaults_pick_everything_but_clashes() {
        let s = set(
            "https://github.com/acme/agent-skills/tree/main/documents",
            vec![skill("pdf", None), skill("brand-voice", Some("~/skills"))],
        );
        let picks: SetPicks = s
            .skills
            .iter()
            .map(|k| (k.name.clone(), default_pick(k)))
            .collect();
        let got = installs(&s, &picks);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "pdf");
    }

    #[test]
    fn clash_choices_map_to_install_modes() {
        let s = set(
            "https://github.com/acme/agent-skills/tree/main/documents",
            vec![skill("brand-voice", Some("~/skills"))],
        );
        let mut picks = SetPicks::new();
        picks.insert(
            "brand-voice".into(),
            Pick {
                on: false,
                how: How::Rename,
            },
        );
        assert_eq!(
            installs(&s, &picks),
            vec![("brand-voice".into(), Clash::Rename("brand-voice-acme".into()))]
        );
        picks.get_mut("brand-voice").unwrap().how = How::Replace;
        assert_eq!(
            installs(&s, &picks),
            vec![("brand-voice".into(), Clash::Replace)]
        );
        picks.get_mut("brand-voice").unwrap().how = How::Skip;
        assert!(installs(&s, &picks).is_empty());
    }

    #[test]
    fn unchecked_skill_is_left_out() {
        let s = set("npx skills add x/y", vec![skill("a", None), skill("b", None)]);
        let mut picks = SetPicks::new();
        picks.insert("a".into(), Pick { on: false, how: How::Skip });
        picks.insert("b".into(), Pick { on: true, how: How::Skip });
        let got = installs(&s, &picks);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "b");
    }

    #[test]
    fn rename_suffix_is_owner_or_2() {
        let s = set("https://github.com/acme/x", vec![]);
        assert_eq!(renamed("a", &s), "a-acme");
        let s = set("npx skills add x/y", vec![]);
        assert_eq!(renamed("a", &s), "a-2");
    }

    #[test]
    fn sizes() {
        assert_eq!(fmt_size(12), "12 B");
        assert_eq!(fmt_size(48 * 1024), "48 KB");
        assert_eq!(fmt_size(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }

    #[test]
    fn usage_text() {
        assert_eq!(usage_label(&[]), None);
        assert_eq!(
            usage_label(&[("Fixer".into(), true)]),
            Some(("Fixer".into(), "always"))
        );
        assert_eq!(
            usage_label(&[("A".into(), false), ("B".into(), false), ("C".into(), false)]),
            Some(("3 agents".into(), "on demand"))
        );
        assert_eq!(
            usage_label(&[("A".into(), true), ("B".into(), false)]),
            Some(("2 agents".into(), "always / on demand"))
        );
    }

    #[test]
    fn source_lines() {
        let sk = Skill {
            name: "x".into(),
            description: String::new(),
            path: PathBuf::from("/tmp/x/SKILL.md"),
            dir_label: "~/skills".into(),
        };
        assert_eq!(source_line(&sk, None), (AppIcon::Folder, "~/skills".into()));
        let mut p = Provenance {
            source: "https://github.com/a/b".into(),
            kind: SourceKind::Link,
            label: "a/b/x".into(),
            commit: Some("2d1e0b7".into()),
            installed_at: Utc::now(),
            chat_title: None,
            actor: None,
        };
        assert_eq!(
            source_line(&sk, Some(&p)),
            (AppIcon::Link, "a/b/x @ 2d1e0b7".into())
        );
        p.kind = SourceKind::Command;
        let (icon, text) = source_line(&sk, Some(&p));
        assert_eq!(icon, AppIcon::Terminal);
        assert!(text.starts_with("a/b/x · "));
        p.actor = Some("Orchestrator".into());
        p.chat_title = Some("research".into());
        let (icon, text) = source_line(&sk, Some(&p));
        assert_eq!(icon, AppIcon::Chat);
        assert!(text.starts_with("Orchestrator · chat “research” · "));
    }
}
