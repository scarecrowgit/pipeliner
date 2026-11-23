//! Local-time helpers for "12m", "Sat 18:20", "Nov 9" style labels and calendar
//! grouping (no date crate: the UTC offset comes from libc `localtime_r`).

/// How often relative-time labels ("now", "12m") are re-rendered.
pub const AGE_REFRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// Re-render the entity every `AGE_REFRESH` while it is alive, so relative ages stay current.
pub fn refresh_ages<T: 'static>(cx: &mut gpui::Context<T>) -> gpui::Task<()> {
    refresh_ages_when(cx, |_| true)
}

/// Like [`refresh_ages`], but only notifies while `needed(&entity)` holds (e.g. a picker is open).
pub fn refresh_ages_when<T: 'static>(
    cx: &mut gpui::Context<T>,
    needed: impl Fn(&T) -> bool + 'static,
) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(AGE_REFRESH).await;
            let alive = this.update(cx, |this, cx| {
                if needed(this) {
                    cx.notify();
                }
            });
            if alive.is_err() {
                return;
            }
        }
    })
}

pub const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// A local calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Days since 1970-01-01 (local).
    pub days: i64,
    /// 0 = Monday.
    pub weekday: u32,
}

/// Local UTC offset in seconds at unix time `t`.
pub fn utc_offset(t: u64) -> i64 {
    let tt = t as libc::time_t;
    // SAFETY: localtime_r only writes into the provided `tm`.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&tt, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff
        }
    }
}

/// Local date of unix time `t` given the offset.
pub fn date_of(t: u64, offset: i64) -> Date {
    let days = (t as i64 + offset).div_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    Date {
        year,
        month,
        day,
        days,
        weekday: (days + 3).rem_euclid(7) as u32,
    }
}

/// Short "when": `12m`, `3h`, `Sat 18:20`, `Nov 9`, `Nov 9 2025`.
pub fn when_label(t: u64, now: u64, offset: i64) -> String {
    let secs = now.saturating_sub(t);
    if secs < 60 {
        return "now".into();
    }
    if secs < 3600 {
        return format!("{}m", secs / 60);
    }
    let d = date_of(t, offset);
    let today = date_of(now, offset);
    if d.days == today.days {
        return format!("{}h", secs / 3600);
    }
    if today.days - d.days < 7 {
        let local = (t as i64 + offset).rem_euclid(86_400);
        return format!(
            "{} {:02}:{:02}",
            WEEKDAYS[d.weekday as usize],
            local / 3600,
            local % 3600 / 60
        );
    }
    let m = MONTHS_SHORT[(d.month as usize).saturating_sub(1) % 12];
    if d.year == today.year {
        format!("{m} {}", d.day)
    } else {
        format!("{m} {} {}", d.day, d.year)
    }
}

/// Compact "when" for the sidebar: `12m`, `3h`, `Sat`, `Nov 9`.
pub fn when_short(t: u64, now: u64, offset: i64) -> String {
    let full = when_label(t, now, offset);
    match full.split_once(' ') {
        Some((day, time)) if time.contains(':') => day.to_string(),
        _ => full,
    }
}

/// `Sunday, 22 November`.
pub fn long_date(t: u64, offset: i64) -> String {
    const DAYS: [&str; 7] = [
        "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
    ];
    let d = date_of(t, offset);
    format!(
        "{}, {} {}",
        DAYS[d.weekday as usize],
        d.day,
        MONTHS[(d.month as usize).saturating_sub(1) % 12]
    )
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}


/// `January` … `December` for month 1-12.
pub fn month_name(month: u32) -> &'static str {
    MONTHS[(month as usize).saturating_sub(1) % 12]
}
