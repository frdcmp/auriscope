//! Offline spectrogram: STFT over a whole channel, stored as 8-bit dB and
//! max-pooled into coarser column levels so zooming out never scans every
//! column. Rendering samples that store into a viewport-sized image.

use egui::{Color32, ColorImage};
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use serde::{Deserialize, Serialize};

use super::window::WindowKind;

/// Lowest level stored. Anything quieter clamps to 0.
pub const DB_FLOOR: f32 = -120.0;
const LEVEL_FACTOR: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StftParams {
    pub window_size: usize,
    /// Overlap as a fraction of the window: 0.5 = 50%, 0.75 = 75%.
    pub overlap_num: u8,
    pub overlap_den: u8,
    pub window: WindowKind,
}

impl Default for StftParams {
    fn default() -> Self {
        Self {
            window_size: 2048,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
        }
    }
}

impl StftParams {
    pub const SIZES: [usize; 7] = [256, 512, 1024, 2048, 4096, 8192, 16384];

    pub fn hop(&self) -> usize {
        let keep = self.window_size * self.overlap_num as usize / self.overlap_den as usize;
        (self.window_size - keep).max(1)
    }

    pub fn overlap_label(&self) -> String {
        format!(
            "{}%",
            100 * self.overlap_num as u32 / self.overlap_den as u32
        )
    }
}

struct Level {
    /// Frames of audio per column at this level.
    columns: usize,
    data: Vec<u8>, // column-major: data[col * bins + bin]
}

pub struct Spectrogram {
    pub params: StftParams,
    pub sample_rate: u32,
    pub bins: usize,
    pub frames: usize,
    levels: Vec<Level>,
}

impl Spectrogram {
    /// Full STFT of `samples`. `progress` gets `0.0..=1.0`. Returns `None`
    /// if `cancel` becomes true mid-way.
    pub fn compute(
        samples: &[f32],
        sample_rate: u32,
        params: StftParams,
        progress: &dyn Fn(f32),
        cancel: &dyn Fn() -> bool,
    ) -> Option<Self> {
        let n = params.window_size;
        let hop = params.hop();
        let bins = n / 2 + 1;
        let window = params.window.make(n);
        let gain = 2.0 / params.window.coherent_gain(n);

        let columns = if samples.is_empty() {
            0
        } else {
            samples.len().div_ceil(hop)
        };

        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let mut input = fft.make_input_vec();
        let mut output = fft.make_output_vec();
        let mut scratch = fft.make_scratch_vec();

        let mut data = vec![0u8; columns * bins];
        let half = n / 2;
        let report_every = (columns / 200).max(1);

        for col in 0..columns {
            if col % report_every == 0 {
                if cancel() {
                    return None;
                }
                progress(col as f32 / columns as f32);
            }
            // Centre the window on the column's time position.
            let centre = col * hop;
            for (i, (inp, w)) in input.iter_mut().zip(&window).enumerate() {
                let idx = centre as isize + i as isize - half as isize;
                let s = if idx >= 0 && (idx as usize) < samples.len() {
                    samples[idx as usize]
                } else {
                    0.0
                };
                *inp = s * w;
            }
            fft.process_with_scratch(&mut input, &mut output, &mut scratch)
                .ok()?;
            let dst = &mut data[col * bins..(col + 1) * bins];
            for (d, c) in dst.iter_mut().zip(&output) {
                *d = db_to_u8(mag_db(*c, gain));
            }
        }
        progress(1.0);

        let mut levels = vec![Level { columns, data }];
        loop {
            let prev = levels.last().unwrap();
            if prev.columns <= 64 {
                break;
            }
            let columns = prev.columns.div_ceil(LEVEL_FACTOR);
            let mut data = vec![0u8; columns * bins];
            for c in 0..columns {
                let dst = &mut data[c * bins..(c + 1) * bins];
                let a = c * LEVEL_FACTOR;
                let b = ((c + 1) * LEVEL_FACTOR).min(prev.columns);
                for src_col in a..b {
                    let src = &prev.data[src_col * bins..(src_col + 1) * bins];
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d = (*d).max(*s);
                    }
                }
            }
            levels.push(Level { columns, data });
        }

