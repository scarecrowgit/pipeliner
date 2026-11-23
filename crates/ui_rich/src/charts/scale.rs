//! Axis scales: "nice" ceilings and evenly spaced ticks. Pure, never panics.

/// The smallest "nice" number (1, 2, 2.5, 5 or 10 times a power of ten) that is at least
/// `value`: `3.81` gives `5`, `8.8` gives `10`, `61.2` gives `100`.
///
/// Zero, negative, NaN and infinite values give `0`.
pub fn nice_ceiling(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    let exp = value.log10().floor().clamp(-300.0, 300.0) as i32;
    let base = 10f64.powi(exp);
    let mantissa = value / base;
    for step in [1.0, 2.0, 2.5, 5.0, 10.0] {
        // A hair of slack so that `10.0` stays `10` despite rounding in `log10`.
        if mantissa <= step * (1.0 + 1e-9) {
            return step * base;
        }
    }
    10.0 * base
}

/// `intervals + 1` evenly spaced values from `0` to `max`, ascending: `ticks(10., 2)` is
/// `[0, 5, 10]`.
///
/// `intervals` is at least 1. A `max` that is not a positive finite number gives all zeros.
pub fn ticks(max: f64, intervals: usize) -> Vec<f64> {
    let n = intervals.max(1);
    let max = if max.is_finite() && max > 0.0 { max } else { 0.0 };
    (0..=n).map(|i| max * i as f64 / n as f64).collect()
}

/// The three y-axis values of a chart, top to bottom: `[max, max / 2, 0]`.
pub fn axis_ticks(max: f64) -> [f64; 3] {
    let t = ticks(max, 2);
    [t[2], t[1], t[0]]
}

/// `value` as a share of `max`, clamped to `0..=1`; zero for anything that is not a positive
/// finite number, or when `max` is not one. Never NaN.
pub fn fraction(value: f64, max: f64) -> f32 {
    if !value.is_finite() || value <= 0.0 || !max.is_finite() || max <= 0.0 {
        return 0.0;
    }
    (value / max).clamp(0.0, 1.0) as f32
}
