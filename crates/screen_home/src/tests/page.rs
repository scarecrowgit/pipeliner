//! The header, the stat strip, the period switch and the spend blocks.

use gpui::{AppContext as _, TestAppContext};
use hub::api::Period;

use super::{NOW, click, open, open_with, shown};
use crate::spend::{bars, chart_text};
use crate::stats::stat_texts;

fn stats(t: &super::Open, cx: &mut gpui::VisualTestContext, period: Period) -> hub::api::HomeStats {
    t.view.read_with(cx, |v, cx| (*v.ops.read(cx).stats(period).and_then(|s| s.value().cloned()).expect("stats loaded")).clone())
}

#[gpui::test]
async fn the_header_has_the_title_the_date_and_the_period_switch(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert_eq!(t.view.read_with(cx, |v, cx| v.title(cx)), "Home");
    // The kit's segmented control names its options by place: today, 7, 30 and 90 days.
    for ix in 0..4 {
        assert!(shown(cx, &format!("home-period-{ix}")), "option {ix}");
    }
    assert!(shown(cx, "home-new-chat"));
}

#[gpui::test]
async fn the_tiles_show_the_numbers_of_the_period(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    let today = stats(&t, cx, Period::Today);
    let cells: Vec<_> = stat_texts(&today, NOW, 0)
        .into_iter()
        .map(|c| (c.name, c.label, c.value, c.sub, c.warning))
        .collect();
    let owned = |n, l, v: &str, s: Option<&str>, w| (n, l, v.to_string(), s.map(str::to_string), w);
    assert_eq!(
        cells,
        [
            owned("working", "Working now", "6", Some("of 12 slots"), false),
            owned("waiting", "Waiting for you", "4", Some("oldest 3h"), true),
            owned("runs", "Runs", "61", Some("92% finished"), false),
            owned("spend", "Spend", "$3.81", Some("of $10.00"), false),
            owned("shipped", "Shipped", "7", Some("PRs and specs"), false),
        ]
    );
    let spend = stat_texts(&today, NOW, 0).remove(3);
    assert_eq!(spend.share, Some((0.38, false)));
    for name in ["working", "waiting", "runs", "spend", "shipped"] {
        assert!(shown(cx, &format!("home-stat-{name}")), "{name}");
    }
}

#[gpui::test]
async fn switching_the_period_asks_hub_and_changes_the_blocks(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert!(t.view.read_with(cx, |v, cx| v.ops.read(cx).stats(Period::Days90).is_none()), "not asked for yet");
    click(cx, "home-period-3");
    assert_eq!(t.view.read_with(cx, |v, _| v.period()), Period::Days90);
    let ninety = stats(&t, cx, Period::Days90);
    let cells = stat_texts(&ninety, NOW, 0);
    assert_eq!(cells[2].value, "4,212");
    assert_eq!(cells[2].sub.as_deref(), Some("89% finished"));
    assert_eq!(cells[3].value, "$612.40");
    assert_eq!(cells[3].sub.as_deref(), Some("of $900.00"));
    assert_eq!(cells[4].value, "418");
    // The chart and the agents are for the new period too.
    let buckets = t.view.read_with(cx, |v, cx| v.ops.read(cx).spend_buckets(Period::Days90).and_then(|b| b.value().cloned()));
    let buckets = buckets.expect("the weeks loaded");
    assert_eq!(buckets.len(), 13);
    let text = chart_text(Period::Days90, &buckets, Some(10.), 0);
    assert_eq!(text.title, "Spend per week");
    assert_eq!(text.sub, "last 13 weeks · limit $70.00 a week");
    assert!(text.note.starts_with("This week so far: $30.40 of $70.00. Highest week: W"), "{}", text.note);
    assert!(text.note.ends_with(", $61.20. No week went over the limit."), "{}", text.note);
    assert!(shown(cx, "home-spend-chart"));
    // The kit's value bars probe their rows for the dev driver only: count what they draw.
    let agents = t.view.read_with(cx, |v, cx| v.ops.read(cx).spend_by_runnable(Period::Days90).and_then(|s| s.value().map(|rows| rows.len())));
    assert_eq!(agents, Some(7));
}

#[gpui::test]
async fn the_chart_of_today_is_per_day_against_the_daily_limit(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    let buckets = t
        .view
        .read_with(cx, |v, cx| v.ops.read(cx).spend_buckets(Period::Today).and_then(|b| b.value().cloned()))
        .expect("the days loaded");
    let text = chart_text(Period::Today, &buckets, Some(10.), 0);
    assert_eq!(text.title, "Spend per day");
    assert_eq!(text.sub, "last 7 days · limit $10.00 a day");
    assert!(text.note.starts_with("Today so far: $3.81 of $10.00. Highest day this week: "), "{}", text.note);
    assert!(text.note.ends_with(", $8.80."), "{}", text.note);
    let drawn = bars(Period::Today, &buckets, 0);
    assert_eq!(drawn.len(), 7);
    assert_eq!(drawn[6].label, "Today");
    assert_eq!(drawn[2].tooltip.as_ref(), format!("{}: $8.80", drawn[2].label));
}

