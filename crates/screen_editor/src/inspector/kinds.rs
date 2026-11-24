//! The smaller node kinds: source, file, review, action and nested pipeline.

use std::rc::Rc;

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};
use hub::model::{ActionNode, FileMode, GraphDef, IssueStatus, NodeDef, NodeKind, SourceKind};
use ui::dev::Probe;
use ui::prelude::*;

use super::fields::{Spec, count_value, on_kind, parse_count};
use super::form::{INNER, OnPick, Pick, field, note, problem_line, psec, row_field};
use super::{InspectorView, ops};

/// Source kinds in picker order, with whether they work yet.
const SOURCES: [(SourceKind, &str, &str, bool); 5] = [
    (SourceKind::Issue, "Issue", "An issue attached to the pipeline on the board", true),
    (SourceKind::Manual, "Manual", "Started by hand or from the CLI", true),
    (SourceKind::Schedule, "Schedule", "An autopilot tick", true),
    (SourceKind::Jira, "Jira", "Issues pulled from a Jira query", false),
    (SourceKind::Sentry, "Sentry", "New Sentry issues", false),
];

/// Changes the action on show.
fn on_action(f: impl Fn(&mut ActionNode, &str) + 'static) -> super::fields::Apply {
    on_kind(move |kind, text| {
        if let NodeKind::Action(action) = kind {
            f(action, text);
        }
    })
}

/// Changes the action on show, as one undo step.
fn action_edit(f: impl FnOnce(&mut ActionNode)) -> impl FnOnce(&mut NodeDef) {
    move |node| {
        if let NodeKind::Action(action) = &mut node.kind {
            f(action);
        }
    }
}

impl InspectorView {
    pub(super) fn kind_sections(&mut self, graph: &GraphDef, node: &NodeDef, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match &node.kind {
            NodeKind::Source(source) => vec![self.source_section(node, source.source, window, cx)],
            NodeKind::File(file) => vec![self.file_section(graph, node, &file.workdir, &file.path, file.mode, window, cx)],
            NodeKind::Review(review) => vec![self.review_section(graph, node, review.send_back_to.as_ref(), review.skip_if_empty, window, cx)],
            NodeKind::Action(action) => self.action_sections(graph, node, action, window, cx),
            NodeKind::Pipeline(nested) => vec![
                psec()
                    .gap(px(SPACE_2))
                    .child(row().gap(px(SPACE_2)).child(div().type_style(MONO).text_color(TEXT_SECONDARY).child(nested.pipeline.to_string())).child(Badge::new("Coming soon")))
                    .child(note("Nested pipelines are drawn on the canvas but do not run yet.", INNER))
                    .probe("insp-pipeline")
                    .into_any_element(),
            ],
            _ => Vec::new(),
        }
    }

    fn source_section(&mut self, node: &NodeDef, current: SourceKind, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let picks: Vec<Pick> = SOURCES
            .iter()
            .map(|(kind, label, sub, works)| {
                let pick = Pick::new(*label).sub(*sub).icon(IconName::Source).checked(*kind == current);
                if *works { pick } else { pick.soon() }
            })
            .collect();
        let (label, sub) = SOURCES.iter().find(|(k, ..)| *k == current).map(|(_, l, s, _)| (*l, *s)).unwrap_or(("Issue", ""));
        let on_pick: OnPick = Rc::new(|this, ix, _, cx| {
            let Some((kind, ..)) = SOURCES.get(ix) else { return };
            let kind = *kind;
            this.edit_node("Set source", move |node| { if let NodeKind::Source(s) = &mut node.kind { s.source = kind; } }, cx)
        });
        let picker = self.picker("insp-source", label, false, Some(IconName::Source), picks, on_pick, window, cx);
        let soon = matches!(current, SourceKind::Jira | SourceKind::Sentry);
        psec()
            .gap(px(SPACE_1_5))
            .child(field("Starts from", picker))
            .children(problem_line(self.problem_at(&node.id, "source", cx), INNER))
            .child(note(if soon { format!("{sub}. Coming soon: pick another source to run it.") } else { format!("{sub}.") }, INNER))
            .into_any_element()
    }

    #[expect(clippy::too_many_arguments, reason = "the three fields of a file node")]
    fn file_section(&mut self, graph: &GraphDef, node: &NodeDef, workdir: &str, path: &str, mode: FileMode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let picker = self.workdir_picker(
            "insp-workdir",
            graph,
            workdir,
            Rc::new(|node, id| {
                if let NodeKind::File(file) = &mut node.kind {
                    file.workdir = id;
                }
            }),
            window,
            cx,
        );
        let path_field = self.line(
            Spec::new("insp-path", path.to_string(), "Edit file path", on_kind(|kind, text| { if let NodeKind::File(file) = kind { file.path = text.to_string(); } }))
                .mono()
                .placeholder(if mode == FileMode::Read { "docs/**" } else { "out/{key}/spec.md" }),
            self.problem_at(&node.id, "path", cx),
            window,
            cx,
        );
        let modes = Segmented::new("insp-mode")
            .small()
            .full_width()
            .option("Read")
            .option("Write")
            .selected(usize::from(mode == FileMode::Write))
            .on_select(self.on_value(cx, |this, ix: usize, cx| {
                let mode = if ix == 1 { FileMode::Write } else { FileMode::Read };
                this.edit_node("Set file mode", move |node| { if let NodeKind::File(file) = &mut node.kind { file.mode = mode; } }, cx)
            }));
        let meaning = match mode {
            FileMode::Read => "Read: files from the workdir offered to the connected agents as context.",
            FileMode::Write => "Write: an artifact an agent writes and the review shows.",
        };
        psec()
            .gap(px(SPACE_3))
            .child(field("Workdir", picker))
            .children(problem_line(self.problem_at(&node.id, "workdir", cx), INNER))
            .child(field(if mode == FileMode::Read { "Path or glob" } else { "Path template" }, path_field))
            .child(field("Mode", div().child(modes).probe("insp-mode")))
            .child(note(meaning, INNER))
            .into_any_element()
    }

