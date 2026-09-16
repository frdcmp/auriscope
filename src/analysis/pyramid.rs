//! Multi-resolution peak/RMS pyramid for drawing the waveform at any zoom.
//!
//! Level 0 buckets `BASE` samples; each further level buckets `FACTOR` times
//! more. Drawing a two-hour file zoomed out reads a few thousand bins, never
//! the samples.

/// Aggregate of a run of samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bin {
    pub min: f32,
    pub max: f32,
    /// Root-mean-square over the run.
    pub rms: f32,
}

impl Bin {
    pub const EMPTY: Bin = Bin {
        min: 0.0,
        max: 0.0,
        rms: 0.0,
    };

    fn from_samples(s: &[f32]) -> Bin {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        let mut sq = 0.0f64;
        for &v in s {
            min = min.min(v);
            max = max.max(v);
            sq += (v as f64) * (v as f64);
        }
        if s.is_empty() {
            return Bin::EMPTY;
        }
        Bin {
            min,
            max,
            rms: (sq / s.len() as f64).sqrt() as f32,
        }
    }

    /// Merge equally weighted bins. Exact for min/max; exact for RMS when the
    /// bins cover equal sample counts, which is how the pyramid is built.
    fn merge(bins: &[Bin]) -> Bin {
        if bins.is_empty() {
            return Bin::EMPTY;
        }
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        let mut sq = 0.0f64;
        for b in bins {
            min = min.min(b.min);
            max = max.max(b.max);
            sq += (b.rms as f64) * (b.rms as f64);
        }
        Bin {
            min,
            max,
            rms: (sq / bins.len() as f64).sqrt() as f32,
        }
    }
}

const BASE: usize = 64;
const FACTOR: usize = 4;

struct Level {
    bucket: usize,
    bins: Vec<Bin>,
}

struct ChannelPyramid {
    levels: Vec<Level>,
}

pub struct WaveformPyramid {
    channels: Vec<ChannelPyramid>,
    frames: usize,
}

impl WaveformPyramid {
    pub fn build(channels: &[Vec<f32>]) -> Self {
        let frames = channels.first().map_or(0, Vec::len);
        let channels = channels
            .iter()
            .map(|samples| {
                let mut levels = Vec::new();
                let l0: Vec<Bin> = samples.chunks(BASE).map(Bin::from_samples).collect();
                levels.push(Level {
                    bucket: BASE,
                    bins: l0,
                });
                loop {
                    let prev = levels.last().unwrap();
                    if prev.bins.len() <= 1 {
                        break;
                    }
                    let bucket = prev.bucket * FACTOR;
                    let bins: Vec<Bin> = prev.bins.chunks(FACTOR).map(Bin::merge).collect();
                    levels.push(Level { bucket, bins });
                }
                ChannelPyramid { levels }
            })
            .collect();
        Self { channels, frames }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// One `Bin` per output column covering frames `start..end` of `channel`.
    /// `samples` is consulted only when the view is zoomed in past level 0.
    pub fn query(
        &self,
        channel: usize,
        samples: &[f32],
        start: f64,
        end: f64,
        columns: usize,
    ) -> Vec<Bin> {
        let mut out = Vec::with_capacity(columns);
        if columns == 0 || end <= start {
            return out;
        }
        let per_col = (end - start) / columns as f64;
        let ch = &self.channels[channel];

        // Largest bucket that still gives at least ~2 bins per column.
        let level = ch
            .levels
            .iter()
            .rev()
            .find(|l| (l.bucket as f64) * 2.0 <= per_col);

        for c in 0..columns {
            let a = start + per_col * c as f64;
            let b = a + per_col;
            let a_i = a.floor().max(0.0) as usize;
            let b_i = (b.ceil().max(0.0) as usize).min(self.frames);
            if a_i >= b_i {
                out.push(Bin::EMPTY);
                continue;
            }
            match level {
                Some(l) => {
                    let ia = a_i / l.bucket;
                    let ib = b_i.div_ceil(l.bucket).min(l.bins.len());
                    out.push(Bin::merge(&l.bins[ia..ib.max(ia + 1).min(l.bins.len())]));
                }
                None => {
                    let end = b_i.min(samples.len());
                    if a_i < end {
                        out.push(Bin::from_samples(&samples[a_i..end]));
                    } else {
                        out.push(Bin::EMPTY);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_min_max_rms_exact_at_every_level() {
        // A ramp 0..n over exactly 64*4*4 samples: three levels.
        let n = BASE * FACTOR * FACTOR;
        let ramp: Vec<f32> = (0..n).map(|i| i as f32).collect();
        let p = WaveformPyramid::build(std::slice::from_ref(&ramp));
        let ch = &p.channels[0];
        assert_eq!(ch.levels.len(), 3);
        for level in &ch.levels {
            for (i, bin) in level.bins.iter().enumerate() {
                let s = i * level.bucket;
                let e = (s + level.bucket).min(n);
                let slice = &ramp[s..e];
                let expect = Bin::from_samples(slice);
                assert_eq!(bin.min, expect.min);
                assert_eq!(bin.max, expect.max);
                assert!((bin.rms - expect.rms).abs() < expect.rms * 1e-5);
            }
        }
    }

    #[test]
    fn query_zoomed_out_matches_direct_scan() {
        let n = 100_000;
        let sig: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.01).sin()).collect();
        let p = WaveformPyramid::build(std::slice::from_ref(&sig));
        let cols = 50;
        let bins = p.query(0, &sig, 0.0, n as f64, cols);
        assert_eq!(bins.len(), cols);
        let per = n / cols;
        for (c, bin) in bins.iter().enumerate() {
            let direct = Bin::from_samples(&sig[c * per..(c + 1) * per]);
            // Buckets do not align to column edges exactly, so min/max can
            // only be wider than the direct scan, never narrower.
            assert!(bin.min <= direct.min + 1e-6);
            assert!(bin.max >= direct.max - 1e-6);
            assert!((bin.rms - direct.rms).abs() < 0.05);
        }
    }

    #[test]
    fn query_zoomed_in_uses_samples() {
        let sig: Vec<f32> = (0..1000).map(|i| (i % 7) as f32).collect();
        let p = WaveformPyramid::build(std::slice::from_ref(&sig));
        let bins = p.query(0, &sig, 10.0, 20.0, 10);
        for (i, b) in bins.iter().enumerate() {
            let v = sig[10 + i];
            assert_eq!(b.min, v);
            assert_eq!(b.max, v);
        }
    }
}
