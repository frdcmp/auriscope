//! Loudness, peaks, clipping, DC offset and stereo correlation. One pass.
//!
//! Everything here measures whatever slices it is handed, so the same code
//! answers for a whole file or for half a second of one: the caller does the
//! slicing and says so in its own output.

use anyhow::Result;
use ebur128::{EbuR128, Mode};

#[derive(Debug, Clone, Default)]
pub struct ChannelStats {
    pub sample_peak_db: f32,
    pub true_peak_dbtp: f32,
    pub clipped_samples: usize,
    pub clipped_runs: usize,
    pub dc_offset: f32,
    pub rms_db: f32,
    /// Level exceeded 10%, 50% and 90% of the time, over short frames.
    ///
    /// The three together say more than an average does: `l10` is where the
    /// loud parts sit, `l50` the middle, and `l90` the quiet bed the signal
    /// rests on. A file of speech in a quiet room has a wide spread; a file of
    /// constant noise has almost none.
    pub l10_db: f32,
    pub l50_db: f32,
    pub l90_db: f32,
    /// The quietest tenth of the frames, which is what a noise floor is when
    /// nobody has marked up where the silence is. Frames of digital zero take
    /// no part in it: a file padded with zeros has a floor, not −infinity.
    pub noise_floor_db: f32,
    /// How many frames went into those four, and how many were all zero.
    pub level_frames: usize,
    pub zero_frames: usize,
}

