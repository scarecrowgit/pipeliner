//! Chart layout (pure) and the charts in a test window.

use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Render, TestAppContext, VisualTestContext,
    Window, div, prelude::*, px,
};

use super::*;

fn bars(spec: &[(&str, f64)]) -> Vec<Bar> {
    spec.iter().map(|(l, v)| Bar::new(l.to_string(), *v)).collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

// ---------------------------------------------------------------------------- scales

#[test]
fn nice_ceiling_rounds_up_to_one_two_two_and_a_half_five_ten() {
    let cases = [
        (0.3, 0.5),
        (1.0, 1.0),
        (1.1, 2.0),
        (2.2, 2.5),
        (3.81, 5.0),
        (5.0, 5.0),
        (5.1, 10.0),
        (8.8, 10.0),
        (10.0, 10.0),
        (10.5, 20.0),
        (61.2, 100.0),
        (70.0, 100.0),
        (612.4, 1000.0),
        (0.0004, 0.0005),
    ];
    for (value, want) in cases {
        let got = nice_ceiling(value);
        assert!((got - want).abs() <= want * 1e-9, "nice_ceiling({value}) = {got}, want {want}");
    }
}

#[test]
fn nice_ceiling_is_at_least_the_value_and_at_most_twice_it() {
    let mut v = 0.001;
    while v < 1e9 {
        let c = nice_ceiling(v);
        assert!(c >= v * (1.0 - 1e-9), "{c} below {v}");
        assert!(c <= v * 2.0 * (1.0 + 1e-9), "{c} more than twice {v}");
        v *= 1.37;
    }
}

#[test]
fn nice_ceiling_of_nothing_is_zero() {
    for v in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(nice_ceiling(v), 0.0, "{v}");
    }
}

#[test]
fn ticks_are_evenly_spaced_from_zero_to_max() {
    assert_eq!(ticks(10.0, 2), vec![0.0, 5.0, 10.0]);
    assert_eq!(ticks(8.0, 4), vec![0.0, 2.0, 4.0, 6.0, 8.0]);
    assert_eq!(ticks(10.0, 0), vec![0.0, 10.0], "at least one interval");
    assert_eq!(ticks(f64::NAN, 2), vec![0.0; 3]);
    assert_eq!(ticks(-4.0, 2), vec![0.0; 3]);
    assert_eq!(axis_ticks(70.0), [70.0, 35.0, 0.0]);
}

#[test]
fn fraction_is_clamped_and_never_nan() {
    assert!(close(fraction(5.0, 10.0), 0.5));
    assert_eq!(fraction(20.0, 10.0), 1.0);
    for (v, m) in [(-1.0, 10.0), (f64::NAN, 10.0), (1.0, 0.0), (1.0, f64::NAN), (f64::INFINITY, 1.0)] {
        assert_eq!(fraction(v, m), 0.0, "{v} of {m}");
    }
}

// ---------------------------------------------------------------------------- bar chart

#[test]
fn the_today_chart_of_the_mockup() {
    let data = bars(&[
        ("Mon", 6.20),
        ("Tue", 7.40),
        ("Wed", 8.80),
        ("Thu", 5.90),
        ("Fri", 7.10),
        ("Sat", 4.30),
        ("Today", 3.81),
    ]);
    let l = layout_bars(&data, Some(10.0), Some(10.0), Some(6), None);
    assert_eq!(l.max, 10.0);
    assert!(close(l.limit.unwrap_or(-1.0), 1.0));
    let heights: Vec<f32> = l.bars.iter().map(|b| b.height).collect();
    for (h, want) in heights.iter().zip([0.62, 0.74, 0.88, 0.59, 0.71, 0.43, 0.381]) {
        assert!(close(*h, want), "{h} vs {want}");
    }
    let values: Vec<bool> = l.bars.iter().map(|b| b.show_value).collect();
    assert_eq!(values, [false, false, true, false, false, false, true], "highest and now only");
    assert!(l.bars.iter().all(|b| b.show_label && b.over == 0.0));
    assert!(l.bars[6].now && l.bars.iter().filter(|b| b.now).count() == 1);
}

