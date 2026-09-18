//! What the spectrum says about a file, as numbers rather than as a picture.
//!
//! A spectrogram shows a codec's ceiling or a hum's harmonics instantly to
//! anyone looking at it, and not at all to anything sorting a thousand files.
//! These are the same observations reduced to figures that can be compared,
//! thresholded and put in a column.
//!
//! Everything is read back off a [`Spectrogram`] that has already been
//! computed, so a render and a report of the same file always agree. Frames of
//! silence are excluded from the descriptors — the centroid of a room tone is
//! a fact about the room, and averaging it in with speech describes neither.
//!
//! **Medians, not means.** Whatever is loudest dominates an average, and in
//! speech that is a handful of frames. The median frame is the file.

use super::stft::Spectrogram;

/// A distribution reduced to the three numbers worth keeping.
#[derive(Debug, Clone, Copy, Default)]
pub struct Spread {
    pub median: f32,
    pub p10: f32,
    pub p90: f32,
}

impl Spread {
    fn of(v: &mut [f32]) -> Self {
        if v.is_empty() {
            return Self {
                median: f32::NAN,
                p10: f32::NAN,
                p90: f32::NAN,
            };
        }
        v.sort_by(f32::total_cmp);
        let at = |f: f64| v[((((v.len() - 1) as f64) * f).round() as usize).min(v.len() - 1)];
        Self {
            median: at(0.5),
            p10: at(0.1),
            p90: at(0.9),
        }
    }
}

/// A frequency that stands proud of its neighbours: mains hum, or a whine.
#[derive(Debug, Clone, Copy)]
pub struct Tone {
    pub hz: f32,
    pub level_db: f32,
    /// How far above the surrounding spectrum it sits. This is what makes it a
    /// tone rather than part of the noise.
    pub prominence_db: f32,
}

/// One-third-octave band level, the way an acoustician would tabulate a
/// spectrum.
#[derive(Debug, Clone, Copy)]
pub struct Band {
    pub centre_hz: f32,
    pub level_db: f32,
}

/// Everything the spectrum pass works out.
#[derive(Debug, Clone, Default)]
pub struct Spectral {
    pub window_size: usize,
    /// Frames that went into the descriptors, after silence was left out.
    pub frames: usize,
    pub centroid_hz: Spread,
    pub rolloff85_hz: Spread,
    pub rolloff95_hz: Spread,
    /// 0 for a pure tone, 1 for white noise. What "noisy" means, numerically.
    pub flatness: Spread,
    /// The highest frequency with real content in it.
    pub cutoff_hz: f32,
    /// Whether that ceiling looks like a codec's rather than the recording's.
    pub transcode_suspect: bool,
    /// Mains hum and its harmonics, if any stand out.
    pub hum: Option<Tone>,
    pub hum_harmonics: Vec<Tone>,
    pub bands: Vec<Band>,
}

/// How far below the loudest frame a frame has to be before it is taken as
/// silence and left out of the descriptors.
const SILENCE_BELOW_PEAK_DB: f32 = 40.0;

/// How far above the shelf a bin has to be to still count as content.
const CUTOFF_MARGIN_DB: f32 = 10.0;

/// A fall of this much between one twelfth-octave step and the next is a wall
/// someone built, not a spectrum running out. Speech manages about a decibel
/// per step at the top of its range and a four-pole filter four or five, so
/// this sits far above anything that happens naturally.
const WALL_DROP_DB: f32 = 15.0;

/// How much of the spectrum a frame's own peak may tower over before the rest
/// is treated as empty. Bounds the dynamic range that goes into the flatness,
/// which otherwise measures the store's floor rather than the signal.
const FLATNESS_RANGE_DB: f64 = 80.0;

/// Below this fraction of Nyquist, a ceiling did not come from the recording.
const TRANSCODE_FRACTION: f32 = 0.75;

