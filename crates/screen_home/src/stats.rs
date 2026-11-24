//! The stat strip: working now, waiting for you, runs, spend and shipped for the chosen period.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use hub::api::HomeStats;
use ui::prelude::*;

use crate::blocks::{Block, error_view, probed, skeleton};
use crate::view::HomeView;

/// The cells, left to right, with the name their probe carries (for the placeholder).
const LABELS: [(&str, &str); 5] = [
    ("working", "Working now"),
    ("waiting", "Waiting for you"),
    ("runs", "Runs"),
    ("spend", "Spend"),
    ("shipped", "Shipped"),
];

impl HomeView {
    /// The strip, a placeholder while it loads, or its error with a retry.
    pub(crate) fn render_stats(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.state(self.ops.read(cx).stats(self.period)) {
            Block::Ready(stats) => self.strip(&stats),
            Block::Loading => strip_frame(LABELS.iter().enumerate().map(|(ix, (name, label))| {
                let cell = col()
                    .child(div().type_style(LABEL).text_color(TEXT_MUTED).child(*label))
                    .child(div().mt(px(SPACE_1)).h(px(STAT_SM.line)).child(skeleton(72., 14.).mt(px(SPACE_1))));
                cell_frame(ix, name, cell)
            })),
            Block::Failed(message) => {
                let view = cx.entity().downgrade();
                strip_frame([error_view("the numbers", message, "stats", move |_, cx| {
                    view.update(cx, |view, cx| view.retry(cx)).ok();
                })])
            }
        }
    }

    fn strip(&self, stats: &HomeStats) -> AnyElement {
        let now = self.now();
        let texts = stat_texts(stats, now, base::time::utc_offset(now));
        strip_frame(texts.into_iter().enumerate().map(|(ix, text)| {
            let mut stat = Stat::new(text.label, text.value);
            if let Some(sub) = text.sub {
                stat = stat.suffix(sub);
            }
            if text.warning {
                stat = stat.warning();
            }
            let bar = text.share.map(|(share, over)| {
                let tone = if over { ProgressTone::Danger } else { ProgressTone::Accent };
                div().mt(px(SPACE_2)).child(Progress::new(share).tone(tone))
            });
            cell_frame(ix, text.name, col().child(stat).children(bar))
        }))
    }
}

/// What one cell of the strip says.
#[derive(Debug, PartialEq)]
pub(crate) struct StatText {
    /// The name its probe carries.
    pub name: &'static str,
    pub label: &'static str,
    pub value: String,
    /// The muted words after the value.
    pub sub: Option<String>,
    /// Something needs a person: the value is drawn in the warning colour.
    pub warning: bool,
    /// How full the bar under the value is, and whether it is past the limit.
    pub share: Option<(f32, bool)>,
}

/// The five cells of the strip, in the mockup's words and formats.
pub(crate) fn stat_texts(stats: &HomeStats, now: i64, offset: i32) -> Vec<StatText> {
    let count = |n: u32| base::fmt::count(u64::from(n));
    let cell = |name, label, value: String, sub: Option<String>| StatText {
        name,
        label,
        value,
        sub,
        warning: false,
        share: None,
    };
    let mut spend = cell("spend", "Spend", base::fmt::cost(stats.spend_usd), None);
    if let Some(limit) = stats.limit_usd {
        spend.sub = Some(format!("of {}", base::fmt::cost(limit)));
        spend.share = Some((base::fmt::percent(stats.spend_usd, limit) as f32 / 100., stats.spend_usd >= limit));
    }
    let mut waiting = cell(
        "waiting",
        "Waiting for you",
        count(stats.waiting),
        stats.oldest_waiting.map(|since| format!("oldest {}", base::time::ago(now, since, offset))),
    );
    waiting.warning = stats.waiting > 0;
    vec![
        cell("working", "Working now", count(stats.working_now), Some(format!("of {} slots", count(stats.slots)))),
        waiting,
        cell(
            "runs",
            "Runs",
            count(stats.runs),
            Some(if stats.runs == 0 { "no runs yet".to_string() } else { format!("{}% finished", stats.finished_percent) }),
        ),
        spend,
        cell("shipped", "Shipped", count(stats.shipped), Some("PRs and specs".to_string())),
    ]
}

/// `.strip`: one frame divided by hairlines.
fn strip_frame(cells: impl IntoIterator<Item = impl IntoElement>) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .w_full()
        .bg(SURFACE)
        .border_1()
        .border_color(LINE)
        .rounded(px(RADIUS_XL))
        .children(cells)
        .into_any_element()
}

/// One cell of the strip.
fn cell_frame(ix: usize, name: &str, content: impl IntoElement) -> AnyElement {
    probed(
        div()
            .flex_1()
            .min_w_0()
            .px(px(SPACE_4))
            .py(px(SPACE_3_5))
            .when(ix > 0, |d| d.border_l_1().border_color(LINE))
            .child(content),
        format!("home-stat-{name}"),
    )
    .into_any_element()
}