#[test]
fn the_ninety_day_chart_labels_only_some_bars() {
    let data = bars(&[
        ("W35", 38.2),
        ("", 41.5),
        ("", 44.0),
        ("W38", 52.3),
        ("", 47.1),
        ("", 39.8),
        ("W41", 55.6),
        ("", 61.2),
        ("", 49.4),
        ("W44", 43.7),
        ("", 50.9),
        ("", 58.3),
        ("Now", 30.4),
    ]);
    let l = layout_bars(&data, Some(70.0), None, Some(12), None);
    assert_eq!(l.max, 70.0, "the limit tops the plot when no bar passes it");
    let labelled: Vec<usize> =
        l.bars.iter().enumerate().filter(|(_, b)| b.show_label).map(|(i, _)| i).collect();
    assert_eq!(labelled, [0, 3, 6, 9, 12]);
    let valued: Vec<usize> =
        l.bars.iter().enumerate().filter(|(_, b)| b.show_value).map(|(i, _)| i).collect();
    assert_eq!(valued, [7, 12], "W42 is the highest week, the last is now");
}

#[test]
fn a_bar_over_the_limit_has_a_part_above_the_line() {
    let l = layout_bars(&bars(&[("a", 12.0), ("b", 5.0)]), Some(10.0), None, None, None);
    assert_eq!(l.max, 20.0, "a nice ceiling above the highest bar");
    assert!(close(l.limit.unwrap_or(-1.0), 0.5));
    assert!(close(l.bars[0].height, 0.6) && close(l.bars[0].over, 0.1));
    assert!(close(l.bars[1].height, 0.25) && l.bars[1].over == 0.0);
}

#[test]
fn a_bar_exactly_on_the_limit_is_not_over() {
    let l = layout_bars(&bars(&[("a", 10.0)]), Some(10.0), None, None, None);
    assert_eq!(l.max, 10.0);
    assert_eq!(l.bars[0].over, 0.0);
}

#[test]
fn default_max_is_a_nice_ceiling_without_a_limit() {
    let l = layout_bars(&bars(&[("a", 3.81), ("b", 2.0)]), None, None, None, None);
    assert_eq!(l.max, 5.0);
    assert_eq!(l.limit, None);
}

#[test]
fn explicit_max_wins_and_bars_past_it_are_clamped() {
    let l = layout_bars(&bars(&[("a", 30.0)]), None, Some(10.0), None, None);
    assert_eq!(l.max, 10.0);
    assert_eq!(l.bars[0].height, 1.0);
}

#[test]
fn all_zero_data_draws_flat_with_no_value_labels() {
    let l = layout_bars(&bars(&[("a", 0.0), ("b", 0.0), ("c", 0.0)]), None, None, Some(2), None);
    assert_eq!(l.max, 1.0, "never a zero scale");
    assert!(l.bars.iter().all(|b| b.height == 0.0 && b.over == 0.0 && !b.show_value));
}

#[test]
fn a_single_bar_is_both_highest_and_now() {
    let l = layout_bars(&bars(&[("Today", 3.0)]), None, None, Some(0), None);
    assert_eq!(l.bars.len(), 1);
    assert!(l.bars[0].show_value && l.bars[0].now && l.bars[0].show_label);
    assert!(close(l.bars[0].height, 0.6));
}

#[test]
fn no_bars_is_an_empty_layout() {
    let l = layout_bars(&[], Some(10.0), None, Some(3), Some(4));
    assert!(l.bars.is_empty());
    assert_eq!(l.max, 10.0);
}

#[test]
fn negative_and_non_finite_values_draw_as_zero() {
    let data = bars(&[("a", -5.0), ("b", f64::NAN), ("c", f64::INFINITY), ("d", 2.0)]);
    let l = layout_bars(&data, Some(f64::NAN), None, None, None);
    assert_eq!(l.limit, None);
    assert!(l.bars[..3].iter().all(|b| b.height == 0.0 && !b.show_value));
    assert!(l.bars[3].height > 0.0 && l.bars[3].show_value);
}

#[test]
fn a_limit_of_zero_or_less_is_no_limit() {
    for limit in [0.0, -3.0] {
        assert_eq!(layout_bars(&bars(&[("a", 1.0)]), Some(limit), None, None, None).limit, None);
    }
}

