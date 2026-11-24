//! The sidebar foot (`.sb-foot`): today's spend against the limit and the runtimes, and the
//! "Spend and runtimes" popover it opens.

use gpui::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window, div, prelude::*, px};
use hub::OpsModel;
use hub::api::Period;
use model::{Runtime, RuntimeKind, SpendToday};
use ui::dev::Probe;
use ui::motion::{self, Look};
use ui::prelude::*;

use super::Sidebar;

/// The anchor key of the foot (and of the rail's spend block).
pub(crate) const ANCHOR: &str = "sidebar-spend";

/// "$3.81" and "of $10.00" (or "no limit").
pub(crate) fn spend_texts(spend: &SpendToday) -> (SharedString, SharedString) {
    let of = match spend.limit_usd {
        Some(limit) => format!("of {}", base::fmt::cost(limit)),
        None => "no limit".to_string(),
    };
    (base::fmt::cost(spend.cost_usd).into(), of.into())
}

/// The spend bar, when there is a limit.
pub(crate) fn spend_bar(spend: &SpendToday) -> Option<Progress> {
    let limit = spend.limit_usd.filter(|limit| *limit > 0.)?;
    let fraction = (spend.cost_usd / limit).clamp(0., 1.) as f32;
    let tone = if spend.at_limit() { ProgressTone::Danger } else { ProgressTone::Accent };
    Some(Progress::new(fraction).tone(tone))
}

impl Sidebar {
    pub(super) fn render_foot(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ops = self.ops.read(cx);
        let spend = ops.spend_today().value().map(|spend| (**spend).clone()).unwrap_or_default();
        let runtimes: Vec<Runtime> = ops.runtimes().value().map(|r| r.to_vec()).unwrap_or_default();
        let (cost, of) = spend_texts(&spend);
        let open = ui::overlays(window, cx).is_open(ANCHOR, cx);
        let rest = Look::bg(if open { SURFACE_HOVER } else { TRANSPARENT });
        let hover = Look::bg(if open { SURFACE_HOVER } else { SURFACE_RAISED });
        let faded = motion::hover_fade_state(ANCHOR, rest, hover, window, cx);
        let ops = self.ops.clone();
        let el = div()
            .id(ANCHOR)
            .debug_selector(|| ANCHOR.into())
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(SPACE_2))
            .mt(px(SPACE_2))
            .mx(px(-SPACE_2))
            .pt(px(SPACE_3))
            .px(px(SPACE_4))
            .border_t_1()
            .border_color(LINE)
            .rounded_b(px(RADIUS_LG))
            .cursor_pointer()
            .on_click(move |_, window, cx| open_popover(ops.clone(), ANCHOR, Placement::above(), window, cx));
        faded
            .apply(el)
            .child(
                row()
                    .type_style(LABEL)
                    .gap(px(SPACE_1))
                    .child(div().text_color(TEXT_MUTED).child("Spend today"))
                    .child(grow())
                    .child(div().type_style(LABEL_STRONG).text_color(TEXT).tabular().child(cost))
                    .child(div().text_color(TEXT_MUTED).tabular().child(of)),
            )
            .children(spend_bar(&spend))
            .child(
                row()
                    .gap(px(SPACE_3))
                    .type_style(LABEL)
                    .text_color(TEXT_MUTED)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    // Runtimes that cannot be picked yet are listed in the popover, not here.
                    .children(runtimes.iter().filter(|runtime| runtime.available).map(|runtime| {
                        let (status, label) = if runtime.online {
                            (Status::Live, runtime.name.clone())
                        } else {
                            (Status::Idle, format!("{} off", runtime.name))
                        };
                        row().flex_none().gap(px(SPACE_1_5)).child(StatusDot::new(status).small()).child(label)
                    })),
            )
            .popover_anchor(ANCHOR)
            .probe(ANCHOR)
    }
}

/// Open the "Spend and runtimes" popover from the trigger recorded as `anchor`.
pub(crate) fn open_popover(
    ops: Entity<OpsModel>,
    anchor: &'static str,
    placement: Placement,
    window: &mut Window,
    cx: &mut App,
) {
    let anchor = Anchor::of(anchor, window, cx);
    ui::overlays(window, cx).open_popover(
        anchor,
        placement,
        move |_, cx| spend_popover(ops.read(cx)),
        window,
        cx,
    );
}

