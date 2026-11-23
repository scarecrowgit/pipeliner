use super::*;
use std::sync::Arc;

const HOUR: i64 = 3_600_000;
const MINUTE: i64 = 60_000;
/// Sunday 2026-11-22 18:40:05 UTC.
const NOW: i64 = 1_795_372_805_000;

fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    from_civil(year, month, day, hour, minute, 0, 0)
}

#[test]
fn clocks() {
    let fixed = FixedClock::new(1_000);
    assert_eq!(fixed.now_ms(), 1_000);
    fixed.advance(250);
    assert_eq!(fixed.now_ms(), 1_250);
    fixed.advance(-50);
    assert_eq!(fixed.now_ms(), 1_200);
    fixed.set(-7);
    assert_eq!(fixed.now_ms(), -7);
    assert_eq!(FixedClock::default().now_ms(), 0);

    let before = now_ms();
    let system = SystemClock.now_ms();
    assert!(system >= before && system - before < 5_000);
    // After 2020, before 2200: the unit is milliseconds.
    assert!(before > 1_577_836_800_000 && before < 7_258_118_400_000);

    let shared: Arc<dyn Clock> = Arc::new(FixedClock::new(42));
    let seen = std::thread::spawn({
        let clock = shared.clone();
        move || clock.now_ms()
    });
    assert_eq!(seen.join().unwrap(), 42);
}

#[test]
fn civil_known_dates() {
    let epoch = civil(0, 0);
    assert_eq!((epoch.year, epoch.month, epoch.day), (1970, 1, 1));
    assert_eq!((epoch.hour, epoch.minute, epoch.second, epoch.weekday), (0, 0, 0, 3));

    let t = civil(NOW, 0);
    assert_eq!(
        t,
        LocalTime {
            year: 2026,
            month: 11,
            day: 22,
            hour: 18,
            minute: 40,
            second: 5,
            weekday: 6,
            utc_offset_secs: 0
        }
    );

    // Leap days, including the century rules.
    let leap = civil(951_782_400_000, 0);
    assert_eq!((leap.year, leap.month, leap.day, leap.weekday), (2000, 2, 29, 1));
    let not_leap = civil(-2_203_891_200_000, 0);
    assert_eq!((not_leap.year, not_leap.month, not_leap.day, not_leap.weekday), (1900, 3, 1, 3));
    let far = civil(13_574_563_200_000, 0);
    assert_eq!((far.year, far.month, far.day, far.weekday), (2400, 2, 29, 1));
}

#[test]
fn civil_before_the_epoch_rounds_down() {
    let t = civil(-1, 0);
    assert_eq!((t.year, t.month, t.day), (1969, 12, 31));
    assert_eq!((t.hour, t.minute, t.second, t.weekday), (23, 59, 59, 2));
    assert_eq!(civil(-1_000, 0).second, 59);
    assert_eq!(civil(-1_001, 0).second, 58);
}

#[test]
fn civil_applies_the_offset() {
    let east = civil(NOW, 19_800); // +05:30
    assert_eq!((east.day, east.hour, east.minute, east.weekday), (23, 0, 10, 0));
    assert_eq!(east.utc_offset_secs, 19_800);
    let west = civil(utc(2026, 1, 1, 2, 0), -5 * 3600);
    assert_eq!((west.year, west.month, west.day, west.hour), (2025, 12, 31, 21));
}

#[test]
fn from_civil_round_trips() {
    assert_eq!(utc(1970, 1, 1, 0, 0), 0);
    assert_eq!(from_civil(2026, 11, 22, 18, 40, 5, 0), NOW);
    assert_eq!(from_civil(2026, 11, 22, 21, 40, 5, 3 * 3600), NOW);
    // Day 32 of October is the 1st of November.
    assert_eq!(utc(2026, 10, 32, 0, 0), utc(2026, 11, 1, 0, 0));

    let mut ms = utc(1899, 12, 25, 7, 13);
    let mut weekday = civil(ms, 0).weekday;
    while ms < utc(2101, 1, 1, 0, 0) {
        for offset in [0, 3_600, -28_800, 45_900] {
            let t = civil(ms, offset);
            assert!((1..=12).contains(&t.month) && (1..=31).contains(&t.day));
            let back = from_civil(t.year, t.month, t.day, t.hour, t.minute, t.second, offset);
            assert_eq!(back, ms, "{t:?}");
        }
        assert_eq!(civil(ms, 0).weekday, weekday);
        ms += 13 * MS_PER_DAY;
        weekday = (weekday + 13) % 7;
    }
}

