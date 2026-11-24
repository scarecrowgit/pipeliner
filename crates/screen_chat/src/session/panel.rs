//! The context panel at the right of a chat (`chat_panel()` in the mockups): workdir,
//! context use and cost, skills in context, todo list, modified files, sub-agents, LSP
//! (coming soon) and MCP servers.

use gpui::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{McpServerInfo, McpState, RepoHost, Session, SessionDetail, SessionState, TodoStatus};
use hub::{ContextUsage, Route};
use ui::dev::Probe;
use ui::prelude::*;

use super::SessionView;

/// Width of the chat's context panel, px (`chat_panel()` draws it 320 wide).
const PANEL_WIDTH: f32 = 320.;
/// Anchor key of the "Manage skills" link.
const SKILLS_ANCHOR: &str = "chat-panel-manage-skills";

/// The whole panel.
pub(super) fn panel(this: &SessionView, cx: &mut Context<SessionView>) -> impl IntoElement {
    let view = cx.entity();
    let model = this.model.read(cx);
    let detail = model.detail().cloned();
    let usage = model.context();
    let session = this.header_session(cx).cloned();
    let compact_at = this.ops.read(cx).settings().value().map(|s| s.compact_at);
    let max_depth = session
        .as_ref()
        .and_then(|s| this.catalog.read(cx).runnable(&s.runnable))
        .and_then(|r| r.agent.as_ref())
        .map(|a| a.spawn.max_depth);

    let mut sections: Vec<AnyElement> = Vec::new();
    if let Some(section) = session.as_ref().and_then(|s| workdir(&view, s, detail.as_deref(), cx)) {
        sections.push(section);
    }
    sections.push(context(&view, usage, compact_at).into_any_element());
    if let Some(detail) = detail.as_deref() {
        sections.push(skills(&view, detail).into_any_element());
        sections.push(todos(detail).into_any_element());
        sections.push(files(&view, detail).into_any_element());
        sections.push(subagents(detail, max_depth).into_any_element());
    }
    sections.push(lsp().into_any_element());
    if let Some(detail) = detail.as_deref() {
        sections.push(mcp(&view, detail).into_any_element());
    }
    div()
        .id("chat-panel")
        .debug_selector(|| "chat-panel".into())
        .flex()
        .flex_col()
        .flex_none()
        .w(px(PANEL_WIDTH))
        .h_full()
        .border_l_1()
        .border_color(LINE)
        .bg(BG)
        .overflow_y_scroll()
        .children(sections)
        .probe("chat-panel")
}

/// A panel row with a name the tests and the dev driver find it by.
fn named(name: impl Into<SharedString>) -> gpui::Stateful<gpui::Div> {
    let name: SharedString = name.into();
    let selector = name.to_string();
    row().id(gpui::ElementId::Name(name)).debug_selector(move || selector).gap(px(SPACE_2)).min_h(px(24.))
}

fn empty(text: &'static str) -> impl IntoElement {
    div().type_style(LABEL).text_color(TEXT_MUTED).child(text)
}

fn meta(text: impl Into<SharedString>) -> impl IntoElement {
    div().flex_none().child(text.into())
}


/// "mac-mini · ~/.harness/worktrees/payments-api/fix-PAY-1423": the runtime and the
/// home-relative path; the panel cuts its middle to fit.
pub(crate) fn workdir_place(runtime: &str, path: &str) -> String {
    let runtime = if runtime.is_empty() { "local" } else { runtime };
    format!("{runtime} · {path}")
}

fn workdir(view: &Entity<SessionView>, session: &Session, detail: Option<&SessionDetail>, cx: &App) -> Option<AnyElement> {
    let workdir = session.workdir.as_ref()?;
    let name = match &workdir.branch {
        Some(branch) => format!("{} · {branch}", workdir.repo),
        None => workdir.repo.clone(),
    };
    let host = match workdir.host {
        RepoHost::GitHub => "GitHub",
        RepoHost::GitLab => "GitLab",
        RepoHost::Local => "Local",
    };
    let place = workdir_place(&workdir.runtime, &hub::display_path(&workdir.path, cx));
    let v = view.clone();
    let change = Link::new("chat-panel-workdir-change", "Change")
        .on_click(move |_, window, cx| v.update(cx, |this, cx| this.workdir.update(cx, |chip, cx| chip.open(window, cx))))
        .probe("chat-panel-workdir-change");
    let git = detail.and_then(|d| d.repo.as_ref()).map(|repo| {
        let mut words = vec![base::fmt::plural(u64::from(repo.dirty), "uncommitted", "uncommitted")];
        if let Some(against) = &repo.compared_to {
            words.push(format!("{} ahead of {against}", repo.ahead));
        }
        words.join(" · ")
    });
    Some(
        Section::new("Workdir")
            .meta(change)
            .child(
                col()
                    .gap(px(SPACE_1))
                    .child(
                        row()
                            .gap(px(SPACE_2))
                            .child(truncate(name).type_style(MONO).font_weight(gpui::FontWeight::MEDIUM))
                            .child(Badge::new(host).square()),
                    )
                    .child(
                        crate::workdir::common::path_line(place.clone(), MONO_SM.line)
                            .type_style(MONO_SM)
                            .text_color(TEXT_MUTED)
                            .probe_value("chat-panel-workdir-path", place),
                    )
                    .when_some(git, |d, git| d.child(div().mt(px(SPACE_2)).child(KeyValue::new().row("git", div().child(git))))),
            )
            .into_any_element(),
    )
}

