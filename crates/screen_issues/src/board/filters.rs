//! What narrows the board and the list: the "Runs with" chips of the mockup, and the filter
//! popover (text, owner, type, priority) behind the Filter button.

use std::collections::BTreeSet;

use gpui::{AnyElement, App, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::api::{Issue, IssueType, Priority, RunnableRef};
use ui::dev::Probe;
use ui::prelude::*;

use super::IssuesView;

/// Probe and anchor of the Filter button.
pub(crate) const FILTER: &str = "issues-filter";

/// The "Runs with" chip that is pressed: what is attached to an issue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RunsWith {
    #[default]
    All,
    Orchestrators,
    Agents,
    Pipelines,
    Unattached,
}

impl RunsWith {
    /// The chips, left to right.
    pub(crate) const ALL: [RunsWith; 5] =
        [RunsWith::All, RunsWith::Orchestrators, RunsWith::Agents, RunsWith::Pipelines, RunsWith::Unattached];

    /// The chip's label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RunsWith::All => "All",
            RunsWith::Orchestrators => "Orchestrators",
            RunsWith::Agents => "Agents",
            RunsWith::Pipelines => "Pipelines",
            RunsWith::Unattached => "Unattached",
        }
    }

    /// The chip's id and probe suffix.
    pub(crate) fn slug(self) -> &'static str {
        match self {
            RunsWith::All => "all",
            RunsWith::Orchestrators => "orchestrators",
            RunsWith::Agents => "agents",
            RunsWith::Pipelines => "pipelines",
            RunsWith::Unattached => "unattached",
        }
    }

    /// Whether an issue with `runner` attached belongs under this chip.
    pub(crate) fn matches(self, runner: Option<&RunnableRef>) -> bool {
        let scheme = runner.map(RunnableRef::scheme);
        match self {
            RunsWith::All => true,
            RunsWith::Orchestrators => scheme == Some("orchestrator"),
            RunsWith::Agents => matches!(scheme, Some("agent" | "spawned")),
            RunsWith::Pipelines => scheme == Some("pipeline"),
            RunsWith::Unattached => scheme.is_none(),
        }
    }
}

/// Every filter of the screen; both views show only the issues that pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Filters {
    pub runs_with: RunsWith,
    /// Owners to keep; empty keeps every owner.
    pub owners: BTreeSet<String>,
    /// Types to keep; empty keeps every type.
    pub kinds: Vec<IssueType>,
    /// Priorities to keep; empty keeps every priority.
    pub priorities: Vec<Priority>,
    /// Words the key, title or description must contain, any case.
    pub text: String,
}

impl Filters {
    /// The issue passes every filter.
    pub(crate) fn matches(&self, issue: &Issue) -> bool {
        self.runs_with.matches(issue.runner.as_ref()) && self.matches_besides_runs_with(issue)
    }

    /// The issue passes every filter but the "Runs with" chip (what the chips count).
    pub(crate) fn matches_besides_runs_with(&self, issue: &Issue) -> bool {
        (self.owners.is_empty() || self.owners.contains(&issue.owner))
            && (self.kinds.is_empty() || self.kinds.contains(&issue.kind))
            && (self.priorities.is_empty() || self.priorities.contains(&issue.priority))
            && self.matches_text(issue)
    }

    fn matches_text(&self, issue: &Issue) -> bool {
        let words: Vec<String> = self.text.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return true;
        }
        let haystack = format!("{} {} {}", issue.key, issue.title, issue.body).to_lowercase();
        words.iter().all(|word| haystack.contains(word))
    }

    /// How many filters of the popover are on (the chip is not counted).
    pub(crate) fn popover_count(&self) -> usize {
        usize::from(!self.owners.is_empty())
            + usize::from(!self.kinds.is_empty())
            + usize::from(!self.priorities.is_empty())
            + usize::from(!self.text.trim().is_empty())
    }

    /// Nothing is filtered out.
    pub(crate) fn is_empty(&self) -> bool {
        self.runs_with == RunsWith::All && self.popover_count() == 0
    }

    /// Adds `owner` to the kept owners, or takes it out.
    pub(crate) fn toggle_owner(&mut self, owner: &str) {
        if !self.owners.remove(owner) {
            self.owners.insert(owner.to_string());
        }
    }

    /// Adds `kind` to the kept types, or takes it out.
    pub(crate) fn toggle_kind(&mut self, kind: IssueType) {
        toggle(&mut self.kinds, kind);
    }

    /// Adds `priority` to the kept priorities, or takes it out.
    pub(crate) fn toggle_priority(&mut self, priority: Priority) {
        toggle(&mut self.priorities, priority);
    }
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
    match list.iter().position(|known| *known == value) {
        Some(ix) => {
            list.remove(ix);
        }
        None => list.push(value),
    }
}

