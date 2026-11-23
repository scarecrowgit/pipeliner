//! The clock, local calendar math and the short time labels of the UI.
//!
//! Times are unix epoch milliseconds (`i64`) everywhere. Calendar functions take the UTC
//! offset as an argument so they are pure; get it once from [`utc_offset`] (or use [`local`])
//! and pass it down. There is no date crate: the offset comes from libc.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests;

const MS_PER_DAY: i64 = 86_400_000;
const SECS_PER_DAY: i64 = 86_400;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] =
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// A source of the current time. Code that stamps or compares times takes a `&dyn Clock` so
/// tests can pin it with [`FixedClock`].
pub trait Clock: Send + Sync {
    /// The current time in unix epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// The real wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        now_ms()
    }
}

/// A clock that only moves when told to, for tests.
#[derive(Debug, Default)]
pub struct FixedClock(pub AtomicI64);

impl FixedClock {
    /// A clock standing at `ms`.
    pub fn new(ms: i64) -> Self {
        Self(AtomicI64::new(ms))
    }

    /// Moves the clock to `ms`.
    pub fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }

    /// Moves the clock forward by `ms` (back when negative).
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// The wall clock in unix epoch milliseconds.
pub fn now_ms() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// A moment broken into calendar fields in some UTC offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    /// Calendar year, e.g. 2026.
    pub year: i32,
    /// Month, 1 to 12.
    pub month: u32,
    /// Day of the month, 1 to 31.
    pub day: u32,
    /// Hour, 0 to 23.
    pub hour: u32,
    /// Minute, 0 to 59.
    pub minute: u32,
    /// Second, 0 to 59.
    pub second: u32,
    /// Day of the week, 0 = Monday to 6 = Sunday.
    pub weekday: u32,
    /// The offset these fields were computed in, seconds east of UTC.
    pub utc_offset_secs: i32,
}

/// The machine's UTC offset (seconds east of UTC) in effect at `ms`, daylight saving
/// included. 0 when the system cannot tell.
pub fn utc_offset(ms: i64) -> i32 {
    // `time_t` is `i64` on 64-bit targets (the conversion cannot fail there) and `i32` on 32-bit.
    #[allow(irrefutable_let_patterns)]
    let Ok(secs) = libc::time_t::try_from(ms.div_euclid(1000)) else {
        return 0;
    };
    // SAFETY: `localtime_r` reads `secs` and writes only into the `tm` we own; an all-zero
    // `tm` (integers and a null `tm_zone`) is a valid value to hand it.
    let offset = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() { 0 } else { tm.tm_gmtoff }
    };
    i32::try_from(offset).unwrap_or(0)
}

/// `ms` as calendar fields in the machine's time zone.
pub fn local(ms: i64) -> LocalTime {
    civil(ms, utc_offset(ms))
}