/// The amount at the right of "Context": "48.2k of 200k", or just "131.3k" when the model's
/// window is not known.
pub(crate) fn context_amount(usage: ContextUsage) -> (String, Option<String>) {
    let used = base::fmt::tokens(usage.tokens);
    (used, (usage.window > 0).then(|| format!("of {}", base::fmt::tokens(usage.window))))
}

/// The line under the bar: "24% used · compacts at 90%"; without a known window there is no
/// share to claim.
pub(crate) fn context_line(usage: ContextUsage, compact_at: Option<f32>) -> String {
    if usage.window == 0 {
        return "Context window not reported for this model".to_string();
    }
    let pct = base::fmt::percent(usage.tokens as f64, usage.window as f64);
    let mut line = format!("{pct}% used");
    if let Some(at) = compact_at {
        line.push_str(&format!(" · compacts at {}%", (at * 100.).round() as u32));
    }
    line
}

fn context(view: &Entity<SessionView>, usage: ContextUsage, compact_at: Option<f32>) -> impl IntoElement {
    let (used, of) = context_amount(usage);
    let known = usage.window > 0;
    let v = view.clone();
    Section::new("Context")
        .meta(
            row()
                .gap(px(SPACE_1))
                .type_style(MONO)
                .tabular()
                .child(div().text_color(TEXT).child(used.clone()))
                .children(of.map(|of| div().text_color(TEXT_MUTED).child(of)))
                .probe_value("chat-panel-context", used),
        )
        .when(known, |d| d.child(Progress::new(usage.fraction())))
        .child(
            row()
                .when(known, |d| d.mt(px(SPACE_2)))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(truncate_lh(context_line(usage, compact_at), LABEL.line))
                .child(
                    Button::new("chat-panel-compact", "Compact now")
                        .ghost()
                        .small()
                        .on_click(move |_, _, cx| v.update(cx, |this, cx| hub::commands(cx).compact(&this.session, cx)))
                        .probe("chat-panel-compact"),
                ),
        )
        .child(
            row()
                .mt(px(SPACE_1_5))
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child("Cost"))
                .child(grow())
                .child(
                    row()
                        .gap(px(SPACE_1))
                        .type_style(MONO)
                        .tabular()
                        .child(base::fmt::cost(usage.cost_usd))
                        .child(div().text_color(TEXT_MUTED).child("this session"))
                        .probe("chat-panel-cost"),
                ),
        )
}

fn skills(view: &Entity<SessionView>, detail: &SessionDetail) -> impl IntoElement {
    let total: u64 = detail.skills.iter().map(|s| s.tokens).sum();
    let mut section = Section::new("Skills in context").meta(meta(format!("{} tokens", base::fmt::tokens(total))));
    if detail.skills.is_empty() {
        section = section.child(empty("No skills in context"));
    }
    for (ix, skill) in detail.skills.iter().enumerate() {
        let badge = if skill.forced { Badge::new("forced").tone(BadgeTone::Accent) } else { Badge::new("auto") };
        section = section.child(
            named(format!("chat-panel-skill-{ix}"))
                .child(truncate(skill.name.clone()).type_style(MONO))
                .child(badge)
                .probe(format!("chat-panel-skill-{ix}")),
        );
    }
    let v = view.clone();
    section.child(
        div().mt(px(SPACE_2)).flex().child(
            Link::new(SKILLS_ANCHOR, "Manage skills")
                .on_click(move |_, window, cx| v.update(cx, |this, cx| this.open_skills(window, cx)))
                .popover_anchor(SKILLS_ANCHOR)
                .probe(SKILLS_ANCHOR),
        ),
    )
}