    fn review_section(&mut self, graph: &GraphDef, node: &NodeDef, send_back: Option<&hub::model::NodeId>, skip: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let upstream = ops::upstream_agents(graph, &node.id);
        let mut picks = vec![Pick::new("Nowhere").sub("Send back is not offered").checked(send_back.is_none())];
        picks.extend(upstream.iter().filter_map(|id| graph.node(id)).map(|n| Pick::new(n.name.clone()).hint(n.id.to_string()).icon(super::form::kind_icon(&n.kind)).checked(Some(&n.id) == send_back)));
        let value = match send_back {
            None => "Nowhere".to_string(),
            Some(id) => graph.node(id).map(|n| n.name.clone()).unwrap_or_else(|| format!("{id} · missing")),
        };
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let target = ix.checked_sub(1).and_then(|i| upstream.get(i).cloned());
            this.edit_node("Set send back", move |node| { if let NodeKind::Review(r) = &mut node.kind { r.send_back_to = target; } }, cx)
        });
        let picker = self.picker("insp-send-back", value, send_back.is_none(), Some(IconName::Replay), picks, on_pick, window, cx);
        let toggle = Toggle::new("insp-skip-if-empty", skip).on_change(self.on_value(cx, |this, on: bool, cx| {
            this.edit_node("Toggle skip if empty", move |node| { if let NodeKind::Review(r) = &mut node.kind { r.skip_if_empty = on; } }, cx)
        }));
        psec()
            .gap(px(SPACE_1_5))
            .child(field("Send back restarts", picker))
            .children(problem_line(self.problem_at(&node.id, "send-back", cx), INNER))
            .child(note("The agent a person's Send back runs again, with their note. It must be upstream of the review.", INNER))
            .child(row_field("Skip if empty", toggle.probe("insp-skip-if-empty")).mt(px(SPACE_3)))
            .child(note("Pass straight through when there is nothing to look at: no diff, no render.", INNER))
            .into_any_element()
    }

    fn action_sections(&mut self, graph: &GraphDef, node: &NodeDef, action: &ActionNode, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let kind = ops::action_kind(action);
        let picks: Vec<Pick> = (0..ops::ACTION_KINDS.len()).map(|k| Pick::new(ops::action_label(k)).icon(IconName::Zap).checked(k == kind)).collect();
        let fallback = graph.workdirs.first().map(|w| w.id.clone()).unwrap_or_default();
        let on_pick: OnPick = Rc::new(move |this, ix, _, cx| {
            let fallback = fallback.clone();
            this.edit_node("Change action", move |node| { if let NodeKind::Action(a) = &mut node.kind { *a = ops::with_action_kind(a, ix, &fallback); } }, cx)
        });
        let picker = self.picker("insp-action-kind", ops::action_label(kind), false, Some(IconName::Zap), picks, on_pick, window, cx);
        let mut main = psec()
            .gap(px(SPACE_1_5))
            .child(field("Action", picker))
            .child(note(ops::action_meaning(kind), INNER));
        if let Some(workdir) = ops::action_workdir(action) {
            let picker = self.workdir_picker(
                "insp-workdir",
                graph,
                workdir,
                Rc::new(|node, id| {
                    if let NodeKind::Action(action) = &mut node.kind
                        && let Some(w) = ops::action_workdir_mut(action)
                    {
                        *w = id;
                    }
                }),
                window,
                cx,
            );
            main = main.child(div().mt(px(SPACE_2)).child(field("Workdir", picker))).children(problem_line(self.problem_at(&node.id, "workdir", cx), INNER));
        }
        let mut sections = vec![main.into_any_element()];
        match action {
            ActionNode::Command { command, tests, parse, timeout_secs, .. } => sections.push(self.command_section(node, command, *tests, parse, *timeout_secs, window, cx)),
            ActionNode::Render { files, .. } => {
                let files = self.line(
                    Spec::new("insp-files", files.clone(), "Edit render files", on_action(|a, text| { if let ActionNode::Render { files, .. } = a { *files = text.to_string(); } }))
                        .mono()
                        .placeholder("**/*.html"),
                    self.problem_at(&node.id, "files", cx),
                    window,
                    cx,
                );
                sections.push(psec().gap(px(SPACE_1_5)).child(field("Files", files)).child(note("A glob of the HTML files to render when the run changed them.", INNER)).into_any_element());
            }
            ActionNode::PullRequest { draft, push, .. } => {
                let draft_toggle = Toggle::new("insp-draft", *draft).on_change(self.on_value(cx, |this, on: bool, cx| {
                    this.edit_node("Toggle draft", action_edit(move |a| { if let ActionNode::PullRequest { draft, .. } = a { *draft = on; } }), cx)
                }));
                let push_toggle = Toggle::new("insp-push", *push).on_change(self.on_value(cx, |this, on: bool, cx| {
                    this.edit_node("Toggle push", action_edit(move |a| { if let ActionNode::PullRequest { push, .. } = a { *push = on; } }), cx)
                }));
                sections.push(
                    psec()
                        .gap(px(SPACE_1_5))
                        .child(row_field("Open as a draft", draft_toggle.probe("insp-draft")))
                        .child(row_field("Push the branch", push_toggle.probe("insp-push")))
                        .child(note("Off: the commit stays local and no pull request is opened.", INNER))
                        .into_any_element(),
                );
            }
            ActionNode::UpdateIssue { status, comment } => sections.push(self.update_issue_section(node, *status, comment, window, cx)),
            ActionNode::Merge { .. } | ActionNode::Report => {}
        }
        sections
    }

    #[expect(clippy::too_many_arguments, reason = "the fields of a command action")]
    fn command_section(&mut self, node: &NodeDef, command: &str, tests: bool, parse: &str, timeout: u64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let command = self.area(
            Spec::new("insp-command", command.to_string(), "Edit command", on_action(|a, text| { if let ActionNode::Command { command, .. } = a { *command = text.to_string(); } }))
                .area(1, 6)
                .mono()
                .placeholder("cargo test --workspace"),
            self.problem_at(&node.id, "command", cx),
            window,
            cx,
        );
        let toggle = Toggle::new("insp-tests", tests).on_change(self.on_value(cx, |this, on: bool, cx| {
            this.edit_node("Toggle tests", action_edit(move |a| { if let ActionNode::Command { tests, .. } = a { *tests = on; } }), cx)
        }));
        let parse_field = self.line(
            Spec::new("insp-parse", parse.to_string(), "Edit test parsing", on_action(|a, text| { if let ActionNode::Command { parse, .. } = a { *parse = text.to_string(); } }))
                .mono()
                .placeholder("cargo-test, or a regex with passed and total"),
            None,
            window,
            cx,
        );
        let timeout_field = self.line(
            Spec::new("insp-timeout", if timeout == 0 { String::new() } else { timeout.to_string() }, "Set timeout", on_action(|a, text| {
                if let ActionNode::Command { timeout_secs, .. } = a {
                    *timeout_secs = u64::from(count_value(text).unwrap_or(0));
                }
            }))
            .parse(parse_count)
            .mono()
            .placeholder("None"),
            None,
            window,
            cx,
        );
        psec()
            .gap(px(SPACE_3))
            .child(field("Command", command))
            .child(
                col()
                    .gap(px(SPACE_1_5))
                    .child(row_field("Tests", toggle.probe("insp-tests")))
                    .child(note("Its result is the Tests row of the review.", INNER)),
            )
            .when(tests, |d| d.child(field("Read pass and fail counts", parse_field)).child(note("Empty shows pass or fail only.", INNER)))
            .child(row().gap(px(SPACE_3)).child(div().flex_1().type_style(LABEL).text_color(TEXT_MUTED).child("Timeout, seconds")).child(div().w(px(96.)).child(timeout_field)))
            .into_any_element()
    }

    fn update_issue_section(&mut self, node: &NodeDef, status: Option<IssueStatus>, comment: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut picks = vec![Pick::new("Leave as is").checked(status.is_none())];
        picks.extend(IssueStatus::ALL.iter().map(|s| Pick::new(s.label()).checked(Some(*s) == status)));
        let on_pick: OnPick = Rc::new(|this, ix, _, cx| {
            let status = ix.checked_sub(1).and_then(|i| IssueStatus::ALL.get(i).copied());
            this.edit_node("Set issue status", action_edit(move |a| { if let ActionNode::UpdateIssue { status: s, .. } = a { *s = status; } }), cx)
        });
        let value: SharedString = status.map(|s| s.label()).unwrap_or("Leave as is").into();
        let picker = self.picker("insp-status", value, status.is_none(), Some(IconName::Issues), picks, on_pick, window, cx);
        let comment = self.area(
            Spec::new("insp-comment", comment.to_string(), "Edit comment", on_action(|a, text| { if let ActionNode::UpdateIssue { comment, .. } = a { *comment = text.to_string(); } }))
                .area(2, 6)
                .placeholder("A comment to add, like the PR link"),
            None,
            window,
            cx,
        );
        psec()
            .gap(px(SPACE_3))
            .child(field("Move the issue to", picker))
            .children(problem_line(self.problem_at(&node.id, "status", cx), INNER))
            .child(field("Comment", comment))
            .into_any_element()
    }
}
