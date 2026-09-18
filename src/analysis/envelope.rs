//! Framed levels, and the filtered copies that decisions are made on.
//!
//! Every structural measurement downstream — where speech is, how loud a pause
//! is, whether a floor steps at a join — starts by turning samples into a much
//! shorter sequence of frame levels. Doing that once, here, is what keeps the
//! rest of the analysis reading like arithmetic instead of like signal
//! processing.
//!
//! Two rules from the delivery work this was built against, both of which cost
//! nothing and both of which change the answer:
//!
//! **Band-limit anything that makes a decision.** Low-frequency rumble is most
//! of the floor on some voices, so a broadband threshold finds speech where
//! there is only room. Decisions run on a 300–8000 Hz copy; the numbers
//! *reported* are the full-band ones, because that is what a person reads on a
//! meter.
//!
//! **Percentiles, not extremes.** A floor is the quiet tenth of the frames, not
//! the minimum, which is one unlucky sample.

/// A second-order section, and the only filter shape needed here.
///
/// Applied forwards and then backwards, which cancels the phase shift — a
/// filter that moved an edge in time would move every onset this module is
/// asked to locate — and squares the magnitude response, so a two-pole design
/// behaves like a four-pole one.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// Butterworth-ish Q. One section at 0.707 is maximally flat; run zero
    /// phase it is a little steeper than that and still has no ripple.
    const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

    pub fn low_pass(sample_rate: u32, hz: f32) -> Self {
        let (alpha, cos) = Self::terms(sample_rate, hz);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    pub fn high_pass(sample_rate: u32, hz: f32) -> Self {
        let (alpha, cos) = Self::terms(sample_rate, hz);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + cos) / 2.0 / a0,
            b1: -(1.0 + cos) / a0,
            b2: (1.0 + cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// The two quantities both shapes are built from.
    fn terms(sample_rate: u32, hz: f32) -> (f32, f32) {
        // Clamped below Nyquist: a corner at or above it has no meaning and
        // the tangent in the design formula runs away.
        let nyq = sample_rate as f32 / 2.0;
        let hz = hz.clamp(1.0, nyq * 0.99);
        let w0 = std::f32::consts::TAU * hz / sample_rate as f32;
        (w0.sin() / (2.0 * Self::Q), w0.cos())
    }

    fn run(&self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(input.len());
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for &x in input {
            let y = self.b0 * x + self.b1 * x1 + self.b2 * x2 - self.a1 * y1 - self.a2 * y2;
            x2 = x1;
            x1 = x;
            y2 = y1;
            y1 = y;
            out.push(y);
        }
    }

    /// Forward, then backward: no phase shift, twice the slope.
    pub fn apply(&self, input: &[f32]) -> Vec<f32> {
        let mut forward = Vec::new();
        self.run(input, &mut forward);
        forward.reverse();
        let mut back = Vec::new();
        self.run(&forward, &mut back);
        back.reverse();
        back
    }
}

/// Keep only what is between `lo` and `hi`.
pub fn band_limit(samples: &[f32], sample_rate: u32, lo_hz: f32, hi_hz: f32) -> Vec<f32> {
    let high = Biquad::high_pass(sample_rate, lo_hz).apply(samples);
    Biquad::low_pass(sample_rate, hi_hz).apply(&high)
}

/// Drop everything below `hz`.
pub fn high_pass(samples: &[f32], sample_rate: u32, hz: f32) -> Vec<f32> {
    Biquad::high_pass(sample_rate, hz).apply(samples)
}

/// The band speech decisions are made in. Below it is rumble and mains; above
/// it is mostly sibilance and hiss, neither of which reliably marks a word.
pub const SPEECH_BAND: (f32, f32) = (300.0, 8000.0);

/// Frame levels over a signal: one RMS and one peak per hop.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub sample_rate: u32,
    pub frame_len: usize,
    pub hop_len: usize,
    /// RMS of each frame, in dB. `-inf` for a frame of digital silence.
    pub rms_db: Vec<f32>,
    /// Largest absolute sample in each frame, linear.
    pub peak: Vec<f32>,
}

impl Envelope {
    /// Frame the signal. `frame_ms` is the window each level is measured over,
    /// `hop_ms` how far apart those windows start; a hop shorter than the frame
    /// overlaps them, which is what puts an onset within a hop of its true
    /// position instead of within a frame.
    pub fn compute(samples: &[f32], sample_rate: u32, frame_ms: usize, hop_ms: usize) -> Self {
        let frame_len = (sample_rate as usize * frame_ms / 1000).max(1);
        let hop_len = (sample_rate as usize * hop_ms / 1000).max(1);
        let mut rms_db = Vec::new();
        let mut peak = Vec::new();
        if !samples.is_empty() {
            let mut start = 0usize;
            while start < samples.len() {
                let end = (start + frame_len).min(samples.len());
                let frame = &samples[start..end];
                let mut sq = 0.0f64;
                let mut p = 0.0f32;
                for &v in frame {
                    sq += (v as f64) * (v as f64);
                    p = p.max(v.abs());
                }
                rms_db.push(db((sq / frame.len() as f64).sqrt()));
                peak.push(p);
                start += hop_len;
            }
        }
        Self {
            sample_rate,
            frame_len,
            hop_len,
            rms_db,
            peak,
        }
    }