#[test]
fn day_and_week_starts() {
    assert_eq!(day_start(NOW, 0), 1_795_305_600_000);
    assert_eq!(day_start(1_795_305_600_000, 0), 1_795_305_600_000);
    assert_eq!(day_start(1_795_305_599_999, 0), 1_795_305_600_000 - MS_PER_DAY);
    // 21:40 at +03:00: midnight was 21:00 UTC the day before.
    assert_eq!(day_start(NOW, 3 * 3600), 1_795_294_800_000);
    // 13:40 at -05:00.
    assert_eq!(day_start(NOW, -5 * 3600), 1_795_323_600_000);
    // 00:40 on Monday at +06:00.
    assert_eq!(day_start(NOW, 6 * 3600), 1_795_370_400_000);
    assert_eq!(day_start(-1, 0), -MS_PER_DAY);

    assert_eq!(week_start(NOW, 0), utc(2026, 11, 16, 0, 0));
    assert_eq!(week_start(NOW, 6 * 3600), 1_795_370_400_000);
    assert_eq!(week_start(utc(2026, 11, 16, 0, 0), 0), utc(2026, 11, 16, 0, 0));
    assert_eq!(week_start(utc(2026, 11, 16, 0, 0) - 1, 0), utc(2026, 11, 9, 0, 0));
    for offset in [0, 3_600, -36_000, 50_400] {
        let start = week_start(NOW, offset);
        let t = civil(start, offset);
        assert_eq!((t.weekday, t.hour, t.minute, t.second), (0, 0, 0, 0));
        assert!(start <= NOW && NOW - start < 7 * MS_PER_DAY);
    }
}

#[test]
fn iso_weeks() {
    assert_eq!(iso_week(utc(2026, 11, 23, 12, 0), 0), 48);
    assert_eq!(iso_week(NOW, 0), 47);
    assert_eq!(iso_week(utc(2026, 1, 1, 0, 0), 0), 1);
    // 2026 has 53 weeks; 1 January 2027 still belongs to it.
    assert_eq!(iso_week(utc(2027, 1, 1, 0, 0), 0), 53);
    assert_eq!(iso_week(utc(2027, 1, 4, 0, 0), 0), 1);
    // The last days of December can be week 1 of the next year.
    assert_eq!(iso_week(utc(2024, 12, 30, 0, 0), 0), 1);
    assert_eq!(iso_week(utc(2020, 12, 31, 0, 0), 0), 53);
    assert_eq!(iso_week(utc(2021, 1, 3, 23, 59), 0), 53);
    assert_eq!(iso_week(utc(2021, 1, 4, 0, 0), 0), 1);
    assert_eq!(iso_week(utc(2025, 10, 12, 0, 0), 0), 41);
    // The offset decides which side of the week boundary a moment is on.
    assert_eq!(iso_week(NOW, 6 * 3600), 48);
}

