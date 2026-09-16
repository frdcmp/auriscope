//! Loudness, peaks, clipping, DC offset and stereo correlation. One pass.

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
}

/// Samples at or above this magnitude count as clipped.
pub const CLIP_THRESHOLD: f32 = 0.999;

fn lin_db(x: f64) -> f32 {
    (20.0 * x.max(1e-10).log10()) as f32
}

pub fn compute_stats(channels: &[Vec<f32>], sample_rate: u32) -> Result<FileStats> {
    let nch = channels.len();
    let frames = channels.first().map_or(0, Vec::len);
    let mut stats = FileStats {
        integrated_lufs: f32::NEG_INFINITY,
        loudness_range_lu: 0.0,
        max_momentary_lufs: f32::NEG_INFINITY,
        max_short_term_lufs: f32::NEG_INFINITY,
        channels: vec![ChannelStats::default(); nch],
        correlation: None,
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
        for &s in ch {
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

    if nch >= 2 {
        let a = &channels[0];
        let b = &channels[1];
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
        if let Ok(m) = meter.loudness_momentary()
            && m.is_finite()
        {
            stats.max_momentary_lufs = stats.max_momentary_lufs.max(m as f32);
        }
        if let Ok(s) = meter.loudness_shortterm()
            && s.is_finite()
        {
            stats.max_short_term_lufs = stats.max_short_term_lufs.max(s as f32);
        }
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
        let s = compute_stats(&[ch.clone(), ch], sr).unwrap();
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

    #[test]
    fn clipping_dc_and_anticorrelation_are_detected() {
        let sr = 8000;
        let mut a = sine(100.0, sr, 8000, 1.2); // clips
        for s in &mut a {
            *s = s.clamp(-1.0, 1.0);
        }
        let b: Vec<f32> = a.iter().map(|s| -s * 0.5 + 0.1).collect(); // inverted, DC
        let s = compute_stats(&[a, b], sr).unwrap();
        assert!(s.channels[0].clipped_samples > 0);
        assert!(s.channels[0].clipped_runs > 100);
        assert_eq!(s.channels[1].clipped_samples, 0);
        assert!((s.channels[1].dc_offset - 0.1).abs() < 0.01);
        assert!(s.correlation.unwrap() < -0.999);
    }
}