#[test]
fn ties_for_the_highest_label_only_the_first() {
    let l = layout_bars(&bars(&[("a", 4.0), ("b", 4.0), ("c", 1.0)]), None, None, None, None);
    let valued: Vec<bool> = l.bars.iter().map(|b| b.show_value).collect();
    assert_eq!(valued, [true, false, false]);
}

#[test]
fn now_out_of_range_marks_nothing() {
    let l = layout_bars(&bars(&[("a", 1.0)]), None, None, Some(9), None);
    assert!(l.bars.iter().all(|b| !b.now));
}

#[test]
fn thinning_spreads_labels_and_keeps_the_ends() {
    let all = vec![true; 13];
    let kept = |max| -> Vec<usize> {
        thin_labels(&all, max).iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect()
    };
    assert_eq!(kept(5), [0, 3, 6, 9, 12]);
    assert_eq!(kept(2), [0, 12]);
    assert_eq!(kept(1), [12], "the last one, which is `now`");
    assert_eq!(kept(0), Vec::<usize>::new());
    assert_eq!(kept(13).len(), 13);
    assert_eq!(kept(99).len(), 13);
}

#[test]
fn thinning_never_shows_an_unlabelled_bar() {
    let labelled = [true, false, true, false, true, false, true];
    let kept = thin_labels(&labelled, 2);
    assert_eq!(kept, [true, false, false, false, false, false, true]);
    assert!(thin_labels(&[false, false], 3).iter().all(|k| !k));
}

#[test]
fn layout_thins_labels_on_request() {
    let data: Vec<Bar> = (0..10).map(|i| Bar::new(format!("d{i}"), i as f64)).collect();
    let l = layout_bars(&data, None, None, None, Some(3));
    assert_eq!(l.bars.iter().filter(|b| b.show_label).count(), 3);
    assert!(l.bars[0].show_label && l.bars[9].show_label);
}

// ---------------------------------------------------------------------------- value bars

#[test]
fn value_bars_are_relative_to_the_largest() {
    let f = value_fractions(&[1.12, 0.94, 0.56, 0.0]);
    assert!(close(f[0], 1.0) && close(f[1], 0.94 / 1.12) && close(f[2], 0.5) && f[3] == 0.0);
}

#[test]
fn value_bars_of_nothing_positive_are_empty() {
    assert_eq!(value_fractions(&[]), Vec::<f32>::new());
    assert_eq!(value_fractions(&[0.0, 0.0]), [0.0, 0.0]);
    assert_eq!(value_fractions(&[-3.0, f64::NAN]), [0.0, 0.0]);
    assert_eq!(value_fractions(&[f64::NAN, 2.0, f64::INFINITY]), [0.0, 1.0, 0.0]);
}

#[test]
fn a_single_value_fills_its_track() {
    assert_eq!(value_fractions(&[7.0]), [1.0]);
}

// ---------------------------------------------------------------------------- timeline

#[test]
fn fractions_place_a_bar_on_the_run() {
    let (s, e) = fractions(9_450, 85_050, 105_000);
    assert!(close(s, 0.09) && close(e, 0.81));
    assert_eq!(fractions(10, 5, 100), fractions(5, 10, 100), "swapped ends are ordered");
    assert_eq!(fractions(0, 500, 100), (0.0, 1.0), "past the end is clamped");
    assert_eq!(fractions(1, 2, 0), (0.0, 0.0), "a run of no time");
}

#[test]
fn ticks_of_the_mockup_run() {
    let t = time_ticks(105_000, 30_000);
    let ms: Vec<u64> = t.iter().map(|(_, ms)| *ms).collect();
    assert_eq!(ms, [0, 30_000, 60_000, 90_000, 105_000]);
    let at: Vec<f32> = t.iter().map(|(f, _)| *f).collect();
    for (a, want) in at.iter().zip([0.0, 0.2857, 0.5714, 0.8571, 1.0]) {
        assert!((a - want).abs() < 1e-3, "{a} vs {want}");
    }
}

#[test]
fn a_tick_close_to_the_end_makes_way_for_it() {
    let ms: Vec<u64> = time_ticks(121_000, 60_000).iter().map(|(_, ms)| *ms).collect();
    assert_eq!(ms, [0, 60_000, 121_000]);
    let ms: Vec<u64> = time_ticks(120_000, 60_000).iter().map(|(_, ms)| *ms).collect();
    assert_eq!(ms, [0, 60_000, 120_000]);
}