fn todos(detail: &SessionDetail) -> impl IntoElement {
    let done = detail.todos.iter().filter(|t| t.status == TodoStatus::Done).count();
    let mut section = Section::new("Todo");
    if detail.todos.is_empty() {
        return section.child(empty("No todo list yet"));
    }
    section = section.meta(meta(format!("{done} of {}", detail.todos.len())));
    for (ix, item) in detail.todos.iter().enumerate() {
        let status = item.status;
        let check = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.))
            .rounded(px(RADIUS_SM))
            .border_1()
            .map(|d| match status {
                TodoStatus::Done => d
                    .bg(ACCENT_STRONG)
                    .border_color(ACCENT_STRONG)
                    .child(icon(IconName::Check, 10., ON_ACCENT)),
                TodoStatus::InProgress => d.bg(SURFACE_RAISED).border_color(ACCENT),
                TodoStatus::Pending => d.bg(SURFACE_RAISED).border_color(LINE_STRONG),
            });
        section = section.child(
            named(format!("chat-panel-todo-{ix}"))
                .h(px(28.))
                .child(check)
                .child(truncate(item.text.clone()).when(status == TodoStatus::Done, |d| d.text_color(TEXT_MUTED)))
                .probe(format!("chat-panel-todo-{ix}")),
        );
    }
    section
}

fn files(view: &Entity<SessionView>, detail: &SessionDetail) -> impl IntoElement {
    let mut section = Section::new("Modified files");
    if detail.files.is_empty() {
        return section.child(empty("No files changed yet"));
    }
    section = section.meta(meta(base::fmt::plural(detail.files.len() as u64, "file", "files")));
    for (ix, file) in detail.files.iter().enumerate() {
        let name = format!("chat-panel-file-{ix}");
        let (v, path) = (view.clone(), file.path.clone());
        let faded = motion::fade_row(named(name.clone()).px(px(SPACE_1)).mx(px(-SPACE_1)).rounded(px(RADIUS_SM)).cursor_pointer(), gpui::ElementId::Name(name.clone().into()), false, SURFACE_HOVER);
        section = section.child(
            faded
                .on_click(move |_, window, cx| v.update(cx, |this, cx| this.mention_file(&path, window, cx)))
                .child(truncate(file.path.clone()).type_style(MONO))
                .child(
                    row()
                        .flex_none()
                        .gap(px(SPACE_1))
                        .type_style(MONO)
                        .child(div().text_color(SUCCESS).child(format!("+{}", file.added)))
                        .child(div().text_color(DANGER).child(format!("−{}", file.removed))),
                )
                .probe(name),
        );
    }
    section
}

fn subagents(detail: &SessionDetail, max_depth: Option<u32>) -> impl IntoElement {
    let mut section = Section::new("Sub-agents");
    if detail.children.is_empty() {
        return section.child(empty("No sub-agents yet"));
    }
    let top = detail.session.depth;
    let deepest = detail.children.iter().map(|c| c.depth.saturating_sub(top)).max().unwrap_or(1);
    let mut count = detail.children.len().to_string();
    if let Some(max) = max_depth {
        count.push_str(&format!(" · depth {deepest} of {max}"));
    }
    section = section.meta(meta(count));
    for (ix, child) in detail.children.iter().enumerate() {
        let name = format!("chat-panel-subagent-{ix}");
        let (word, color) = match child.state {
            SessionState::Running => ("running", INFO),
            SessionState::Waiting => ("waiting", WARNING),
            SessionState::Failed => ("failed", DANGER),
            SessionState::Done => ("done", SUCCESS),
            SessionState::Idle => ("idle", TEXT_MUTED),
        };
        let id = child.id.clone();
        let faded = motion::fade_row(named(name.clone()).px(px(SPACE_1)).mx(px(-SPACE_1)).rounded(px(RADIUS_SM)).cursor_pointer(), gpui::ElementId::Name(name.clone().into()), false, SURFACE_HOVER);
        section = section.child(
            faded
                .on_click(move |_, _, cx| hub::navigate(Route::Chat { session: id.clone() }, cx))
                .child(StatusDot::new(Status::Spawn).small())
                .child(div().flex_none().type_style(MONO).text_color(SPAWN_TEXT).child(format!("@{}", child.runnable.id())))
                .child(truncate(child.title.clone()).text_color(TEXT_MUTED))
                .child(div().flex_none().type_style(LABEL).text_color(color).child(word))
                .probe(name),
        );
    }
    section
}

fn lsp() -> impl IntoElement {
    Section::new("LSP")
        .meta(Tag::new("Coming soon"))
        .child(empty("Language server diagnostics show here"))
}

