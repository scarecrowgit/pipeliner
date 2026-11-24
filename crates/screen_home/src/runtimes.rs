//! "Runtimes": where agents run, how many slots are taken, and what waits for a machine.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use hub::api::{Runtime, RuntimeKind};
use ui::prelude::*;

use crate::blocks::{Block, empty_view, error_view, probed, skeleton};
use crate::view::HomeView;

/// The icon in front of a runtime's name.
fn mark(kind: RuntimeKind) -> IconName {
    match kind {
        RuntimeKind::Local => IconName::Server,
        RuntimeKind::Cloud => IconName::Cloud,
        RuntimeKind::Remote => IconName::Laptop,
    }
}

/// "3 of 4 slots", "5 of 8 sandboxes".
fn slots(runtime: &Runtime) -> String {
    let unit = match runtime.kind {
        RuntimeKind::Cloud => "sandboxes",
        RuntimeKind::Local | RuntimeKind::Remote => "slots",
    };
    format!("{} of {} {unit}", runtime.slots_busy, runtime.slots_total)
}

impl HomeView {
    pub(crate) fn render_runtimes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.state(Some(self.ops.read(cx).runtimes()));
        let mut header = CardHeader::new("Runtimes");
        let body = match state {
            Block::Ready(runtimes) if runtimes.is_empty() => empty_view("home-runtimes-empty", "No runtimes", "This machine is not set up to run agents."),
            Block::Ready(runtimes) => {
                let online = runtimes.iter().filter(|runtime| runtime.online).count() as u64;
                header = header.meta(format!(
                    "{} of {} online",
                    base::fmt::count(online),
                    base::fmt::count(runtimes.len() as u64)
                ));
                col()
                    .children(
                        runtimes.iter().enumerate().map(|(ix, runtime)| runtime_row(ix, runtime, ix + 1 == runtimes.len())),
                    )
                    .into_any_element()
            }
            Block::Loading => col()
                .children((0..2).map(|_| {
                    col()
                        .flex_none()
                        .gap(px(SPACE_2))
                        .px(px(SPACE_4))
                        .py(px(SPACE_3))
                        .border_b_1()
                        .border_color(LINE_SUBTLE)
                        .child(skeleton(120., 12.))
                        .child(skeleton(200., 4.))
                }))
                .into_any_element(),
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                error_view("the runtimes", message, "runtimes", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                })
            }
        };
        Card::new().flush().header(header).child(body).into_any_element()
    }
}

/// One runtime: name, state, slots with a bar, and the note under it.
fn runtime_row(ix: usize, runtime: &Runtime, last: bool) -> AnyElement {
    let status = if runtime.online { Status::Live } else { Status::Idle };
    let head = row()
        .gap(px(SPACE_1_5))
        .child(icon(mark(runtime.kind), 14., TEXT_SECONDARY))
        .child(div().type_style(BODY_STRONG).child(runtime.name.clone()))
        .child(StatusDot::new(status).small())
        .child(grow())
        .child(if runtime.online {
            div().type_style(MONO).tabular().child(slots(runtime))
        } else {
            div().type_style(LABEL).text_color(TEXT_MUTED).child("offline")
        });
    let share = base::fmt::percent(f64::from(runtime.slots_busy), f64::from(runtime.slots_total)) as f32 / 100.;
    let note_color = if runtime.online { TEXT_MUTED } else { WARNING };
    let el = col()
        .flex_none()
        .px(px(SPACE_4))
        .py(px(SPACE_3))
        .when(!last, |d| d.border_b_1().border_color(LINE_SUBTLE))
        .child(head)
        .when(runtime.online, |d| d.child(div().mt(px(SPACE_2)).child(Progress::new(share))))
        .when(!runtime.note.is_empty(), |d| {
            d.child(
                div()
                    .mt(px(SPACE_1_5))
                    .w_full()
                    .type_style(LABEL)
                    .text_color(note_color)
                    .child(truncate_block(runtime.note.clone(), LABEL.line)),
            )
        });
    probed(el, format!("home-runtime-{ix}")).into_any_element()
}