#[test]
fn ticks_degenerate_inputs() {
    assert_eq!(time_ticks(0, 1000), vec![(0.0, 0)]);
    assert_eq!(time_ticks(5000, 0), vec![(0.0, 0), (1.0, 5000)]);
    assert_eq!(time_ticks(5000, 9000), vec![(0.0, 0), (1.0, 5000)]);
    assert!(time_ticks(u64::MAX, 1).len() <= 64, "a tiny step cannot flood the axis");
}

#[test]
fn even_ticks_span_the_axis() {
    let t = Tick::evenly(["0:00", "0:30", "1:00"]);
    assert_eq!(t.iter().map(|t| t.at).collect::<Vec<_>>(), [0.0, 0.5, 1.0]);
    assert_eq!(Tick::evenly(["only"]).len(), 1);
    assert!(Tick::evenly(Vec::<&str>::new()).is_empty());
}

#[test]
fn timeline_rows_clamp_and_order_their_span() {
    let r = TimelineRow::new("a", "", 0.9, 0.2, BarTone::Agent);
    assert_eq!(r.span(), (0.2, 0.9));
    let r = TimelineRow::new("a", "", f32::NAN, 7.0, BarTone::Agent);
    assert_eq!(r.span(), (0.0, 1.0));
}

// ---------------------------------------------------------------------------- in a window

struct Probe(Box<dyn Fn() -> AnyElement>);

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::root().child(div().w(px(400.)).child((self.0)()))
    }
}

fn show(
    cx: &mut TestAppContext,
    element: impl Fn() -> AnyElement + 'static,
) -> &mut VisualTestContext {
    ui::motion::set_reduced(true);
    let (_, cx) = cx.add_window_view(move |_, _| Probe(Box::new(element)));
    cx.run_until_parked();
    cx
}

fn height_of(cx: &mut VisualTestContext, selector: &'static str) -> f32 {
    let b = cx.debug_bounds(selector).expect("the element was laid out");
    f32::from(b.size.height)
}

fn mockup_days() -> Vec<Bar> {
    bars(&[
        ("Mon", 6.20),
        ("Tue", 7.40),
        ("Wed", 8.80),
        ("Thu", 5.90),
        ("Fri", 7.10),
        ("Sat", 4.30),
        ("Today", 3.81),
    ])
    .into_iter()
    .map(|b| {
        let tip = format!("{}: ${:.2}", b.label, b.value);
        b.tooltip(tip)
    })
    .collect()
}

#[gpui::test]
fn the_bar_chart_is_a_plot_and_a_row_of_labels(cx: &mut TestAppContext) {
    let cx = show(cx, || {
        div()
            .debug_selector(|| "chart".into())
            .child(
                BarChart::new("spend", mockup_days())
                    .limit(10.0)
                    .max(10.0)
                    .axis_labels("$10", "$5", "$0"),
            )
            .into_any_element()
    });
    assert_eq!(height_of(cx, "chart"), 120.0 + 6.0 + 16.0);
}

#[gpui::test]
fn the_bar_chart_without_labels_has_no_label_row(cx: &mut TestAppContext) {
    let cx = show(cx, || {
        div()
            .debug_selector(|| "chart".into())
            .child(BarChart::new("spend", bars(&[("", 1.0), ("", 2.0)])))
            .into_any_element()
    });
    assert_eq!(height_of(cx, "chart"), 120.0);
}

#[gpui::test]
fn the_bar_chart_survives_bad_data(cx: &mut TestAppContext) {
    let nasty = vec![
        Vec::new(),
        bars(&[("a", 0.0)]),
        bars(&[("a", -4.0), ("b", f64::NAN), ("c", f64::INFINITY)]),
        bars(&[("only", 3.0)]),
        bars(&[("a", 1e300), ("b", 1e-300)]),
    ];
    for (i, data) in nasty.into_iter().enumerate() {
        let id = ("nasty", i);
        let cx = show(cx, move || {
            BarChart::new(id, data.clone()).limit(f64::NAN).max(f64::NAN).now(7).into_any_element()
        });
        cx.run_until_parked();
    }
    let _ = show(cx, || BarChart::new("zero-limit", bars(&[("a", 1.0)])).limit(0.0).without_now().max_labels(0).into_any_element());
}