/// The label of the Filter button: "Filter", or "Filter · 2" with two filters on.
pub(crate) fn button_label(filters: &Filters) -> SharedString {
    match filters.popover_count() {
        0 => "Filter".into(),
        n => format!("Filter · {n}").into(),
    }
}

/// The filter popover: a search field, then owners, types and priorities to tick.
pub(crate) fn popover(view: &Entity<IssuesView>, window: &mut Window, cx: &mut App) -> AnyElement {
    let this = view.read(cx);
    let filters = this.filters.clone();
    let owners = this.owners(cx);
    let search = this.search.clone();
    let focused = search.read(cx).is_focused(window);
    let field = InputFrame::new("issues-filter-search").small().sunken().icon(IconName::Search).focused(focused).child(search);

    let mut pop = Popover::new().width(300.).search(field).child(PopLabel::new("Owner"));
    for (ix, owner) in owners.into_iter().enumerate() {
        let on = filters.owners.contains(&owner);
        let label = if owner.is_empty() { "No owner".to_string() } else { owner.clone() };
        let v = view.clone();
        pop = pop.child(
            MenuItem::new(("issues-filter-owner", ix), label)
                .check_column()
                .checked(on)
                .on_click(move |_, _, cx| v.update(cx, |this, cx| this.edit_filters(|f| f.toggle_owner(&owner), cx))),
        );
    }
    pop = pop.child(PopSeparator).child(PopLabel::new("Type"));
    for kind in IssueType::ALL {
        let on = filters.kinds.contains(&kind);
        let v = view.clone();
        pop = pop.child(
            MenuItem::new(("issues-filter-type", kind as usize), kind.label())
                .check_column()
                .checked(on)
                .on_click(move |_, _, cx| v.update(cx, |this, cx| this.edit_filters(|f| f.toggle_kind(kind), cx))),
        );
    }
    pop = pop.child(PopSeparator).child(PopLabel::new("Priority"));
    for priority in [Priority::High, Priority::Medium, Priority::Low] {
        let on = filters.priorities.contains(&priority);
        let v = view.clone();
        pop = pop.child(
            MenuItem::new(("issues-filter-priority", priority as usize), priority.label())
                .check_column()
                .checked(on)
                .on_click(move |_, _, cx| {
                    v.update(cx, |this, cx| this.edit_filters(|f| f.toggle_priority(priority), cx))
                }),
        );
    }
    if !filters.is_empty() {
        let v = view.clone();
        pop = pop.child(PopSeparator).child(
            MenuItem::new("issues-filter-clear", "Clear filters")
                .icon(IconName::X)
                .on_click(move |_, window, cx| v.update(cx, |this, cx| this.clear_filters(window, cx))),
        );
    }
    div().child(pop).probe("issues-filter-popover").into_any_element()
}

/// What the board shows when filters leave nothing: a line and a way back.
pub(crate) fn nothing_matches(view: &Entity<IssuesView>) -> impl IntoElement {
    let v = view.clone();
    col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(SPACE_3))
        .child(div().type_style(BODY).text_color(TEXT_MUTED).child("No issue matches these filters."))
        .child(
            Button::new("issues-filters-reset", "Clear filters")
                .secondary()
                .small()
                .on_click(move |_, window, cx| v.update(cx, |this, cx| this.clear_filters(window, cx))),
        )
}
