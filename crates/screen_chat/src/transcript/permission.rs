//! A permission question: "Build wants to run a command", the call, the policy that asked, and
//! Allow once / Always allow `<rule>` / Reject, answered through `hub::commands`.

use std::sync::Arc;

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use hub::model::{PermissionDecision, PermissionRequest, ToolClass, ToolDisplay};
use hub::transcript::{PermissionBlock, PermissionState};
use ui::dev::Probe;
use ui::prelude::*;
use ui_rich::text_field;

use super::TranscriptView;

/// Size of the banner's policy mark (`.tile` without its fill).
const MARK: f32 = 24.;

/// What the agent wants to do, after "Build wants to": "run a command".
pub(crate) fn wants(request: &PermissionRequest) -> String {
    match (&request.class, &request.display) {
        (ToolClass::Execute, _) => "run a command".to_string(),
        (ToolClass::Write, ToolDisplay::Diff(file)) if file.created => "create a file".to_string(),
        (ToolClass::Write, _) => "edit a file".to_string(),
        (ToolClass::Read, _) => "read a file".to_string(),
        (ToolClass::External | ToolClass::Internal, _) => format!("use {}", request.tool),
    }
}

/// The call in machine words: `$ bun test src/charges`, a path, `grep pattern`.
pub(crate) fn call_line(display: &ToolDisplay) -> String {
    match display {
        ToolDisplay::Command { command, .. } => format!("$ {command}"),
        ToolDisplay::Diff(file) => file.path.clone(),
        ToolDisplay::Line { verb, target, .. } => format!("{verb} {target}"),
        ToolDisplay::Text { title, .. } => title.clone(),
    }
}

/// "Always allow bun test": the rule's pattern, or the tool when it matches every call.
pub(crate) fn always_label(request: &PermissionRequest) -> Option<String> {
    let rule = request.always_rule.as_ref()?;
    let what = if rule.pattern.is_empty() { &rule.tool } else { &rule.pattern };
    Some(format!("Always allow {what}"))
}

impl TranscriptView {
    /// The warning banner of a pending question, with its three answers.
    pub(crate) fn permission_banner(
        &mut self,
        request: &Arc<PermissionRequest>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mark = div()
            .flex_none()
            .size(px(MARK))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(IconName::Policy, 16., WARNING));
        let what = col()
            .flex_1()
            .min_w_0()
            .child(div().type_style(BODY_STRONG).text_color(TEXT).child(format!("{} wants to {}", request.agent, wants(request))))
            .child(div().type_style(MONO).text_color(TEXT).child(call_line(&request.display)));
        let policy = div().flex_none().type_style(MONO_SM).text_color(TEXT_MUTED).child(format!("policy: {}", request.policy));
        let top = row().gap(px(SPACE_3)).child(mark).child(what).child(policy);

        let allow = {
            let id = request.id.clone();
            Button::new("perm-allow", "Allow once")
                .primary()
                .small()
                .on_click(cx.listener(move |this, _, _, cx| this.answer(&id, PermissionDecision::AllowOnce, cx)))
                .probe("perm-allow")
        };
        let always = always_label(request).zip(request.always_rule.clone()).map(|(label, rule)| {
            let id = request.id.clone();
            Button::new("perm-always", label)
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.answer(&id, PermissionDecision::AllowAlways { rule: rule.clone() }, cx)
                }))
                .probe("perm-always")
        });
        let deny = {
            let id = request.id.clone();
            Button::new("perm-deny", "Reject")
                .danger()
                .small()
                .on_click(cx.listener(move |this, _, window, cx| this.start_reject_for(&id, window, cx)))
                .probe("perm-deny")
        };
        let answers = match self.reject_note(&request.id).cloned() {
            Some(input) => row()
                .gap(px(SPACE_2))
                .pl(px(MARK + SPACE_3))
                .child(div().flex_1().min_w_0().child(text_field("perm-reject-note", &input).small()).probe("perm-reject-note"))
                .child(
                    Button::new("perm-reject-send", "Reject")
                        .danger()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.send_reject(cx)))
                        .probe("perm-reject-send"),
                )
                .child(
                    Button::new("perm-reject-cancel", "Cancel")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_reject(cx)))
                        .probe("perm-reject-cancel"),
                )
                .child(KeyHints::new().hint(["↵"], "reject").hint(["Esc"], "cancel")),
            None => row()
                .gap(px(SPACE_2))
                .pl(px(MARK + SPACE_3))
                .child(allow)
                .children(always)
                .child(deny)
                .child(grow())
                .child(KeyHints::new().hint(["↵"], "allow").hint(["Esc"], "reject")),
        };
        Banner::new()
            .tone(BannerTone::Warning)
            .child(col().w_full().gap(px(SPACE_3)).child(top).child(answers))
            .into_any_element()
    }

    /// A question with no tool row to sit on: the banner while pending, a quiet line after.
    pub(crate) fn standalone_permission(
        &mut self,
        _index: usize,
        question: &PermissionBlock,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let request = question.request.clone();
        match question.state {
            PermissionState::Pending => self.permission_banner(&request, cx),
            PermissionState::Resolved => row()
                .gap(px(SPACE_2))
                .type_style(LABEL)
                .text_color(TEXT_MUTED)
                .child(icon(IconName::Policy, 12., TEXT_MUTED))
                .child(truncate(format!("{} asked to {} · {} · answered", request.agent, wants(&request), call_line(&request.display))))
                .into_any_element(),
        }
    }
}
