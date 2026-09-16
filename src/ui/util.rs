/// `m:ss.mmm`, or `h:mm:ss.mmm` past an hour.
pub fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor() as u64;
    let m = ((secs % 3600.0) / 60.0).floor() as u64;
    let s = secs % 60.0;
    if h > 0 {
        format!("{h}:{m:02}:{s:06.3}")
    } else {
        format!("{m}:{s:06.3}")
    }
}

/// Short form for rulers: drops millis when the step is coarse.
pub fn fmt_time_step(secs: f64, step: f64) -> String {
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor() as u64;
    let m = ((secs % 3600.0) / 60.0).floor() as u64;
    let s = secs % 60.0;
    let body = if step >= 1.0 {
        format!("{m}:{:02}", s.round() as u64)
    } else if step >= 0.01 {
        format!("{m}:{s:05.2}")
    } else {
        format!("{m}:{s:06.3}")
    };
    if h > 0 { format!("{h}:{body}") } else { body }
}

/// A "nice" tick step in seconds for a ruler covering `visible_secs` in
/// `width_px`, aiming for at least `min_px` between ticks.
pub fn nice_time_step(visible_secs: f64, width_px: f32, min_px: f32) -> f64 {
    const STEPS: [f64; 22] = [
        0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0,
        60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0,
    ];
    let min_secs = visible_secs * (min_px / width_px.max(1.0)) as f64;
    STEPS
        .iter()
        .copied()
        .find(|s| *s >= min_secs)
        .unwrap_or(3600.0)
}

pub fn fmt_hz(hz: f32) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k.fract().abs() < 0.05 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{hz:.0}")
    }
}

/// `fmt_time` right-aligned in a field as wide as the longest time the clip
/// can show, so a readout that updates every frame keeps its neighbours put
/// instead of shuffling them when the minute rolls over to two digits.
pub fn fmt_time_field(secs: f64, total_secs: f64) -> String {
    let w = fmt_time(total_secs.max(secs)).len();
    format!("{:>w$}", fmt_time(secs))
}