/// Loudness and level against time, one entry per [`BLOCK_MS`] block.
///
/// The loudness meter is fed in blocks and asked after each one, so this costs
/// nothing beyond keeping what was already being read.
///
/// `None` until the meter's window has actually filled — momentary needs
/// 400 ms and short-term 3 s. Asked earlier it still answers, but it divides by
/// the whole window whether or not it has one, so the opening blocks read
/// progressively too quiet. Left in, a file would appear to fade in for its
/// first three seconds, every time. The maxima ignore them for the same
/// reason.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub block_secs: f32,
    pub momentary_lufs: Vec<Option<f32>>,
    pub short_term_lufs: Vec<Option<f32>>,
    /// Per channel, the RMS and the peak of each block.
    pub channels: Vec<ChannelTimeline>,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelTimeline {
    pub rms_db: Vec<f32>,
    pub peak_db: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct FileStats {
    pub integrated_lufs: f32,
    pub loudness_range_lu: f32,
    pub max_momentary_lufs: f32,
    pub max_short_term_lufs: f32,
    pub channels: Vec<ChannelStats>,
    /// Pearson correlation between channels 0 and 1, if stereo or wider.
    pub correlation: Option<f32>,
    pub timeline: Timeline,
}

/// Samples at or above this magnitude count as clipped.
pub const CLIP_THRESHOLD: f32 = 0.999;

/// How long a frame is for the percentile levels. Short enough that a pause
/// between words is several frames, long enough that one is not measuring a
/// single cycle of a low note.
pub const FRAME_MS: usize = 10;

/// How long a block is for the loudness meter and the timeline. Ten frames,
/// so the two grids line up and the block figures are an aggregate of the
/// frame ones rather than a second pass over the samples.
pub const BLOCK_MS: usize = 100;

/// The windows the loudness meter averages over, and so how much audio has to
/// have gone in before its answer means anything.
const MOMENTARY_MS: usize = 400;
const SHORT_TERM_MS: usize = 3000;

fn lin_db(x: f64) -> f32 {
    (20.0 * x.max(1e-10).log10()) as f32
}

/// The value `frac` of the way through a sorted slice, 0 being the smallest.
/// Nearest-rank, which for a few hundred frames is as meaningful as
/// interpolating between two of them and easier to reason about.
fn percentile(sorted: &[f32], frac: f64) -> f32 {
    if sorted.is_empty() {
        return f32::NEG_INFINITY;
    }
    let i = ((sorted.len() - 1) as f64 * frac).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// Measure the slices given, at the rate given.
pub fn compute_stats(channels: &[&[f32]], sample_rate: u32) -> Result<FileStats> {
    let nch = channels.len();
    let frames = channels.first().map_or(0, |c| c.len());
    let mut stats = FileStats {
        integrated_lufs: f32::NEG_INFINITY,
        loudness_range_lu: 0.0,
        max_momentary_lufs: f32::NEG_INFINITY,
        max_short_term_lufs: f32::NEG_INFINITY,
        channels: vec![ChannelStats::default(); nch],
        correlation: None,
        timeline: Timeline {
            block_secs: BLOCK_MS as f32 / 1000.0,
            channels: vec![ChannelTimeline::default(); nch],
            ..Default::default()
        },
    };
    if nch == 0 || frames == 0 {
        return Ok(stats);
    }

    // Per-channel scalar stats.
    for (c, ch) in channels.iter().enumerate() {
        let mut peak = 0.0f32;
        let mut sum = 0.0f64;
        let mut sq = 0.0f64;
        let mut clipped = 0usize;
        let mut runs = 0usize;
        let mut in_run = false;
        for &s in ch.iter() {
            let a = s.abs();
            peak = peak.max(a);
            sum += s as f64;
            sq += (s as f64) * (s as f64);
            if a >= CLIP_THRESHOLD {
                clipped += 1;
                if !in_run {
                    runs += 1;
                    in_run = true;
                }
            } else {
                in_run = false;
            }
        }
        let cs = &mut stats.channels[c];
        cs.sample_peak_db = lin_db(peak as f64);
        cs.clipped_samples = clipped;
        cs.clipped_runs = runs;
        cs.dc_offset = (sum / frames as f64) as f32;
        cs.rms_db = lin_db((sq / frames as f64).sqrt());
    }

    // Framed levels, and the block aggregate the timeline is built from. One
    // walk per channel; the block figures come from the frames rather than
    // from a second pass, which is why the two grids share a length.
    let frame_len = (sample_rate as usize * FRAME_MS / 1000).max(1);
    let per_block = BLOCK_MS / FRAME_MS;
    for (c, ch) in channels.iter().enumerate() {
        let mut frame_db: Vec<f32> = Vec::with_capacity(ch.len() / frame_len + 1);
        let mut zero_frames = 0usize;
        let (mut block_sq, mut block_peak, mut block_n) = (0.0f64, 0.0f32, 0usize);
        for frame in ch.chunks(frame_len) {
            let mut sq = 0.0f64;
            let mut peak = 0.0f32;
            for &v in frame {
                sq += (v as f64) * (v as f64);
                peak = peak.max(v.abs());
            }
            if peak == 0.0 {
                zero_frames += 1;
            } else {
                frame_db.push(lin_db((sq / frame.len() as f64).sqrt()));
            }
            block_sq += sq;
            block_peak = block_peak.max(peak);
            block_n += frame.len();
            if block_n >= frame_len * per_block {
                let t = &mut stats.timeline.channels[c];
                t.rms_db.push(lin_db((block_sq / block_n as f64).sqrt()));
                t.peak_db.push(lin_db(block_peak as f64));
                block_sq = 0.0;
                block_peak = 0.0;
                block_n = 0;
            }
        }
        if block_n > 0 {
            let t = &mut stats.timeline.channels[c];
            t.rms_db.push(lin_db((block_sq / block_n as f64).sqrt()));
            t.peak_db.push(lin_db(block_peak as f64));
        }
        frame_db.sort_by(f32::total_cmp);
        let cs = &mut stats.channels[c];
        // A level "exceeded 10% of the time" is the 90th percentile of the
        // values, which is the reading that trips people up here.
        cs.l10_db = percentile(&frame_db, 0.90);
        cs.l50_db = percentile(&frame_db, 0.50);
        cs.l90_db = percentile(&frame_db, 0.10);
        cs.noise_floor_db = percentile(&frame_db, 0.10);
        cs.level_frames = frame_db.len();
        cs.zero_frames = zero_frames;
    }

    if nch >= 2 {
        let a = channels[0];
        let b = channels[1];
        let ma = stats.channels[0].dc_offset as f64;
        let mb = stats.channels[1].dc_offset as f64;
        let (mut sab, mut saa, mut sbb) = (0.0f64, 0.0f64, 0.0f64);
        for (x, y) in a.iter().zip(b) {
            let dx = *x as f64 - ma;
            let dy = *y as f64 - mb;
            sab += dx * dy;
            saa += dx * dx;
            sbb += dy * dy;
        }
        let denom = (saa * sbb).sqrt();
        stats.correlation = Some(if denom > 0.0 {
            (sab / denom) as f32
        } else {
            0.0
        });
    }

    // Loudness in 100 ms blocks so momentary/short-term maxima can be tracked.
    let mut meter = EbuR128::new(
        nch as u32,
        sample_rate,
        Mode::I | Mode::LRA | Mode::S | Mode::M | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK,
    )?;
    let block = (sample_rate as usize / 10).max(1);
    let mut planes: Vec<&[f32]> = Vec::with_capacity(nch);
    let mut pos = 0;
    while pos < frames {
        let end = (pos + block).min(frames);
        planes.clear();
        for ch in channels {
            planes.push(&ch[pos..end]);
        }
        meter.add_frames_planar_f32(&planes)?;
        pos = end;
        // How much audio the meter has had, which is what decides whether its
        // answer is over a full window or over a window it has padded out.
        let heard_ms = pos * 1000 / sample_rate.max(1) as usize;
        let ask = |v: Result<f64, ebur128::Error>, window| {
            v.ok()
                .filter(|v| v.is_finite() && heard_ms >= window)
                .map(|v| v as f32)
        };
        let m = ask(meter.loudness_momentary(), MOMENTARY_MS);
        if let Some(m) = m {
            stats.max_momentary_lufs = stats.max_momentary_lufs.max(m);
        }
        let st = ask(meter.loudness_shortterm(), SHORT_TERM_MS);
        if let Some(s) = st {
            stats.max_short_term_lufs = stats.max_short_term_lufs.max(s);
        }
        stats.timeline.momentary_lufs.push(m);
        stats.timeline.short_term_lufs.push(st);
    }
    stats.integrated_lufs = meter
        .loudness_global()
        .map(|v| v as f32)
        .unwrap_or(f32::NEG_INFINITY);
    stats.loudness_range_lu = meter.loudness_range().map(|v| v as f32).unwrap_or(0.0);
    for (c, cs) in stats.channels.iter_mut().enumerate() {
        cs.true_peak_dbtp = meter
            .true_peak(c as u32)
            .map(lin_db)
            .unwrap_or(cs.sample_peak_db);
    }
    Ok(stats)
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
    fn ebu_r128_reference_tone_reads_minus_23_lufs() {
        // EBU Tech 3341 case 1: 1 kHz stereo sine at -23 dBFS -> -23.0 LUFS ±0.1.
        let sr = 48_000;
        let amp = 10f32.powf(-23.0 / 20.0);
        let ch = sine(1000.0, sr, sr as usize * 20, amp);
        let s = compute_stats(&[&ch[..], &ch[..]], sr).unwrap();
        assert!(
            (s.integrated_lufs + 23.0).abs() < 0.1,
            "integrated {}",
            s.integrated_lufs
        );
        assert!((s.max_momentary_lufs + 23.0).abs() < 0.2);
        assert!((s.max_short_term_lufs + 23.0).abs() < 0.2);
        assert!((s.channels[0].sample_peak_db + 23.0).abs() < 0.05);
        assert!((s.channels[0].true_peak_dbtp + 23.0).abs() < 0.2);
        assert!(s.correlation.unwrap() > 0.999);
        assert_eq!(s.channels[0].clipped_samples, 0);
    }

    /// A tone at a known level reads back at that level on every percentile:
    /// nothing varies, so the spread is zero and the floor is the tone.
    #[test]
    fn percentiles_of_a_steady_tone_all_agree() {
        let sr = 48_000;
        let amp = 10f32.powf(-20.0 / 20.0);
        let ch = sine(1000.0, sr, sr as usize, amp);
        let s = compute_stats(&[&ch[..]], sr).unwrap();
        let c = &s.channels[0];
        // A sine's RMS is 3.01 dB below its peak.
        for v in [c.l10_db, c.l50_db, c.l90_db, c.noise_floor_db] {
            assert!((v + 23.01).abs() < 0.2, "percentile {v}");
        }
        assert_eq!(c.zero_frames, 0);
        assert_eq!(c.level_frames, 100);
    }

    /// Loud speech over a quiet floor: the percentiles have to separate the
    /// two, which is the whole reason for having three of them.
    #[test]
    fn percentiles_separate_a_signal_from_its_floor() {
        let sr = 48_000;
        let loud = 10f32.powf(-10.0 / 20.0);
        let quiet = 10f32.powf(-60.0 / 20.0);
        // A quarter loud, three quarters floor.
        let mut ch = sine(1000.0, sr, sr as usize, quiet);
        for (i, v) in ch.iter_mut().enumerate() {
            if i < sr as usize / 4 {
                *v = loud * (std::f32::consts::TAU * 1000.0 * i as f32 / sr as f32).sin();
            }
        }
        let c = &compute_stats(&[&ch[..]], sr).unwrap().channels[0];
        assert!((c.l10_db + 13.0).abs() < 0.5, "l10 {}", c.l10_db);
        assert!((c.l90_db + 63.0).abs() < 0.5, "l90 {}", c.l90_db);
        assert!(
            (c.noise_floor_db + 63.0).abs() < 0.5,
            "the floor is the quiet part, not the average: {}",
            c.noise_floor_db
        );
    }

    /// Digital silence is not a level. A padded file must report the floor of
    /// the audio in it, not minus infinity, or every padded take reads the
    /// same and the measurement is useless.
    #[test]
    fn zero_padding_does_not_become_the_noise_floor() {
        let sr = 48_000;
        let quiet = 10f32.powf(-55.0 / 20.0);
        let mut ch = vec![0.0f32; sr as usize / 2];
        ch.extend(sine(1000.0, sr, sr as usize / 2, quiet));
        let c = &compute_stats(&[&ch[..]], sr).unwrap().channels[0];
        assert_eq!(c.zero_frames, 50, "half the frames are pure zero");
        assert_eq!(c.level_frames, 50);
        assert!(
            (c.noise_floor_db + 58.0).abs() < 0.5,
            "floor {} should be the tone, not silence",
            c.noise_floor_db
        );
    }

    /// The timeline is the same measurement against time: one entry per block,
    /// the loudness series starting null until the meter's window has filled.
    #[test]
    fn the_timeline_lines_up_block_for_block() {
        let sr = 48_000;
        let amp = 10f32.powf(-23.0 / 20.0);
        let ch = sine(1000.0, sr, sr as usize * 5, amp);
        let s = compute_stats(&[&ch[..], &ch[..]], sr).unwrap();
        let t = &s.timeline;
        assert_eq!(t.block_secs, 0.1);
        assert_eq!(t.momentary_lufs.len(), 50, "five seconds of 100 ms blocks");
        assert_eq!(t.short_term_lufs.len(), 50);
        assert_eq!(t.channels[0].rms_db.len(), 50);
        assert_eq!(t.channels[0].peak_db.len(), 50);

        // Momentary needs 400 ms and short-term 3 s. The meter answers sooner
        // than that, but over a window it has padded out, so those blocks are
        // withheld: three of them and twenty-nine.
        assert!(t.momentary_lufs[..3].iter().all(Option::is_none));
        assert!(t.momentary_lufs[3].is_some());
        assert!(t.short_term_lufs[..29].iter().all(Option::is_none));
        assert!(t.short_term_lufs[29].is_some());
        let last = t.momentary_lufs[49].expect("momentary by five seconds");
        assert!((last + 23.0).abs() < 0.3, "momentary {last}");
        let st = t.short_term_lufs[49].expect("short-term by five seconds");
        assert!((st + 23.0).abs() < 0.3, "short-term {st}");
        let rms = t.channels[0].rms_db[25];
        assert!((rms + 26.01).abs() < 0.2, "block rms {rms}");
    }

    /// Measuring a slice measures the slice. This is what makes
    /// `analyze --start --end` mean anything.
    #[test]
    fn a_region_is_measured_not_the_whole() {
        let sr = 48_000;
        let loud = 10f32.powf(-6.0 / 20.0);
        let quiet = 10f32.powf(-60.0 / 20.0);
        let mut ch = sine(1000.0, sr, sr as usize, loud);
        ch.extend(sine(1000.0, sr, sr as usize, quiet));

        let whole = compute_stats(&[&ch[..]], sr).unwrap();
        let tail = compute_stats(&[&ch[sr as usize..]], sr).unwrap();
        assert!((whole.channels[0].sample_peak_db + 6.0).abs() < 0.1);
        assert!(
            (tail.channels[0].sample_peak_db + 60.0).abs() < 0.1,
            "the second half peaks at -60, not at the file's -6: {}",
            tail.channels[0].sample_peak_db
        );
        assert!(tail.integrated_lufs < whole.integrated_lufs - 40.0);
    }

    #[test]
    fn clipping_dc_and_anticorrelation_are_detected() {
        let sr = 8000;
        let mut a = sine(100.0, sr, 8000, 1.2); // clips
        for s in &mut a {
            *s = s.clamp(-1.0, 1.0);
        }
        let b: Vec<f32> = a.iter().map(|s| -s * 0.5 + 0.1).collect(); // inverted, DC
        let s = compute_stats(&[&a[..], &b[..]], sr).unwrap();
        assert!(s.channels[0].clipped_samples > 0);
        assert!(s.channels[0].clipped_runs > 100);
        assert_eq!(s.channels[1].clipped_samples, 0);
        assert!((s.channels[1].dc_offset - 0.1).abs() < 0.01);
        assert!(s.correlation.unwrap() < -0.999);
    }
}
