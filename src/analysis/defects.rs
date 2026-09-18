//! Things that are wrong with a file: silence that is too silent, transients
//! that are too sharp, and joins that show.
//!
//! Every detector here reports *where* and *how much*, never a verdict. A
//! click at −80 dB in a pause and a click at −6 dB mid-word are the same
//! measurement and very different problems, and which one matters is the
//! caller's business — or a person's.
//!
//! The thresholds are conventions with reasons, not truths. Where one came
//! from a published calibration it says so, because a number a reader cannot
//! trace is a number they cannot argue with.

use super::envelope::Envelope;

/// A run of samples that are exactly zero.
///
/// Exact, not "quiet". Analogue silence has a floor; digital silence is an
/// edit, a mute or a pad, and the difference is the whole point of measuring
/// it. A recording that never touches zero for a millisecond at a time did not
/// get there by accident.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZeroRun {
    pub start_frame: usize,
    pub end_frame: usize,
}

impl ZeroRun {
    pub fn len(&self) -> usize {
        self.end_frame - self.start_frame
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Runs of digital silence at least `min_len` samples long.
pub fn zero_runs(samples: &[f32], min_len: usize) -> Vec<ZeroRun> {
    let min_len = min_len.max(1);
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &v) in samples.iter().enumerate() {
        match (v == 0.0, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push(ZeroRun {
                        start_frame: s,
                        end_frame: i,
                    });
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start
        && samples.len() - s >= min_len
    {
        out.push(ZeroRun {
            start_frame: s,
            end_frame: samples.len(),
        });
    }
    out
}

/// A step between neighbouring samples far larger than the signal around it.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub frame: usize,
    /// The step, in dB relative to the local RMS. This is the number the
    /// threshold is set in.
    pub ratio_db: f32,
}

/// How long a window the step is measured against. Short enough to be "local"
/// within a word, long enough to hold a couple of cycles of a low voice.
pub const LOCAL_MS: usize = 20;

/// Sample-to-sample steps that stand out from their surroundings.
///
/// Measured against a **local** RMS rather than the file's: whole-file RMS is
/// dominated by speech, against which a −50 dB step inside a pause is 25 dB
/// down and invisible. Locally it is the loudest thing for 20 ms.
///
/// `threshold_db` of 32 is the delivery convention this was built against —
/// p99 of natural transients over 200 unedited takes measured 28.6 dB, and 32
/// leaves headroom above that. Genuine speech plosives do reach it, which is
/// why each hit is reported with its level rather than counted.
pub fn clicks(samples: &[f32], sample_rate: u32, threshold_db: f32) -> Vec<Click> {
    let n = samples.len();
    if n < 3 {
        return Vec::new();
    }
    // Prefix sums of squares, so the RMS over any window is two lookups
    // rather than a second loop per sample.
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + (samples[i] as f64) * (samples[i] as f64);
    }
    let half = (sample_rate as usize * LOCAL_MS / 2000).max(1);
    // A guard either side, left out of the level the step is measured against.
    // A loud enough click drags the RMS of any window containing it upwards,
    // and so hides itself: a full-scale spike in a −60 dB pause reads as a
    // modest step against a window it has single-handedly made loud.
    let guard = (sample_rate as usize / 1000).max(1);
    let merge = (sample_rate as usize * 2 / 1000).max(1);
    let sum = |lo: usize, hi: usize| -> (f64, usize) {
        if hi <= lo {
            (0.0, 0)
        } else {
            (prefix[hi] - prefix[lo], hi - lo)
        }
    };

    let mut out: Vec<Click> = Vec::new();
    let mut open: Option<usize> = None;
    for i in 1..n {
        let delta = (samples[i] - samples[i - 1]).abs();
        if delta == 0.0 {
            continue;
        }
        let (a, an) = sum(i.saturating_sub(half), i.saturating_sub(guard));
        let (b, bn) = sum((i + guard).min(n), (i + half).min(n));
        if an + bn == 0 {
            continue;
        }
        // Floored at the quietest thing a 32-bit file can hold, so a step out
        // of digital silence reads as very large rather than as infinite.
        let rms = ((a + b) / (an + bn) as f64).sqrt().max(1e-9);
        let ratio = (20.0 * (delta as f64 / rms).log10()) as f32;
        if ratio < threshold_db {
            continue;
        }
        // One transient spans many samples. Merge by distance from the last
        // sample that crossed, not from the peak, or the tail of a wide
        // transient starts a second event of its own.
        match (open, out.last_mut()) {
            (Some(prev), Some(last)) if i - prev <= merge => {
                if ratio > last.ratio_db {
                    last.frame = i;
                    last.ratio_db = ratio;
                }
            }
            _ => out.push(Click {
                frame: i,
                ratio_db: ratio,
            }),
        }
        open = Some(i);
    }
    out
}

/// A join between two pieces of audio, seen in the floor rather than heard.
#[derive(Debug, Clone, Copy)]
pub struct Seam {
    pub frame: usize,
    /// How far the level jumps across the join, in dB.
    pub floor_step_db: f32,
}

/// Places where the noise floor steps.
///
/// Room tone does not change level abruptly; an edit between two takes, or
/// between tone and something laid over it, does. The comparison is between
/// the `window_ms` before a point and the `window_ms` after it, walked at the
/// envelope's hop.
///
/// `skip` marks frames to leave out — speech, normally. Inside speech the level
/// changes constantly and every syllable would be a seam. This detector only
/// means anything in the quiet.
pub fn floor_steps(
    envelope: &Envelope,
    skip: &[bool],
    window_ms: usize,
    threshold_db: f32,
) -> Vec<Seam> {
    let span = (window_ms / envelope.hop_ms().max(1)).max(1);
    let mut out: Vec<Seam> = Vec::new();
    let mut open: Option<usize> = None;
    if envelope.len() < span * 2 + 1 {
        return out;
    }
    for i in span..envelope.len() - span {
        // Both sides must be quiet, and so must the point itself: a step at the
        // edge of a word is the word, not a seam.
        if skip.get(i).copied().unwrap_or(false)
            || (i - span..i + span).any(|j| skip.get(j).copied().unwrap_or(false))
        {
            continue;
        }
        let mean = |r: std::ops::Range<usize>| -> Option<f32> {
            let v: Vec<f32> = r
                .filter_map(|j| envelope.rms_db.get(j).copied())
                .filter(|d| d.is_finite())
                .collect();
            (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
        };
        let (Some(before), Some(after)) = (mean(i - span..i), mean(i + 1..i + 1 + span)) else {
            continue;
        };
        let step = (after - before).abs();
        if step < threshold_db {
            continue;
        }
        let frame = i * envelope.hop_len;
        // As the window slides across one join the difference rises to a peak
        // and falls away again, so a run of frames crosses the threshold.
        // Merge on the distance from the last frame that crossed; measuring
        // from the peak instead would split the falling side into a second
        // seam once it drifted far enough from it.
        match (open, out.last_mut()) {
            (Some(prev), Some(last)) if i - prev <= 1 => {
                if step > last.floor_step_db {
                    last.frame = frame;
                    last.floor_step_db = step;
                }
            }
            _ => out.push(Seam {
                frame,
                floor_step_db: step,
            }),
        }
        open = Some(i);
    }
    out
}

/// Whether the very start or end of a file still has signal in it, which means
/// the take was cut into rather than allowed to finish.
#[derive(Debug, Clone, Copy)]
pub struct Truncation {
    pub head: bool,
    pub tail: bool,
    pub head_db: f32,
    pub tail_db: f32,
}

/// `edge_ms` at each end against the file's own floor: signal more than
/// `above_floor_db` over it at the very first or last sample is a cut.
pub fn truncation(
    samples: &[f32],
    sample_rate: u32,
    floor_db: f32,
    edge_ms: usize,
    above_floor_db: f32,
) -> Truncation {
    let n = (sample_rate as usize * edge_ms / 1000)
        .max(1)
        .min(samples.len());
    let rms = |s: &[f32]| -> f32 {
        if s.is_empty() {
            return f32::NEG_INFINITY;
        }
        let sq: f64 = s.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        let r = (sq / s.len() as f64).sqrt();
        if r <= 0.0 {
            f32::NEG_INFINITY
        } else {
            (20.0 * r.log10()) as f32
        }
    };
    let head_db = rms(&samples[..n]);
    let tail_db = rms(&samples[samples.len().saturating_sub(n)..]);
    let limit = floor_db + above_floor_db;
    Truncation {
        head: head_db > limit,
        tail: tail_db > limit,
        head_db,
        tail_db,
    }
}

impl Envelope {
    /// The hop, in milliseconds, which is the unit the window sizes above are
    /// counted in.
    pub fn hop_ms(&self) -> usize {
        (self.hop_len * 1000 / self.sample_rate.max(1) as usize).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect()
    }

    #[test]
    fn zero_runs_are_found_at_the_length_asked_for() {
        let mut x = sine(1000.0, 48_000, 4800, 0.5);
        for v in x.iter_mut().take(2000).skip(1000) {
            *v = 0.0;
        }
        let runs = zero_runs(&x, 480); // 10 ms at 48 kHz
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start_frame, 1000);
        assert_eq!(runs[0].end_frame, 2000);
        // Shorter than asked for is not a run.
        assert!(zero_runs(&x, 2000).is_empty());
    }

    #[test]
    fn a_run_at_the_very_end_still_counts() {
        let mut x = sine(1000.0, 48_000, 2000, 0.5);
        for v in x.iter_mut().skip(1000) {
            *v = 0.0;
        }
        let runs = zero_runs(&x, 480);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].end_frame, 2000);
    }

    /// A step dropped into a quiet stretch is found, and the clean signal it
    /// was dropped into is not.
    #[test]
    fn a_click_stands_out_from_its_surroundings() {
        let sr = 48_000;
        let mut x = sine(200.0, sr, sr as usize, 0.02); // a quiet tone
        assert!(
            clicks(&x, sr, 32.0).is_empty(),
            "a plain tone has no clicks in it"
        );
        // 0.5 on a 0.02 tone is only about 31 dB — under the threshold, and
        // rightly so. A click has to clear natural transients to count.
        x[24_000] = 1.0;
        let found = clicks(&x, sr, 32.0);
        assert_eq!(found.len(), 1, "one transient, not one per sample");
        assert!(found[0].frame.abs_diff(24_000) <= 2);
        assert!(found[0].ratio_db > 32.0);
    }

    /// The measure is local: the same step in a loud passage is ordinary, and
    /// in a quiet one it is not. This is why whole-file RMS cannot be used.
    #[test]
    fn the_same_step_reads_differently_against_different_neighbours() {
        let sr = 48_000;
        let step = 0.05;
        let quiet = {
            let mut x = sine(200.0, sr, sr as usize, 0.001);
            x[24_000] += step;
            clicks(&x, sr, 32.0)
        };
        let loud = {
            let mut x = sine(200.0, sr, sr as usize, 0.5);
            x[24_000] += step;
            clicks(&x, sr, 32.0)
        };
        assert_eq!(quiet.len(), 1, "against a -60 dB floor this is a click");
        assert!(loud.is_empty(), "against a -6 dB signal it is nothing");
    }

    /// A join between two different room tones shows as a step in the floor.
    #[test]
    fn a_floor_step_is_a_seam() {
        let sr = 48_000;
        let mut x = sine(1000.0, sr, sr as usize, 10f32.powf(-60.0 / 20.0));
        for (i, v) in x.iter_mut().enumerate() {
            if i >= sr as usize / 2 {
                *v *= 10f32.powf(12.0 / 20.0); // 12 dB louder from halfway
            }
        }
        let e = Envelope::compute(&x, sr, 20, 5);
        let skip = vec![false; e.len()];
        let seams = floor_steps(&e, &skip, 50, 6.0);
        assert_eq!(seams.len(), 1, "one join, one seam");
        let at = seams[0].frame as f64 / sr as f64;
        assert!((at - 0.5).abs() < 0.05, "seam at {at} s, expected 0.5");
        assert!(seams[0].floor_step_db > 6.0);

        // Steady tone, no seam.
        let flat = Envelope::compute(&sine(1000.0, sr, sr as usize, 0.001), sr, 20, 5);
        assert!(floor_steps(&flat, &vec![false; flat.len()], 50, 6.0).is_empty());
    }

    /// Marked regions are not examined: inside speech every syllable steps.
    #[test]
    fn skipped_frames_produce_no_seams() {
        let sr = 48_000;
        let mut x = sine(1000.0, sr, sr as usize, 10f32.powf(-60.0 / 20.0));
        for (i, v) in x.iter_mut().enumerate() {
            if i >= sr as usize / 2 {
                *v *= 10f32.powf(12.0 / 20.0);
            }
        }
        let e = Envelope::compute(&x, sr, 20, 5);
        let skip = vec![true; e.len()];
        assert!(floor_steps(&e, &skip, 50, 6.0).is_empty());
    }

    #[test]
    fn a_file_cut_into_reads_as_truncated() {
        let sr = 48_000;
        let loud = sine(1000.0, sr, sr as usize, 0.5);
        let t = truncation(&loud, sr, -60.0, 5, 20.0);
        assert!(t.head && t.tail, "full-scale tone at both edges is a cut");

        let mut faded = loud.clone();
        let edge = sr as usize / 100;
        for i in 0..edge {
            faded[i] *= 0.0;
            let n = faded.len() - 1 - i;
            faded[n] *= 0.0;
        }
        let t = truncation(&faded, sr, -60.0, 5, 20.0);
        assert!(!t.head && !t.tail, "silence at the edges is not a cut");
    }
}