        Some(Self {
            params,
            sample_rate,
            bins,
            frames: samples.len(),
            levels,
        })
    }

    pub fn columns(&self) -> usize {
        self.levels.first().map_or(0, |l| l.columns)
    }

    /// Frequency of bin `i` in Hz.
    pub fn bin_hz(&self, i: usize) -> f32 {
        i as f32 * self.sample_rate as f32 / self.params.window_size as f32
    }

    pub fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    /// Raw dB value at (column, bin) on the finest level.
    pub fn db_at(&self, column: usize, bin: usize) -> f32 {
        let l = &self.levels[0];
        if column >= l.columns || bin >= self.bins {
            return DB_FLOOR;
        }
        u8_to_db(l.data[column * self.bins + bin])
    }
}

#[inline]
fn mag_db(c: Complex<f32>, gain: f32) -> f32 {
    let m = c.norm() * gain;
    20.0 * m.max(1e-10).log10()
}

#[inline]
pub fn db_to_u8(db: f32) -> u8 {
    let t = (db - DB_FLOOR) / -DB_FLOOR; // 0 at floor, 1 at 0 dB
    (t.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[inline]
pub fn u8_to_db(v: u8) -> f32 {
    DB_FLOOR + (v as f32 / 255.0) * -DB_FLOOR
}

/// How to map the spectrogram into a viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewParams {
    /// Visible frame range.
    pub start_frame: f64,
    pub end_frame: f64,
    pub min_hz: f32,
    pub max_hz: f32,
    pub log_frequency: bool,
    /// dB mapped to the darkest colour.
    pub db_min: f32,
    /// dB mapped to the brightest colour.
    pub db_max: f32,
}

/// Render the visible region into an RGBA image of `width` x `height`.
/// Row 0 is the top of the image (highest frequency).
pub fn render_view(
    spec: &Spectrogram,
    view: &ViewParams,
    width: usize,
    height: usize,
    lut: &[Color32],
) -> ColorImage {
    let mut img = ColorImage::filled([width.max(1), height.max(1)], lut[0]);
    if width == 0 || height == 0 || spec.columns() == 0 || view.end_frame <= view.start_frame {
        return img;
    }
    let hop = spec.params.hop() as f64;
    let frames_per_px = (view.end_frame - view.start_frame) / width as f64;
    let cols_per_px = frames_per_px / hop;

    // Pick the coarsest level that still has >= 1 column per pixel.
    let mut level_idx = 0;
    let mut level_cols = 1.0;
    for (i, _) in spec.levels.iter().enumerate() {
        let factor = LEVEL_FACTOR.pow(i as u32) as f64;
        if cols_per_px >= factor {
            level_idx = i;
            level_cols = factor;
        } else {
            break;
        }
    }
    let level = &spec.levels[level_idx];
    let bins = spec.bins;
    let frames_per_col = hop * level_cols;

    // Per-row bin ranges (top row = max_hz).
    let nyq = spec.nyquist();
    let min_hz = view.min_hz.max(if view.log_frequency { 10.0 } else { 0.0 });
    let max_hz = view.max_hz.min(nyq).max(min_hz + 1.0);
    let hz_per_bin = spec.bin_hz(1).max(1e-6);
    let row_bins: Vec<(usize, usize)> = (0..height)
        .map(|row| {
            let t_top = 1.0 - row as f32 / height as f32;
            let t_bot = 1.0 - (row + 1) as f32 / height as f32;
            let (hz_top, hz_bot) = if view.log_frequency {
                let lmin = min_hz.ln();
                let lmax = max_hz.ln();
                (
                    (lmin + (lmax - lmin) * t_top).exp(),
                    (lmin + (lmax - lmin) * t_bot).exp(),
                )
            } else {
                (
                    min_hz + (max_hz - min_hz) * t_top,
                    min_hz + (max_hz - min_hz) * t_bot,
                )
            };
            let b0 = (hz_bot / hz_per_bin).floor().max(0.0) as usize;
            let b1 = ((hz_top / hz_per_bin).ceil() as usize)
                .max(b0 + 1)
                .min(bins);
            (b0.min(bins - 1), b1)
        })
        .collect();

    let db_range = (view.db_max - view.db_min).max(1e-3);
    let pixels = img.pixels.as_mut_slice();

    for x in 0..width {
        let f0 = view.start_frame + frames_per_px * x as f64;
        let f1 = f0 + frames_per_px;
        let c0 = (f0 / frames_per_col).floor().max(0.0) as usize;
        let c1 = ((f1 / frames_per_col).ceil().max(0.0) as usize).max(c0 + 1);
        if c0 >= level.columns {
            continue;
        }
        let c1 = c1.min(level.columns);
        for (row, &(b0, b1)) in row_bins.iter().enumerate() {
            let mut v = 0u8;
            for c in c0..c1 {
                let col = &level.data[c * bins..(c + 1) * bins];
                for &s in &col[b0..b1] {
                    v = v.max(s);
                }
            }
            let db = u8_to_db(v);
            let t = ((db - view.db_min) / db_range).clamp(0.0, 1.0);
            let idx = (t * (lut.len() - 1) as f32).round() as usize;
            pixels[row * width + x] = lut[idx];
        }
    }
    img
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
    fn sine_lands_in_expected_bin_at_expected_level() {
        let sr = 48_000;
        for kind in WindowKind::ALL {
            let params = StftParams {
                window_size: 2048,
                overlap_num: 1,
                overlap_den: 2,
                window: kind,
            };
            // Bin 100 exactly: 100 * 48000 / 2048 Hz.
            let freq = 100.0 * sr as f32 / 2048.0;
            let sig = sine(freq, sr, 48_000, 1.0);
            let spec = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
            let col = spec.columns() / 2;
            let (peak_bin, _) = (0..spec.bins)
                .map(|b| (b, spec.db_at(col, b)))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert_eq!(peak_bin, 100, "{kind:?}");
            let db = spec.db_at(col, 100);
            assert!(db.abs() < 1.0, "{kind:?}: full-scale sine read {db} dB");
            // Far away from the peak the leakage must be well down.
            let far = spec.db_at(col, 600);
            assert!(far < -40.0, "{kind:?}: leakage {far} dB");
        }
    }

    #[test]
    fn coarse_levels_are_max_pools() {
        let sr = 8000;
        let sig = sine(1000.0, sr, 8000 * 4, 0.5);
        let params = StftParams {
            window_size: 256,
            overlap_num: 1,
            overlap_den: 2,
            window: WindowKind::Hann,
        };
        let spec = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
        assert!(spec.levels.len() >= 2);
        let l0 = &spec.levels[0];
        let l1 = &spec.levels[1];
        for c in 0..l1.columns {
            for b in 0..spec.bins {
                let mut m = 0u8;
                for sc in c * LEVEL_FACTOR..((c + 1) * LEVEL_FACTOR).min(l0.columns) {
                    m = m.max(l0.data[sc * spec.bins + b]);
                }
                assert_eq!(l1.data[c * spec.bins + b], m);
            }
        }
    }

    #[test]
    fn render_produces_requested_size_and_is_bright_at_the_tone() {
        let sr = 8000;
        let sig = sine(1000.0, sr, 16_000, 1.0);
        let spec =
            Spectrogram::compute(&sig, sr, StftParams::default(), &|_| {}, &|| false).unwrap();
        let view = ViewParams {
            start_frame: 0.0,
            end_frame: 16_000.0,
            min_hz: 0.0,
            max_hz: 4000.0,
            log_frequency: false,
            db_min: -90.0,
            db_max: 0.0,
        };
        let lut = super::super::ColorMap::Grey.lut();
        let img = render_view(&spec, &view, 64, 40, &lut);
        assert_eq!(img.size, [64, 40]);
        // 1 kHz of 4 kHz linear from the bottom = row 40 * (1 - 0.25) = 30.
        let bright = img.pixels[30 * 64 + 32].r();
        let dark = img.pixels[5 * 64 + 32].r();
        assert!(bright > 200, "tone row {bright}");
        assert!(dark < 60, "quiet row {dark}");
    }

    #[test]
    fn cancel_stops_early() {
        let sig = vec![0.0f32; 1 << 20];
        let r = Spectrogram::compute(&sig, 48_000, StftParams::default(), &|_| {}, &|| true);
        assert!(r.is_none());
    }
}