/// `ms` as calendar fields in the given UTC offset.
pub fn civil(ms: i64, utc_offset_secs: i32) -> LocalTime {
    let secs = ms.div_euclid(1000).saturating_add(i64::from(utc_offset_secs));
    let days = secs.div_euclid(SECS_PER_DAY);
    let of_day = secs.rem_euclid(SECS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    LocalTime {
        year,
        month,
        day,
        hour: (of_day / 3600) as u32,
        minute: (of_day % 3600 / 60) as u32,
        second: (of_day % 60) as u32,
        // 1970-01-01 was a Thursday.
        weekday: (days + 3).rem_euclid(7) as u32,
        utc_offset_secs,
    }
}

/// The inverse of [`civil`]: epoch milliseconds of a calendar date and time in the given UTC
/// offset. Day and time fields past their range roll over (`day` 32 is the 1st of the next
/// month); `month` is clamped to 1..=12.
pub fn from_civil(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    utc_offset_secs: i32,
) -> i64 {
    let days = days_from_civil(i64::from(year), month.clamp(1, 12), 1) + i64::from(day) - 1;
    let secs = days * SECS_PER_DAY
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
        - i64::from(utc_offset_secs);
    secs.saturating_mul(1000)
}

/// Milliseconds of the local midnight that starts the day containing `ms`.
pub fn day_start(ms: i64, utc_offset_secs: i32) -> i64 {
    let offset_ms = i64::from(utc_offset_secs) * 1000;
    let local = ms.saturating_add(offset_ms);
    local.saturating_sub(local.rem_euclid(MS_PER_DAY)).saturating_sub(offset_ms)
}

/// Milliseconds of the local Monday midnight that starts the week containing `ms`.
pub fn week_start(ms: i64, utc_offset_secs: i32) -> i64 {
    let weekday = i64::from(civil(ms, utc_offset_secs).weekday);
    day_start(ms, utc_offset_secs).saturating_sub(weekday * MS_PER_DAY)
}

/// ISO 8601 week number (1 to 53) of the local date of `ms`, for "W42" labels. The first days
/// of January can belong to week 52 or 53 of the previous year.
pub fn iso_week(ms: i64, utc_offset_secs: i32) -> u32 {
    let days = local_days(ms, utc_offset_secs);
    // The week belongs to the year of its Thursday.
    let thursday = days - (days + 3).rem_euclid(7) + 3;
    let (year, _, _) = civil_from_days(thursday);
    let jan1 = days_from_civil(i64::from(year), 1, 1);
    ((thursday - jan1) / 7 + 1).clamp(1, 53) as u32
}

/// How long ago `then_ms` was, as short as it can be said: `now` under a minute (and for
/// times in the future), `12m`, `3h` under a day, the weekday (`Sat`) within the last week,
/// `12 Oct` this year, `12 Oct 2025` otherwise.
pub fn ago(now_ms: i64, then_ms: i64, utc_offset_secs: i32) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    if secs < 60 {
        return "now".to_string();
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    if secs < SECS_PER_DAY {
        return format!("{}h", secs / 3600);
    }
    let then = civil(then_ms, utc_offset_secs);
    let days_back = local_days(now_ms, utc_offset_secs) - local_days(then_ms, utc_offset_secs);
    if days_back < 7 {
        return weekday_short(then.weekday).to_string();
    }
    let month = month_short(then.month);
    if then.year == civil(now_ms, utc_offset_secs).year {
        format!("{} {month}", then.day)
    } else {
        format!("{} {month} {}", then.day, then.year)
    }
}

/// The date written out for headings: `Sunday, 22 November`.
pub fn long_date(ms: i64, utc_offset_secs: i32) -> String {
    let t = civil(ms, utc_offset_secs);
    format!("{}, {} {}", weekday_name(t.weekday), t.day, month_name(t.month))
}

/// The time of day on a 24-hour clock: `18:40`.
pub fn clock_time(ms: i64, utc_offset_secs: i32) -> String {
    let t = civil(ms, utc_offset_secs);
    format!("{:02}:{:02}", t.hour, t.minute)
}

/// `January` to `December` for month 1 to 12.
pub fn month_name(month: u32) -> &'static str {
    MONTHS[(month.clamp(1, 12) - 1) as usize]
}

/// `Jan` to `Dec` for month 1 to 12.
pub fn month_short(month: u32) -> &'static str {
    &month_name(month)[..3]
}

/// `Monday` to `Sunday` for weekday 0 to 6.
pub fn weekday_name(weekday: u32) -> &'static str {
    WEEKDAYS[(weekday % 7) as usize]
}

/// `Mon` to `Sun` for weekday 0 to 6.
pub fn weekday_short(weekday: u32) -> &'static str {
    &weekday_name(weekday)[..3]
}

/// Days since 1970-01-01 of the local date of `ms`.
fn local_days(ms: i64, utc_offset_secs: i32) -> i64 {
    ms.div_euclid(1000).saturating_add(i64::from(utc_offset_secs)).div_euclid(SECS_PER_DAY)
}

/// Year, month and day of a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    // An i64 of milliseconds spans about ±292 million years, which fits an i32.
    (year.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32, month, day)
}

/// Day count since 1970-01-01 of a calendar date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