    pub fn len(&self) -> usize {
        self.rms_db.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rms_db.is_empty()
    }

    /// When frame `i` starts, in seconds.
    pub fn time_of(&self, i: usize) -> f64 {
        (i * self.hop_len) as f64 / self.sample_rate as f64
    }

    /// The frame covering `secs`.
    pub fn frame_at(&self, secs: f64) -> usize {
        ((secs * self.sample_rate as f64) as usize / self.hop_len).min(self.len().saturating_sub(1))
    }

    /// The level `frac` of the way up the sorted frame levels, ignoring frames
    /// of digital silence.
    ///
    /// Zero frames are left out on purpose: a file padded with silence would
    /// otherwise report a floor of −infinity and every padded file would look
    /// alike.
    pub fn percentile(&self, frac: f64) -> f32 {
        let mut v: Vec<f32> = self
            .rms_db
            .iter()
            .copied()
            .filter(|d| d.is_finite())
            .collect();
        if v.is_empty() {
            return f32::NEG_INFINITY;
        }
        v.sort_by(f32::total_cmp);
        let i = (((v.len() - 1) as f64) * frac).round() as usize;
        v[i.min(v.len() - 1)]
    }

    /// The quiet tenth: what a noise floor is when nobody has said where the
    /// silence is.
    pub fn floor_db(&self) -> f32 {
        self.percentile(0.10)
    }
}

/// Decibels from a linear amplitude, with digital silence kept as `-inf` so a
/// caller can tell it from a very quiet sound.
fn db(x: f64) -> f32 {
    if x <= 0.0 {
        return f32::NEG_INFINITY;
    }
    (20.0 * x.log10()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sr: u32, n: usize, amp: f32) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        let sq: f64 = x.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        db((sq / x.len() as f64).sqrt())
    }

    /// A filter has to pass what it is meant to and stop what it is not, and a
    /// zero-phase one must not shift the signal while doing it.
    #[test]
    fn the_band_keeps_the_middle_and_drops_the_ends() {
        let sr = 48_000;
        let n = sr as usize;
        let inside = band_limit(&sine(1000.0, sr, n, 0.5), sr, 300.0, 8000.0);
        let below = band_limit(&sine(50.0, sr, n, 0.5), sr, 300.0, 8000.0);
        let above = band_limit(&sine(16000.0, sr, n, 0.5), sr, 300.0, 8000.0);

        let mid = &inside[sr as usize / 4..inside.len() - sr as usize / 4];
        assert!(
            (rms_db(mid) + 9.03).abs() < 1.0,
            "1 kHz should pass at about its own level, got {}",
            rms_db(mid)
        );
        assert!(
            rms_db(&below[sr as usize / 4..]) < -40.0,
            "50 Hz should be well down, got {}",
            rms_db(&below[sr as usize / 4..])
        );
        assert!(
            rms_db(&above[sr as usize / 4..]) < -40.0,
            "16 kHz should be well down, got {}",
            rms_db(&above[sr as usize / 4..])
        );
    }

    /// Zero phase means an impulse comes back centred where it went in. This is
    /// what lets a filtered copy decide *where* something starts.
    #[test]
    fn filtering_does_not_move_anything_in_time() {
        let sr = 48_000;
        let mut x = vec![0.0f32; 4096];
        x[2048] = 1.0;
        let y = band_limit(&x, sr, 300.0, 8000.0);
        let peak = y
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, _)| i)
            .unwrap();
        assert!(
            peak.abs_diff(2048) <= 1,
            "the impulse should stay at 2048, landed at {peak}"
        );
    }

    #[test]
    fn frames_land_where_the_hop_puts_them() {
        let sr = 48_000;
        let e = Envelope::compute(&sine(1000.0, sr, sr as usize, 0.5), sr, 20, 5);
        assert_eq!(e.frame_len, 960);
        assert_eq!(e.hop_len, 240);
        assert_eq!(e.len(), 200, "one second at a 5 ms hop");
        assert!((e.time_of(100) - 0.5).abs() < 1e-9);
        assert_eq!(e.frame_at(0.5), 100);
        assert!((e.rms_db[50] + 9.03).abs() < 0.2);
    }

    /// The floor is the quiet part, and silence is not a level.
    #[test]
    fn the_floor_ignores_digital_silence() {
        let sr = 48_000;
        let mut x = vec![0.0f32; sr as usize / 2];
        x.extend(sine(1000.0, sr, sr as usize / 2, 10f32.powf(-50.0 / 20.0)));
        let e = Envelope::compute(&x, sr, 20, 10);
        assert!(
            (e.floor_db() + 53.0).abs() < 1.0,
            "floor should be the tone at -53 dB RMS, got {}",
            e.floor_db()
        );
    }

    #[test]
    fn an_empty_signal_measures_nothing_rather_than_panicking() {
        let e = Envelope::compute(&[], 48_000, 20, 5);
        assert!(e.is_empty());
        assert_eq!(e.floor_db(), f32::NEG_INFINITY);
        assert_eq!(e.frame_at(1.0), 0);
    }
}