/// Spend today by agent, the limit rule, the runtimes and their slots.
fn spend_popover(ops: &OpsModel) -> AnyElement {
    let spend = ops.spend_today().value().map(|spend| (**spend).clone()).unwrap_or_default();
    let (cost, of) = spend_texts(&spend);
    let spenders = ops
        .spend_by_runnable(Period::Today)
        .and_then(|slot| slot.value())
        .map(|rows| rows.to_vec())
        .unwrap_or_default();
    let mut rows: Vec<(SharedString, f64, bool)> =
        spenders.iter().filter(|row| !row.spawned).take(3).map(|row| (row.name.clone().into(), row.cost_usd, false)).collect();
    let spawned: f64 = spenders.iter().filter(|row| row.spawned).map(|row| row.cost_usd).sum();
    if spawned > 0. {
        rows.push(("Created by agents".into(), spawned, true));
    }
    let top = rows.iter().map(|(_, cost, _)| *cost).fold(0., f64::max);
    let runtimes: Vec<Runtime> = ops.runtimes().value().map(|r| r.to_vec()).unwrap_or_default();
    // Slots of the runtimes work can go to now; the others are drawn as coming soon.
    let usable = runtimes.iter().filter(|runtime| runtime.available && runtime.online);
    let (slots, busy) = usable.fold((0, 0), |(s, b), r| (s + r.slots_total, b + r.slots_busy));

    let spend_head = col()
        .px(px(SPACE_2_5))
        .pt(px(SPACE_2_5))
        .pb(px(SPACE_2))
        .child(
            row()
                .gap(px(SPACE_1))
                .child(div().type_style(BODY_STRONG).child("Spend today"))
                .child(grow())
                .child(div().type_style(BODY_STRONG).tabular().child(cost))
                .child(div().text_color(TEXT_MUTED).tabular().child(of)),
        )
        .children(spend_bar(&spend).map(|bar| div().mt(px(SPACE_2)).child(bar)))
        .child(col().mt(px(SPACE_2_5)).children(rows.into_iter().map(|(name, cost, spawned)| {
            let fraction = if top > 0. { (cost / top) as f32 } else { 0. };
            row()
                .h(px(SPACE_6))
                .type_style(LABEL)
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_color(if spawned { SPAWN_TEXT } else { TEXT_SECONDARY })
                        .child(truncate_lh(name, LABEL.line)),
                )
                .child(
                    div().flex_1().mx(px(SPACE_2_5)).h(px(SPACE_1)).rounded_full().bg(LINE).overflow_hidden().child(
                        div()
                            .h_full()
                            .w(gpui::relative(fraction))
                            .rounded_full()
                            .bg(if spawned { SPAWN_BAR } else { TEXT_MUTED }),
                    ),
                )
                .child(div().w(px(44.)).flex_none().flex().justify_end().type_style(MONO).tabular().child(base::fmt::cost(cost)))
        })))
        .when(spend.limit_usd.is_some(), |d| {
            d.child(
                div()
                    .mt(px(SPACE_2))
                    .child(Note::new("Resets at 00:00. At the limit, running work finishes and new runs wait for you.")),
            )
        });

    let runtime_head = col()
        .px(px(SPACE_2_5))
        .pt(px(SPACE_2_5))
        .pb(px(SPACE_2))
        .child(
            row()
                .mb(px(SPACE_1))
                .child(div().type_style(BODY_STRONG).child("Runtimes"))
                .child(grow())
                .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(format!("{slots} slots · {busy} busy"))),
        )
        .children(runtimes.iter().map(runtime_row));

    Popover::new()
        .width(340.)
        .child(spend_head)
        .child(PopSeparator)
        .child(runtime_head)
        .child(PopSeparator)
        .child(
            MenuItem::new("spend-limits", "Limits and runtimes")
                .icon(IconName::Settings)
                .hint("Settings")
                .on_click(|_, window, cx| super::limits::open(window, cx))
                .probe_name("spend-limits"),
        )
        .into_any_element()
}

/// "4 slots · 2 busy · this office".
pub(crate) fn runtime_line(runtime: &Runtime) -> String {
    let slots = format!("{} · {} busy", base::fmt::plural(u64::from(runtime.slots_total), "slot", "slots"), runtime.slots_busy);
    if runtime.note.is_empty() { slots } else { format!("{slots} · {}", runtime.note) }
}

/// One runtime: its tile, name, slots, and online state. Runtimes that cannot be used yet
/// (cloud sandboxes, other machines) say "Coming soon".
pub(crate) fn runtime_row(runtime: &Runtime) -> impl IntoElement {
    let tile = match runtime.kind {
        RuntimeKind::Local => IconName::Server,
        RuntimeKind::Cloud => IconName::Cloud,
        RuntimeKind::Remote => IconName::Laptop,
    };
    let (status, word, color) =
        if runtime.online { (Status::Live, "online", TEXT_SECONDARY) } else { (Status::Idle, "offline", TEXT_MUTED) };
    let fg = if runtime.available { TEXT } else { TEXT_DISABLED };
    row()
        .min_h(px(44.))
        .py(px(SPACE_1))
        .gap(px(SPACE_2_5))
        .child(Tile::new(tile))
        .child(
            col()
                .flex_1()
                .child(div().type_style(BODY_STRONG).text_color(fg).child(runtime.name.clone()))
                .child(truncate_block(runtime_line(runtime), LABEL.line).type_style(LABEL).text_color(TEXT_MUTED)),
        )
        .when(!runtime.available, |d| d.child(Tag::new("Coming soon")))
        .when(runtime.available, |d| {
            d.child(
                row()
                    .flex_none()
                    .gap(px(SPACE_1_5))
                    .type_style(LABEL)
                    .child(StatusDot::new(status).small())
                    .child(div().text_color(color).child(word)),
            )
        })
}