#[test]
fn ago_tiers() {
    let ago = |back: i64| super::ago(NOW, NOW - back, 0);
    assert_eq!(ago(0), "now");
    assert_eq!(ago(59_999), "now");
    assert_eq!(ago(-5 * HOUR), "now");
    assert_eq!(ago(MINUTE), "1m");
    assert_eq!(ago(12 * MINUTE + 30_000), "12m");
    assert_eq!(ago(HOUR - 1), "59m");
    assert_eq!(ago(HOUR), "1h");
    assert_eq!(ago(3 * HOUR + 59 * MINUTE), "3h");
    assert_eq!(ago(24 * HOUR - 1), "23h");
    assert_eq!(ago(24 * HOUR), "Sat");
    assert_eq!(ago(2 * 24 * HOUR), "Fri");
    // Monday of the same week, at midnight: six calendar days back.
    assert_eq!(super::ago(NOW, utc(2026, 11, 16, 0, 0), 0), "Mon");
    // Last Sunday would read like today, so it gets a date.
    assert_eq!(super::ago(NOW, utc(2026, 11, 15, 23, 59), 0), "15 Nov");
    assert_eq!(super::ago(NOW, utc(2026, 10, 12, 9, 0), 0), "12 Oct");
    assert_eq!(super::ago(NOW, utc(2026, 1, 1, 0, 0), 0), "1 Jan");
    assert_eq!(super::ago(NOW, utc(2025, 10, 12, 9, 0), 0), "12 Oct 2025");
    assert_eq!(super::ago(utc(2026, 1, 3, 0, 0), utc(2025, 12, 20, 0, 0), 0), "20 Dec 2025");
}

#[test]
fn ago_uses_the_local_calendar() {
    let then = utc(2026, 11, 20, 23, 30);
    assert_eq!(ago(NOW, then, 0), "Fri");
    assert_eq!(ago(NOW, then, 3_600), "Sat");
    let new_year = utc(2025, 12, 31, 23, 0);
    assert_eq!(ago(NOW, new_year, 0), "31 Dec 2025");
    assert_eq!(ago(NOW, new_year, 7_200), "1 Jan");
}

#[test]
fn date_and_clock_labels() {
    assert_eq!(long_date(NOW, 0), "Sunday, 22 November");
    assert_eq!(long_date(NOW, 19_800), "Monday, 23 November");
    assert_eq!(long_date(0, 0), "Thursday, 1 January");
    assert_eq!(clock_time(NOW, 0), "18:40");
    assert_eq!(clock_time(NOW, 19_800), "00:10");
    assert_eq!(clock_time(utc(2026, 3, 4, 9, 5), 0), "09:05");
    assert_eq!(clock_time(-1, 0), "23:59");
}

#[test]
fn names() {
    assert_eq!(month_name(1), "January");
    assert_eq!(month_name(12), "December");
    assert_eq!(month_short(9), "Sep");
    assert_eq!(weekday_name(0), "Monday");
    assert_eq!(weekday_short(6), "Sun");
    // Out of range never panics.
    assert_eq!(month_name(0), "January");
    assert_eq!(month_short(99), "Dec");
    assert_eq!(weekday_short(7), "Mon");
}

#[test]
fn local_time_agrees_with_the_offset() {
    let offset = utc_offset(NOW);
    assert!((-43_200..=50_400).contains(&offset), "{offset}");
    assert_eq!(local(NOW), civil(NOW, offset));
    assert_eq!(local(NOW).utc_offset_secs, offset);
}

#[test]
fn extreme_inputs_do_not_panic() {
    for ms in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
        for offset in [i32::MIN, -50_400, 0, 50_400, i32::MAX] {
            let t = civil(ms, offset);
            assert!((1..=12).contains(&t.month) && (1..=31).contains(&t.day));
            assert!(t.hour < 24 && t.minute < 60 && t.second < 60 && t.weekday < 7);
            assert!(day_start(ms, offset) <= ms.saturating_add(1) || offset != 0);
            let _ = week_start(ms, offset);
            assert!((1..=53).contains(&iso_week(ms, offset)));
            let _ = long_date(ms, offset);
            let _ = clock_time(ms, offset);
            let _ = ago(ms, 0, offset);
            let _ = ago(0, ms, offset);
            let _ = ago(i64::MAX, ms, offset);
            let _ = ago(i64::MIN, ms, offset);
        }
        let _ = utc_offset(ms);
        let _ = local(ms);
    }
    let _ = from_civil(i32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, i32::MIN);
    let _ = from_civil(i32::MIN, 0, 0, 0, 0, 0, i32::MAX);
}