#[gpui::test]
async fn the_chosen_period_is_remembered_while_the_app_runs(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    click(cx, "home-period-2");
    assert_eq!(t.view.read_with(cx, |v, _| v.period()), Period::Days30);
    let second = cx.update(|window, cx| cx.new(|cx| crate::HomeView::new(window, cx)));
    assert_eq!(second.read_with(cx, |v, _| v.period()), Period::Days30);
}

#[gpui::test]
async fn the_sub_line_follows_the_period_not_the_bar_count(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    let one = t
        .view
        .read_with(cx, |v, cx| v.ops.read(cx).spend_buckets(Period::Today).and_then(|b| b.value().cloned()))
        .expect("loaded");
    let one = &one[one.len() - 1..];
    for period in [Period::Today, Period::Days7] {
        let text = chart_text(period, one, Some(10.), 0);
        assert_eq!(text.sub, "last 7 days · limit $10.00 a day");
    }
    assert_eq!(chart_text(Period::Days30, one, None, 0).sub, "last 30 days");
    assert_eq!(chart_text(Period::Days90, one, None, 0).sub, "last 13 weeks");
}

#[gpui::test]
async fn with_nothing_spent_the_chart_gives_way_to_the_empty_state(cx: &mut TestAppContext) {
    let (_t, cx) = open_with(cx, |world| {
        for buckets in world.ops.buckets.values_mut() {
            for bucket in buckets {
                bucket.cost_usd = 0.;
            }
        }
    });
    assert!(shown(cx, "home-spend-empty"));
    assert!(!shown(cx, "home-spend-bars"));
}

#[gpui::test]
async fn the_chart_is_drawn_when_something_was_spent(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    assert!(shown(cx, "home-spend-bars"));
    assert!(!shown(cx, "home-spend-empty"));
}

#[gpui::test]
async fn the_runs_tile_without_runs_has_no_percentage(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    let mut today = stats(&t, cx, Period::Today);
    today.runs = 0;
    today.finished_percent = 0;
    let runs = stat_texts(&today, NOW, 0).remove(2);
    assert_eq!(runs.value, "0");
    assert_eq!(runs.sub.as_deref(), Some("no runs yet"));
}

#[gpui::test]
async fn a_narrow_page_puts_the_right_column_under_the_left(cx: &mut TestAppContext) {
    let (t, cx) = open(cx);
    assert!(!t.view.read_with(cx, |v, _| v.stacked()), "1440 px: side by side");

    // The app's narrowest window with the sidebar open leaves the page about 660 px.
    cx.simulate_resize(gpui::size(gpui::px(660.), gpui::px(1600.)));
    cx.run_until_parked();
    assert!(t.view.read_with(cx, |v, _| v.stacked()));
    assert!(shown(cx, "home-spend-chart"));
    let working = cx.debug_bounds("home-working-5").expect("the last working row");
    let chart = cx.debug_bounds("home-spend-chart").expect("the chart");
    assert!(chart.top() > working.bottom(), "the chart comes after the table");
    assert!(chart.right() <= gpui::px(660.), "the chart stays in the window: {:?}", chart.right());
    assert!(working.right() <= gpui::px(660.), "so does the table: {:?}", working.right());
    let need = cx.debug_bounds("home-need-0").expect("a need");
    // A need row is inside its card's 1 px border; the chart is a card.
    assert_eq!(need.size.width + gpui::px(2.), chart.size.width, "both columns take the page's width");
}

#[gpui::test]
async fn scrolling_to_the_end_stops_at_the_last_card(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    // A window shorter than the page: the page scrolls.
    cx.simulate_resize(gpui::size(gpui::px(1440.), gpui::px(700.)));
    cx.run_until_parked();
    let at = cx.debug_bounds("home-need-0").expect("a need").center();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-5000.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let last = cx.debug_bounds("home-runtime-2").expect("the last runtime");
    let working = cx.debug_bounds("home-open-agents").expect("the agents link");
    let bottom = last.bottom().max(working.bottom());
    // The stage's header is 56 px; the page ends with its 24 px padding at the window's foot.
    assert!(bottom >= gpui::px(700. - 24. - 40.), "the page scrolled past its end: the last card ends at {bottom:?}");
    assert!(bottom <= gpui::px(700.), "and the last card is in view: {bottom:?}");
}

#[gpui::test]
async fn a_stacked_page_also_stops_at_its_last_card(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(700.)));
    cx.run_until_parked();
    let at = cx.debug_bounds("home-need-0").expect("a need").center();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-8000.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let last = cx.debug_bounds("home-runtime-2").expect("the last runtime");
    assert!(last.bottom() >= gpui::px(700. - 24. - 40.), "the page scrolled past its end: {:?}", last.bottom());
    assert!(last.bottom() <= gpui::px(700.), "the last card is in view: {:?}", last.bottom());
}

#[gpui::test]
async fn the_chart_card_keeps_its_height_when_the_window_widens(cx: &mut TestAppContext) {
    let (_t, cx) = open(cx);
    let height = cx.debug_bounds("home-spend-chart").expect("the chart").size.height;
    for width in [900., 1920., 1000., 1440.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1080.)));
        cx.run_until_parked();
        let now = cx.debug_bounds("home-spend-chart").expect("the chart").size.height;
        assert!((now - height).abs() < gpui::px(40.), "at {width} px the chart card is {now:?} tall, not {height:?}");
    }
}