#[gpui::test]
fn value_bars_are_28_px_rows_and_24_compact(cx: &mut TestAppContext) {
    let rows = || {
        [("Payments lead", 1.12), ("PAY triage", 0.94), ("Build", 0.62), ("Others · 6", 0.39)]
            .map(|(n, v)| ValueRow::new(n, v))
    };
    let cx = show(cx, move || {
        div()
            .child(div().debug_selector(|| "full".into()).child(ValueBars::new(rows())))
            .child(div().debug_selector(|| "compact".into()).child(ValueBars::new(rows()).compact()))
            .into_any_element()
    });
    assert_eq!(height_of(cx, "full"), 4.0 * 28.0);
    assert_eq!(height_of(cx, "compact"), 4.0 * 24.0);
}

#[gpui::test]
fn value_bars_survive_bad_data(cx: &mut TestAppContext) {
    let _ = show(cx, || {
        ValueBars::new([
            ValueRow::new("", f64::NAN),
            ValueRow::new("a very long name that cannot possibly fit in 112 pixels", -1.0).spawn(),
            ValueRow::new("zero", 0.0).amount("—"),
        ])
        .into_any_element()
    });
    let _ = show(cx, || ValueBars::new(Vec::new()).compact().into_any_element());
}

fn mockup_rows() -> Vec<TimelineRow> {
    let total = 105_000;
    let row = |l: &'static str, m: &'static str, start: u64, width: u64, tone: BarTone| {
        TimelineRow::from_ms(l, m, total * start / 100, total * (start + width) / 100, total, tone)
    };
    vec![
        row("Jira puller", "3 issues", 0, 6, BarTone::Agent),
        row("Router", "3 routed", 5, 4, BarTone::Agent),
        row("Fixer ×3", "PAY-1423", 9, 76, BarTone::Agent),
        row("Test writer ×2", "by Fixer", 30, 32, BarTone::Spawn),
        row("Reproducer", "PAY-1427", 9, 49, BarTone::Agent),
        row("Log digger", "by Reproducer", 24, 23, BarTone::Spawn),
        row("Analyst ×2", "PAY-1431", 9, 62, BarTone::Agent),
        row("Review", "6 variants", 86, 14, BarTone::Waiting),
    ]
}

fn mockup_ticks() -> Vec<Tick> {
    time_ticks(105_000, 30_000)
        .into_iter()
        .map(|(at, ms)| Tick::new(at, format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)))
        .collect()
}

#[gpui::test]
fn the_timeline_is_22_px_rows_and_a_20_px_axis(cx: &mut TestAppContext) {
    let cx = show(cx, || {
        div()
            .child(
                div()
                    .debug_selector(|| "above".into())
                    .child(Timeline::new(mockup_rows()).ticks(mockup_ticks())),
            )
            .child(
                div()
                    .debug_selector(|| "below".into())
                    .child(Timeline::new(mockup_rows()).ticks(mockup_ticks()).axis_below()),
            )
            .child(div().debug_selector(|| "bare".into()).child(Timeline::new(mockup_rows())))
            .into_any_element()
    });
    assert_eq!(height_of(cx, "above"), 20.0 + 4.0 + 8.0 * 22.0);
    assert_eq!(height_of(cx, "below"), 20.0 + 4.0 + 8.0 * 22.0);
    assert_eq!(height_of(cx, "bare"), 8.0 * 22.0);
}

#[gpui::test]
fn the_timeline_survives_bad_data(cx: &mut TestAppContext) {
    let _ = show(cx, || {
        Timeline::new([
            TimelineRow::new("", "", f32::NAN, f32::INFINITY, BarTone::Waiting),
            TimelineRow::from_ms("zero", "", 0, 0, 0, BarTone::Agent),
        ])
        .ticks([Tick::new(f32::NAN, "x")])
        .into_any_element()
    });
    let _ = show(cx, || Timeline::new(Vec::new()).ticks(Tick::evenly(["a", "b"])).into_any_element());
}