fn mcp(view: &Entity<SessionView>, detail: &SessionDetail) -> impl IntoElement {
    let mut section = Section::new("MCP servers").last();
    if detail.mcp.is_empty() {
        return section.child(empty("No MCP servers"));
    }
    section = section.meta(meta(detail.mcp.len().to_string()));
    let mut list = KeyValue::new();
    for (ix, server) in detail.mcp.iter().enumerate() {
        list = list.mono_row(server.name.clone(), mcp_value(view, ix, server));
    }
    section.child(list)
}

/// What a server in a state without detail reads as: `None` for the states that show more.
pub(crate) fn mcp_state_word(state: McpState) -> Option<&'static str> {
    match state {
        McpState::Idle => Some("not started"),
        McpState::Starting => Some("starting"),
        McpState::Disabled => Some("off"),
        McpState::Ready | McpState::Failed => None,
    }
}

fn mcp_value(view: &Entity<SessionView>, ix: usize, server: &McpServerInfo) -> AnyElement {
    let name = format!("chat-panel-mcp-{ix}");
    let cell = named(name.clone()).type_style(LABEL).min_h(px(20.));
    let el = match server.state {
        McpState::Ready => cell.text_color(TEXT_MUTED).child(base::fmt::plural(u64::from(server.tools), "tool", "tools")),
        McpState::Idle | McpState::Starting | McpState::Disabled => {
            cell.text_color(TEXT_MUTED).child(mcp_state_word(server.state).unwrap_or_default())
        }
        McpState::Failed => {
            let (v, server_name) = (view.clone(), server.name.clone());
            let reconnect = format!("{name}-reconnect");
            cell.child(div().text_color(DANGER).child(server.error.clone().unwrap_or_else(|| "failed".into())))
                .child(
                    Link::new(gpui::ElementId::Name(reconnect.clone().into()), "Reconnect")
                        .on_click(move |_, _, cx| v.update(cx, |this, cx| this.reconnect(&server_name, cx)))
                        .probe(reconnect),
                )
        }
    };
    el.probe(name).into_any_element()
}

impl SessionView {
    /// A modified file's row: mention it in the message being written.
    fn mention_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.read(cx).disabled_reason().is_some() {
            return;
        }
        let path = path.to_string();
        self.composer.update(cx, |c, cx| {
            let lead = if c.text().is_empty() || c.text().ends_with(' ') { "" } else { " " };
            c.insert_text(&format!("{lead}@{path} "), cx);
            c.focus(window, cx);
        });
    }

    fn reconnect(&mut self, server: &str, cx: &mut Context<Self>) {
        hub::commands(cx).reconnect_mcp(&self.session, server, cx);
    }

    /// "Manage skills": every installed skill, checked when forced into the session.
    fn open_skills(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = Anchor::of(SKILLS_ANCHOR, window, cx);
        let view = cx.entity();
        ui::overlays(window, cx).open_menu(anchor, Placement::below().end(), move |_, cx| skills_menu(&view, cx), window, cx);
    }

    fn toggle_forced_skill(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(session) = self.header_session(cx) else { return };
        let mut forced = session.forced_skills.clone();
        match forced.iter().position(|s| s == name) {
            Some(ix) => {
                forced.remove(ix);
            }
            None => forced.push(name.to_string()),
        }
        hub::commands(cx).set_forced_skills(&self.session, forced, cx);
    }
}

fn skills_menu(view: &Entity<SessionView>, cx: &App) -> Menu {
    let this = view.read(cx);
    let forced: Vec<String> = this.header_session(cx).map(|s| s.forced_skills.clone()).unwrap_or_default();
    let mut menu = Menu::new("chat-skills-menu").width(320.).max_height(360.).label("Forced into every request");
    let skills = this.catalog.read(cx).skills().value().cloned();
    match skills {
        Some(skills) if !skills.is_empty() => {
            for (ix, skill) in skills.iter().enumerate() {
                let (v, name) = (view.clone(), skill.name.clone());
                menu = menu.item(
                    MenuItem::new(("chat-skills-menu", ix), skill.name.clone())
                        .check_column()
                        .checked(forced.contains(&skill.name))
                        .subline(skill.description.clone())
                        .hint(format!("{} tokens", base::fmt::tokens(skill.tokens)))
                        .on_click(move |_, _, cx| v.update(cx, |this, cx| this.toggle_forced_skill(&name, cx))),
                );
            }
        }
        Some(_) => menu = menu.item(MenuItem::new("chat-skills-none", "No skills installed").disabled(true)),
        None => menu = menu.item(MenuItem::new("chat-skills-loading", "Loading skills…").disabled(true)),
    }
    menu
}