/// Measure a spectrogram.
///
/// `quiet` marks columns to exclude as silence; pass an empty slice to have the
/// level decide for itself. Restricting the hum search to silence is worth
/// doing when the segmentation is available — a 50 Hz line is easiest to see
/// when nobody is talking over it.
pub fn analyse(spec: &Spectrogram, quiet: &[bool]) -> Spectral {
    let bins = spec.bins;
    let columns = spec.columns();
    let mut out = Spectral {
        window_size: spec.params.window_size,
        ..Default::default()
    };
    if bins < 2 || columns == 0 {
        return out;
    }
    let hz_per_bin = spec.bin_hz(1) as f64;
    let nyquist = spec.nyquist();

    // Which columns carry something. A frame far below the loudest one is
    // room tone, and its shape is not what anyone is asking about.
    let level: Vec<f32> = (0..columns)
        .map(|c| {
            (0..bins)
                .map(|b| spec.db_at(c, b))
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    let peak = level.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let loud: Vec<bool> = level
        .iter()
        .map(|l| l.is_finite() && *l > peak - SILENCE_BELOW_PEAK_DB)
        .collect();

    let mut centroid = Vec::new();
    let mut roll85 = Vec::new();
    let mut roll95 = Vec::new();
    let mut flat = Vec::new();
    // Running sums for the average spectrum, in power, over the loud frames
    // and over the quiet ones separately: descriptors want the first, the hum
    // search wants the second.
    let mut loud_sum = vec![0.0f64; bins];
    let mut loud_n = 0usize;
    let mut quiet_sum = vec![0.0f64; bins];
    let mut quiet_n = 0usize;

    let mut mag = vec![0.0f64; bins];
    for (c, &is_loud) in loud.iter().enumerate().take(columns) {
        for (b, m) in mag.iter_mut().enumerate() {
            // Back to a linear magnitude. The store is 8-bit dB, so this is
            // coarse — good to about half a dB, which is well inside what any
            // of these descriptors claims.
            *m = 10f64.powf(spec.db_at(c, b) as f64 / 20.0);
        }
        let is_quiet = quiet.get(c).copied().unwrap_or(!is_loud);
        if is_quiet {
            for (b, m) in mag.iter().enumerate() {
                quiet_sum[b] += m * m;
            }
            quiet_n += 1;
        }
        if !is_loud {
            continue;
        }
        for (b, m) in mag.iter().enumerate() {
            loud_sum[b] += m * m;
        }
        loud_n += 1;

        let total: f64 = mag.iter().sum();
        if total <= 0.0 {
            continue;
        }
        let weighted: f64 = mag
            .iter()
            .enumerate()
            .map(|(b, m)| b as f64 * hz_per_bin * m)
            .sum();
        centroid.push((weighted / total) as f32);
        roll85.push(rolloff(&mag, hz_per_bin, 0.85));
        roll95.push(rolloff(&mag, hz_per_bin, 0.95));
        flat.push(flatness(&mag));
    }

    out.frames = loud_n;
    out.centroid_hz = Spread::of(&mut centroid);
    out.rolloff85_hz = Spread::of(&mut roll85);
    out.rolloff95_hz = Spread::of(&mut roll95);
    out.flatness = Spread::of(&mut flat);

    // The average spectrum of the loud frames, in dB, which the ceiling and
    // the band table are both read off.
    let avg_db: Vec<f32> = loud_sum
        .iter()
        .map(|p| {
            let mean = p / loud_n.max(1) as f64;
            if mean <= 0.0 {
                f32::NEG_INFINITY
            } else {
                (10.0 * mean.log10()) as f32
            }
        })
        .collect();
    let (cutoff, suspect) = cutoff(&avg_db, hz_per_bin, nyquist);
    out.cutoff_hz = cutoff;
    out.transcode_suspect = suspect;
    out.bands = third_octaves(&avg_db, hz_per_bin, nyquist);

    // Hum, from the quiet frames if there are any and the loud ones otherwise:
    // a tone is easiest to see when nothing is talking over it.
    let (hum_src, hum_n) = if quiet_n >= 4 {
        (&quiet_sum, quiet_n)
    } else {
        (&loud_sum, loud_n)
    };
    let hum_db: Vec<f32> = hum_src
        .iter()
        .map(|p| {
            let mean = p / hum_n.max(1) as f64;
            if mean <= 0.0 {
                f32::NEG_INFINITY
            } else {
                (10.0 * mean.log10()) as f32
            }
        })
        .collect();
    if let Some((mains, harmonics)) = find_hum(&hum_db, hz_per_bin) {
        out.hum = Some(mains);
        out.hum_harmonics = harmonics;
    }
    out
}

/// The frequency below which `frac` of the energy lies.
fn rolloff(mag: &[f64], hz_per_bin: f64, frac: f64) -> f32 {
    let total: f64 = mag.iter().map(|m| m * m).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let want = total * frac;
    let mut run = 0.0;
    for (b, m) in mag.iter().enumerate() {
        run += m * m;
        if run >= want {
            return (b as f64 * hz_per_bin) as f32;
        }
    }
    (mag.len() as f64 * hz_per_bin) as f32
}

/// Geometric mean over arithmetic mean of the power spectrum: 1 when every bin
/// holds the same energy, near 0 when one of them holds it all.
///
/// Floored [`FLATNESS_RANGE_DB`] below the frame's own peak. A geometric mean
/// is dominated by its smallest terms, and a spectrogram stores a fixed floor
/// far below anything audible — left alone, every frame would report the
/// flatness of that floor rather than of the sound.
fn flatness(mag: &[f64]) -> f32 {
    let peak = mag.iter().copied().fold(0.0f64, f64::max);
    if peak <= 0.0 {
        return 0.0;
    }
    let min = peak * peak * 10f64.powf(-FLATNESS_RANGE_DB / 10.0);
    let power: Vec<f64> = mag.iter().map(|m| (m * m).max(min)).collect();
    let n = power.len() as f64;
    let log_mean = power.iter().map(|p| p.ln()).sum::<f64>() / n;
    let mean = power.iter().sum::<f64>() / n;
    if mean <= 0.0 {
        return 0.0;
    }
    (log_mean.exp() / mean) as f32
}

/// Where the spectrum stops, and whether it stopped because of a codec.
///
/// The question is not "how far up does anything reach" — a loud passband
/// leaves a leakage skirt above any real ceiling, and that skirt reaches
/// wherever the analysis window's sidelobes do. The question is whether the
/// spectrum falls off a cliff, so that is what gets measured: the steepest
/// single fall on a twelfth-octave grid.
///
/// Natural material slopes. Speech loses roughly a decibel per twelfth-octave
/// at the top of its range, and even a four-pole filter manages only four or
/// five. A codec's ceiling drops tens of decibels between one step and the
/// next, which is what [`WALL_DROP_DB`] is set against.
fn cutoff(avg_db: &[f32], hz_per_bin: f64, nyquist: f32) -> (f32, bool) {
    let bins = avg_db.len();
    if bins < 8 {
        return (nyquist, false);
    }
    // A twelfth-octave grid, each point the median of the bins inside it, so
    // one noisy bin cannot invent a cliff.
    let step = 2f64.powf(1.0 / 12.0);
    let edge = step.sqrt();
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut centre = 50.0f64;
    while centre < nyquist as f64 {
        let lo = (centre / edge / hz_per_bin).floor().max(0.0) as usize;
        let hi = (((centre * edge) / hz_per_bin).ceil() as usize).min(bins);
        if lo < hi {
            let mut v: Vec<f32> = avg_db[lo..hi]
                .iter()
                .copied()
                .filter(|d| d.is_finite())
                .collect();
            if !v.is_empty() {
                v.sort_by(f32::total_cmp);
                points.push((centre as f32, v[v.len() / 2]));
            }
        }
        centre *= step;
    }
    if points.len() < 3 {
        return (nyquist, false);
    }

    let (mut at, mut worst) = (0usize, 0.0f32);
    for i in 0..points.len() - 1 {
        let drop = points[i].1 - points[i + 1].1;
        if drop > worst {
            worst = drop;
            at = i;
        }
    }
    if worst >= WALL_DROP_DB {
        // The content ends at the top of the step it fell from.
        let hz = points[at].0 * edge as f32;
        return (hz, hz < nyquist * TRANSCODE_FRACTION);
    }

    // No cliff, so report where content fades out instead — above whatever
    // the spectrum settles to — and claim nothing about why.
    let mut top: Vec<f32> = avg_db[bins - bins / 10..]
        .iter()
        .copied()
        .filter(|d| d.is_finite())
        .collect();
    if top.is_empty() {
        return (nyquist, false);
    }
    top.sort_by(f32::total_cmp);
    let limit = top[top.len() / 2] + CUTOFF_MARGIN_DB;
    // Nothing stands clear of the shelf because the spectrum *is* the shelf:
    // flat to the top, so the content reaches the top.
    let highest = (0..bins)
        .rev()
        .find(|b| avg_db[*b] > limit)
        .unwrap_or(bins - 1);
    ((highest as f64 * hz_per_bin) as f32, false)
}

/// Mains hum: a line at 50 or 60 Hz with harmonics above it.
///
/// Prominence against the median of a third of an octave either side, so a
/// spectrum that simply rises towards the bottom does not read as a tone.
fn find_hum(avg_db: &[f32], hz_per_bin: f64) -> Option<(Tone, Vec<Tone>)> {
    const MIN_PROMINENCE_DB: f32 = 6.0;
    let at = |hz: f64| -> Option<Tone> {
        let bin = (hz / hz_per_bin).round() as usize;
        if bin == 0 || bin >= avg_db.len() {
            return None;
        }
        let level = avg_db[bin];
        if !level.is_finite() {
            return None;
        }
        // A third of an octave either side, the tone itself left out.
        let lo = ((hz / 1.26) / hz_per_bin).floor().max(1.0) as usize;
        let hi = (((hz * 1.26) / hz_per_bin).ceil() as usize).min(avg_db.len() - 1);
        let mut around: Vec<f32> = (lo..=hi)
            .filter(|b| b.abs_diff(bin) > 1)
            .map(|b| avg_db[b])
            .filter(|d| d.is_finite())
            .collect();
        if around.is_empty() {
            return None;
        }
        around.sort_by(f32::total_cmp);
        let neighbours = around[around.len() / 2];
        Some(Tone {
            hz: (bin as f64 * hz_per_bin) as f32,
            level_db: level,
            prominence_db: level - neighbours,
        })
    };
    // Whichever of the two mains frequencies stands out more, if either does.
    let best = [50.0f64, 60.0]
        .into_iter()
        .filter_map(at)
        .filter(|t| t.prominence_db >= MIN_PROMINENCE_DB)
        .max_by(|a, b| a.prominence_db.total_cmp(&b.prominence_db))?;
    let harmonics = (2..=6)
        .filter_map(|n| at(best.hz as f64 * n as f64))
        .filter(|t| t.prominence_db >= MIN_PROMINENCE_DB)
        .collect();
    Some((best, harmonics))
}

/// Standard third-octave centres from 20 Hz up, each the total of the bins
/// falling inside it.
fn third_octaves(avg_db: &[f32], hz_per_bin: f64, nyquist: f32) -> Vec<Band> {
    let mut out = Vec::new();
    let mut centre = 20.0f64;
    let step = 2f64.powf(1.0 / 3.0);
    while centre <= nyquist as f64 {
        let lo = centre / step.sqrt();
        let hi = centre * step.sqrt();
        let (b0, b1) = (
            (lo / hz_per_bin).floor().max(0.0) as usize,
            ((hi / hz_per_bin).ceil() as usize).min(avg_db.len()),
        );
        if b0 < b1 {
            let power: f64 = avg_db[b0..b1]
                .iter()
                .filter(|d| d.is_finite())
                .map(|d| 10f64.powf(*d as f64 / 10.0))
                .sum();
            out.push(Band {
                centre_hz: centre as f32,
                level_db: if power > 0.0 {
                    (10.0 * power.log10()) as f32
                } else {
                    f32::NEG_INFINITY
                },
            });
        }
        centre *= step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::stft::StftParams;
    use super::*;

    fn spec_of(samples: &[f32], sr: u32, window: usize) -> Spectrogram {
        let params = StftParams {
            window_size: window,
            ..Default::default()
        };
        Spectrogram::compute(samples, sr, params, &|_| {}, &|| false).unwrap()
    }

    fn sine(freq: f32, sr: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect()
    }

    /// Delete everything above `hz`, leaving a vertical edge.
    fn brick_wall(x: &[f32], sr: u32, hz: f32) -> Vec<f32> {
        use realfft::RealFftPlanner;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(x.len());
        let inv = planner.plan_fft_inverse(x.len());
        let mut input = x.to_vec();
        let mut spectrum = fwd.make_output_vec();
        fwd.process(&mut input, &mut spectrum).unwrap();
        let cut = (hz as f64 * x.len() as f64 / sr as f64).round() as usize;
        for (i, bin) in spectrum.iter_mut().enumerate() {
            if i > cut {
                *bin = Default::default();
            }
        }
        let mut out = inv.make_output_vec();
        inv.process(&mut spectrum, &mut out).unwrap();
        let scale = 1.0 / x.len() as f32;
        out.iter().map(|v| v * scale).collect()
    }

    fn noise(sr: u32, n: usize, amp: f32) -> Vec<f32> {
        // A cheap deterministic hash, so the test is the same run to run.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let _ = sr;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Twelve bits, scaled to -1..1. Taking more of them than that
                // puts the signal far outside full scale and gives it a DC
                // offset that buries everything below a few hundred hertz.
                amp * (((state >> 40) & 0xFFF) as f32 / 2048.0 - 1.0)
            })
            .collect()
    }

    /// A pure tone is at its own frequency and is not noisy; noise is spread
    /// out and is. These two are the sanity check on every other number here.
    #[test]
    fn a_tone_and_noise_sit_at_opposite_ends() {
        let sr = 48_000;
        let tone = analyse(&spec_of(&sine(1000.0, sr, sr as usize, 0.5), sr, 4096), &[]);
        assert!(
            (tone.centroid_hz.median - 1000.0).abs() < 60.0,
            "centroid {}",
            tone.centroid_hz.median
        );
        assert!(
            tone.flatness.median < 0.05,
            "a tone is not flat: {}",
            tone.flatness.median
        );

        let hiss = analyse(&spec_of(&noise(sr, sr as usize, 0.3), sr, 4096), &[]);
        assert!(
            hiss.flatness.median > 0.3,
            "noise should read flat: {}",
            hiss.flatness.median
        );
        assert!(
            hiss.centroid_hz.median > 8000.0,
            "white noise sits high: {}",
            hiss.centroid_hz.median
        );
    }

    /// A lowpassed file has a ceiling, and it is the one that was applied.
    #[test]
    fn a_lowpass_is_found_where_it_was_put() {
        let sr = 48_000;
        let wide = noise(sr, sr as usize, 0.3);
        // A true wall, made by deleting the bins rather than by filtering: a
        // codec's ceiling is vertical, and that is the thing being detected.
        // Any ordinary filter slopes, and "where the content ends" then
        // depends on how far down one cares to look.
        let cut = brick_wall(&wide, sr, 6000.0);
        let s = analyse(&spec_of(&cut, sr, 4096), &[]);
        assert!(
            (s.cutoff_hz - 6000.0).abs() < 400.0,
            "cutoff {} should be near 6 kHz",
            s.cutoff_hz
        );
        assert!(
            s.transcode_suspect,
            "6 kHz of 24 is well under three quarters"
        );

        // Full-band noise has no ceiling to find.
        let full = analyse(&spec_of(&wide, sr, 4096), &[]);
        assert!(!full.transcode_suspect);
        assert!(full.cutoff_hz > 18_000.0, "cutoff {}", full.cutoff_hz);
    }

    /// Speech slopes off steeply but smoothly, all the way down. Reading that
    /// slope as a ceiling would call every voice recording a transcode, which
    /// is what happened when the edge was measured down from the peak instead
    /// of up from the shelf.
    #[test]
    fn a_natural_slope_is_not_a_codec_ceiling() {
        let sr = 48_000;
        let n = sr as usize;
        // Pink-ish: noise with the top rolled off gently, like a voice.
        let mut x = noise(sr, n, 0.3);
        for _ in 0..2 {
            x = super::super::envelope::band_limit(&x, sr, 100.0, 3000.0);
        }
        let s = analyse(&spec_of(&x, sr, 4096), &[]);
        assert!(
            !s.transcode_suspect,
            "a gentle rolloff at {} Hz is not a wall",
            s.cutoff_hz
        );
    }

    /// Hum is a line above its neighbours, and its harmonics come with it.
    #[test]
    fn mains_hum_is_found_under_noise() {
        let sr = 48_000;
        let n = sr as usize * 2;
        let mut x = noise(sr, n, 10f32.powf(-70.0 / 20.0));
        for (i, v) in x.iter_mut().enumerate() {
            let t = std::f32::consts::TAU * i as f32 / sr as f32;
            *v += 10f32.powf(-40.0 / 20.0) * (t * 50.0).sin();
            *v += 10f32.powf(-46.0 / 20.0) * (t * 100.0).sin();
        }
        let s = analyse(&spec_of(&x, sr, 16384), &[]);
        let hum = s.hum.expect("50 Hz should be found");
        assert!((hum.hz - 50.0).abs() < 3.0, "hum at {}", hum.hz);
        assert!(hum.prominence_db > 6.0);
        assert!(
            !s.hum_harmonics.is_empty(),
            "the 100 Hz harmonic should come with it"
        );

        // Noise alone has no tone in it.
        let clean = analyse(&spec_of(&noise(sr, n, 0.01), sr, 16384), &[]);
        assert!(clean.hum.is_none());
    }

    #[test]
    fn third_octave_bands_cover_the_range_in_order() {
        let sr = 48_000;
        let s = analyse(&spec_of(&sine(1000.0, sr, sr as usize, 0.5), sr, 4096), &[]);
        assert!(s.bands.len() > 25, "20 Hz to Nyquist is about 31 bands");
        assert!((s.bands[0].centre_hz - 20.0).abs() < 0.1);
        for w in s.bands.windows(2) {
            assert!(w[1].centre_hz > w[0].centre_hz);
        }
        // The band holding the tone is the loudest of them.
        let loudest = s
            .bands
            .iter()
            .max_by(|a, b| a.level_db.total_cmp(&b.level_db))
            .unwrap();
        assert!(
            (loudest.centre_hz - 1000.0).abs() < 130.0,
            "loudest band at {} Hz",
            loudest.centre_hz
        );
    }

    #[test]
    fn an_empty_spectrogram_measures_nothing() {
        let s = analyse(&spec_of(&[], 48_000, 1024), &[]);
        assert_eq!(s.frames, 0);
        assert!(s.bands.is_empty());
        assert!(s.hum.is_none());
    }
}
