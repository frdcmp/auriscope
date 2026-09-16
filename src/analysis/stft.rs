//! Offline spectrogram: STFT over a whole channel, stored as 8-bit dB and
//! max-pooled into coarser column levels so zooming out never scans every
//! column. Rendering samples that store into a viewport-sized image.

use egui::{Color32, ColorImage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
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
    /// Time–frequency reassignment: move each bin's energy to its
    /// instantaneous frequency and group-delay-corrected time instead of
    /// leaving it smeared across the window's main lobe. Sharp lines for
    /// harmonics, sharp columns for clicks. Three FFTs per frame instead of
    /// one.
    #[serde(default)]
    pub reassign: bool,
}

impl Default for StftParams {
    fn default() -> Self {
        Self {
            window_size: 2048,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: false,
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
    /// Sub-bin position of each cell's energy, present only for reassigned
    /// spectrograms: 0 is half a bin below the cell centre, 255 half above.
    /// One extra byte per cell buys lines that glide instead of stepping.
    offs: Option<Vec<u8>>,
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
        let columns = if samples.is_empty() {
            0
        } else {
            samples.len().div_ceil(hop)
        };
        let mut an = Analyser::new(&params);
        let mut data = vec![0u8; columns * bins];
        let mut offs = params.reassign.then(|| vec![OFF_ZERO; columns * bins]);
        let report_every = (columns / 200).max(1);

        if let Some(offs) = offs.as_deref_mut() {
            let reach = (n / 2).div_ceil(hop) + 1;
            let mut grid = RingGrid::new(&mut data, offs, columns, bins, reach);
            let mut pre = PrepBuf::new(bins);
            for col in 0..columns {
                if col % report_every == 0 {
                    if cancel() {
                        return None;
                    }
                    progress(col as f32 / columns as f32);
                }
                an.analyse(samples, (col * hop) as f64)?;
                prepare_frame(
                    an.frame(),
                    hop as f32,
                    &mut pre.lvl,
                    &mut pre.dc,
                    &mut pre.bin,
                    &mut pre.off,
                );
                grid.advance(col);
                grid.scatter(pre.column(0, bins), col);
            }
            grid.finish();
        } else {
            for col in 0..columns {
                if col % report_every == 0 {
                    if cancel() {
                        return None;
                    }
                    progress(col as f32 / columns as f32);
                }
                an.analyse(samples, (col * hop) as f64)?;
                let dst = &mut data[col * bins..(col + 1) * bins];
                for (d, p) in dst.iter_mut().zip(&an.power) {
                    *d = power_to_u8(*p);
                }
            }
        }
        progress(1.0);

        let mut levels = vec![Level {
            columns,
            data,
            offs,
        }];
        loop {
            let next = {
                let prev = levels.last().unwrap();
                if prev.columns <= 64 {
                    break;
                }
                let columns = prev.columns.div_ceil(LEVEL_FACTOR);
                let mut data = vec![0u8; columns * bins];
                let mut offs = prev.offs.as_ref().map(|_| vec![OFF_ZERO; columns * bins]);
                for c in 0..columns {
                    let a = c * LEVEL_FACTOR;
                    let b = ((c + 1) * LEVEL_FACTOR).min(prev.columns);
                    for src_col in a..b {
                        let sb = src_col * bins;
                        let db = c * bins;
                        for k in 0..bins {
                            let v = prev.data[sb + k];
                            if v > data[db + k] {
                                data[db + k] = v;
                                if let (Some(o), Some(po)) = (offs.as_mut(), prev.offs.as_ref()) {
                                    o[db + k] = po[sb + k];
                                }
                            }
                        }
                    }
                }
                Level {
                    columns,
                    data,
                    offs,
                }
            };
            levels.push(next);
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

    /// Where the energy stored in (column, bin) really sits, in fractional
    /// bins. Equals `bin` unless the spectrogram was reassigned.
    pub fn pos_at(&self, column: usize, bin: usize) -> f32 {
        let l = &self.levels[0];
        match &l.offs {
            Some(o) if column < l.columns && bin < self.bins => {
                bin as f32 + u8_to_off(o[column * self.bins + bin])
            }
            _ => bin as f32,
        }
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

/// Linear power (full-scale sine = 1.0) to the stored 8-bit dB scale.
///
/// Uses a quadratic log2 approximation: it is evaluated once per cell for
/// millions of cells per tile, and its worst error of 0.017 dB is nearly
/// thirty times smaller than one 8-bit step.
#[inline]
fn power_to_u8(p: f32) -> u8 {
    const DB_PER_LOG2: f32 = 3.010_3; // 10·log10(2)
    db_to_u8(DB_PER_LOG2 * fast_log2(p.max(1e-20)))
}

#[inline]
fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000); // [1, 2)
    exp as f32 + (-0.339_751_8 * m + 2.009_364_8) * m - 1.664_231
}

/// Encoded sub-bin offset meaning "at the cell centre".
const OFF_ZERO: u8 = 128;

#[inline]
fn off_to_u8(o: f32) -> u8 {
    // `as u8` truncates and saturates; +0.5 on a non-negative value makes
    // that a round, without a libm call per cell.
    ((o.clamp(-0.5, 0.5) + 0.5) * 255.0 + 0.5) as u8
}

#[inline]
fn u8_to_off(v: u8) -> f32 {
    v as f32 / 255.0 - 0.5
}

/// One frame of analysis, with optional time–frequency reassignment.
///
/// Holds the FFT plan, the three windows and every buffer, so the per-frame
/// path allocates nothing. With `reassign` off it is an ordinary windowed
/// FFT. With it on, two extra transforms — against the time-weighted window
/// and the derivative window — give, per bin, the offset from the frame's
/// centre where the energy really sits and the fractional bin where its
/// frequency really is (Auger & Flandrin).
struct Analyser {
    n: usize,
    bins: usize,
    reassign: bool,
    /// Scale for squared magnitude so a full-scale sine reads 1.0.
    power_gain: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    win: Vec<f32>,
    win_t: Vec<f32>,
    win_d: Vec<f32>,
    frame: Vec<f32>,
    input: Vec<f32>,
    x_h: Vec<Complex<f32>>,
    x_t: Vec<Complex<f32>>,
    x_d: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Per bin, after `analyse`.
    power: Vec<f32>,
    /// Per bin: reassigned time offset from the frame centre, in samples.
    dt: Vec<f32>,
    /// Per bin: reassigned fractional bin index.
    kf: Vec<f32>,
}

impl Analyser {
    fn new(params: &StftParams) -> Self {
        let n = params.window_size;
        let bins = n / 2 + 1;
        let win = params.window.make(n);
        let gain = 2.0 / params.window.coherent_gain(n);
        let half = n as f32 / 2.0;
        let win_t: Vec<f32> = win
            .iter()
            .enumerate()
            .map(|(i, &w)| (i as f32 - half) * w)
            .collect();
        // Central difference of the (periodic) window: dh/dn.
        let win_d: Vec<f32> = (0..n)
            .map(|i| (win[(i + 1) % n] - win[(i + n - 1) % n]) * 0.5)
            .collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        Self {
            n,
            bins,
            reassign: params.reassign,
            power_gain: gain * gain,
            input: fft.make_input_vec(),
            x_h: fft.make_output_vec(),
            x_t: fft.make_output_vec(),
            x_d: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            win,
            win_t,
            win_d,
            frame: vec![0.0; n],
            power: vec![0.0; bins],
            dt: vec![0.0; bins],
            kf: (0..bins).map(|k| k as f32).collect(),
        }
    }

    /// Transform the frame centred on `centre` (a sample index; fractional
    /// values are rounded to the nearest sample). Samples outside the file
    /// read as silence.
    fn analyse(&mut self, samples: &[f32], centre: f64) -> Option<()> {
        let Self {
            n,
            bins,
            reassign,
            power_gain,
            fft,
            win,
            win_t,
            win_d,
            frame,
            input,
            x_h,
            x_t,
            x_d,
            scratch,
            power,
            dt,
            kf,
        } = self;
        let base = (centre - *n as f64 / 2.0).round() as isize;
        for (i, f) in frame.iter_mut().enumerate() {
            let idx = base + i as isize;
            *f = if idx >= 0 && (idx as usize) < samples.len() {
                samples[idx as usize]
            } else {
                0.0
            };
        }
        run_fft(fft.as_ref(), frame, win, input, x_h, scratch)?;
        for (p, c) in power.iter_mut().zip(x_h.iter()) {
            *p = c.norm_sqr() * *power_gain;
        }
        if !*reassign {
            return Some(());
        }
        run_fft(fft.as_ref(), frame, win_t, input, x_t, scratch)?;
        run_fft(fft.as_ref(), frame, win_d, input, x_d, scratch)?;
        let bins_per_rad = *n as f32 / std::f32::consts::TAU;
        let half = *n as f32 / 2.0;
        let last = (*bins - 1) as f32;
        for k in 0..*bins {
            let h = x_h[k];
            let den = h.norm_sqr();
            if den < 1e-20 {
                dt[k] = 0.0;
                kf[k] = k as f32;
                continue;
            }
            let hc = h.conj();
            // t̂ = t + Re{X_th / X_h};  ω̂ = ω − Im{X_dh / X_h}
            let t_off = (x_t[k] * hc).re / den;
            let w_off = (x_d[k] * hc).im / den;
            dt[k] = t_off.clamp(-half, half);
            kf[k] = (k as f32 - w_off * bins_per_rad).clamp(0.0, last);
        }
        Some(())
    }

    fn frame(&self) -> Frame<'_> {
        Frame {
            power: &self.power,
            dt: &self.dt,
            kf: &self.kf,
        }
    }
}

/// Worker threads for the parallel parts of the analysis and rendering.
/// `AURISCOPE_THREADS` overrides the detected count, for timing and for
/// checking that the parallel paths match the sequential ones.
fn worker_count() -> usize {
    if let Some(n) = std::env::var("AURISCOPE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return n.max(1);
    }
    std::thread::available_parallelism().map_or(1, |n| n.get().min(16))
}

fn run_fft(
    fft: &dyn RealToComplex<f32>,
    frame: &[f32],
    win: &[f32],
    input: &mut [f32],
    out: &mut [Complex<f32>],
    scratch: &mut [Complex<f32>],
) -> Option<()> {
    for ((i, f), w) in input.iter_mut().zip(frame).zip(win) {
        *i = f * w;
    }
    fft.process_with_scratch(input, out, scratch).ok()
}

/// Max-combining accumulator for reassigned energy.
///
/// Reassignment moves energy between neighbouring columns, so a column is
/// only final once every frame that can reach it has been scattered. The
/// grid keeps a ring of `2·reach + 2` columns of quantised levels and
/// releases them to the output in order. Everything stored is already
/// 8-bit: the arithmetic happens in [`prepare_frame`], which runs in
/// parallel, and this step is a compare-and-store per cell.
struct RingGrid<'a> {
    bins: usize,
    columns: usize,
    reach: usize,
    width: usize,
    ring_l: Vec<u8>,
    ring_o: Vec<u8>,
    out: &'a mut [u8],
    out_off: &'a mut [u8],
    /// First column not yet finalised, and its ring slot.
    next_final: usize,
    edge_slot: usize,
}

impl<'a> RingGrid<'a> {
    fn new(
        out: &'a mut [u8],
        out_off: &'a mut [u8],
        columns: usize,
        bins: usize,
        reach: usize,
    ) -> Self {
        let width = (2 * reach + 2).min(columns.max(1));
        Self {
            bins,
            columns,
            reach,
            width,
            ring_l: vec![0; width * bins],
            ring_o: vec![OFF_ZERO; width * bins],
            out,
            out_off,
            next_final: 0,
            edge_slot: 0,
        }
    }

    /// Release every column that frame `c` can no longer reach.
    fn advance(&mut self, c: usize) {
        while self.next_final + self.reach < c && self.next_final < self.columns {
            self.finalise_next();
        }
    }

    fn finalise_next(&mut self) {
        let j = self.next_final;
        let slot = self.edge_slot * self.bins..(self.edge_slot + 1) * self.bins;
        let dst = j * self.bins..(j + 1) * self.bins;
        self.out[dst.clone()].copy_from_slice(&self.ring_l[slot.clone()]);
        self.out_off[dst].copy_from_slice(&self.ring_o[slot.clone()]);
        self.ring_l[slot.clone()].fill(0);
        self.ring_o[slot].fill(OFF_ZERO);
        self.next_final += 1;
        self.edge_slot += 1;
        if self.edge_slot == self.width {
            self.edge_slot = 0;
        }
    }

    /// Scatter one prepared frame whose own column is `col`.
    fn scatter(&mut self, pre: Prepared<'_>, col: usize) {
        let last_col = self.columns - 1;
        for k in 0..self.bins {
            let lvl = pre.lvl[k];
            if lvl == 0 {
                continue;
            }
            // Never behind the released edge, never past the end.
            let c = (col as i64 + pre.dc[k] as i64).clamp(self.next_final as i64, last_col as i64)
                as usize;
            // Ring slot without a modulo: `c` is within `width` of the
            // released edge, whose slot is tracked.
            let mut slot = self.edge_slot + (c - self.next_final);
            if slot >= self.width {
                slot -= self.width;
            }
            let i = slot * self.bins + (pre.bin[k] as usize).min(self.bins - 1);
            if lvl > self.ring_l[i] {
                self.ring_l[i] = lvl;
                self.ring_o[i] = pre.off[k];
            }
        }
    }

    fn finish(mut self) {
        while self.next_final < self.columns {
            self.finalise_next();
        }
    }
}

/// One analysed frame: power per bin and, when reassigned, where each bin's
/// energy really sits (time offset in samples, fractional bin).
#[derive(Clone, Copy)]
struct Frame<'a> {
    power: &'a [f32],
    dt: &'a [f32],
    kf: &'a [f32],
}

/// One frame reduced to what the scatter needs per bin: 8-bit level,
/// destination column offset, destination bin and sub-bin position.
#[derive(Clone, Copy)]
struct Prepared<'a> {
    lvl: &'a [u8],
    dc: &'a [i32],
    bin: &'a [u16],
    off: &'a [u8],
}

/// Per-column scratch for [`prepare_frame`], `bins` entries each.
struct PrepBuf {
    lvl: Vec<u8>,
    dc: Vec<i32>,
    bin: Vec<u16>,
    off: Vec<u8>,
}

impl PrepBuf {
    fn new(cells: usize) -> Self {
        Self {
            lvl: vec![0; cells],
            dc: vec![0; cells],
            bin: vec![0; cells],
            off: vec![OFF_ZERO; cells],
        }
    }

    fn column(&self, i: usize, bins: usize) -> Prepared<'_> {
        let r = i * bins..(i + 1) * bins;
        Prepared {
            lvl: &self.lvl[r.clone()],
            dc: &self.dc[r.clone()],
            bin: &self.bin[r.clone()],
            off: &self.off[r],
        }
    }
}

/// Work out where each bin of an analysed frame goes. `stride` is the frame
/// spacing in samples, which converts the time offset into a column offset.
///
/// Bins more than 70 dB below the frame's peak are not moved. Down there the
/// estimates are dominated by leakage from whatever is loud in the frame and
/// point nowhere useful; leaving that energy where the plain STFT would draw
/// it keeps it visible without scattering it.
///
/// Everything else goes to the nearest cell, which also records how far from
/// its centre the energy really sits. The renderer uses that to draw the
/// line at the fractional position, so a glide is smooth without the line
/// getting any wider.
fn prepare_frame(
    fr: Frame<'_>,
    stride: f32,
    lvl: &mut [u8],
    dc: &mut [i32],
    bin: &mut [u16],
    off: &mut [u8],
) {
    let peak = fr.power.iter().copied().fold(0.0f32, f32::max);
    let trust = peak * 1e-7;
    let bins = fr.power.len();
    let last_bin = (bins - 1) as f32;
    let inv_stride = 1.0 / stride.max(1e-6);
    // Casts truncate toward zero, so round via a positive bias: this runs
    // once per cell and a libm `round` each would dominate.
    const BIAS: f32 = 1_048_576.0;
    for k in 0..bins {
        let p = fr.power[k];
        lvl[k] = power_to_u8(p);
        if p < trust {
            dc[k] = 0;
            bin[k] = k as u16;
            off[k] = OFF_ZERO;
            continue;
        }
        dc[k] = ((fr.dt[k] * inv_stride).clamp(-BIAS, BIAS) + BIAS + 0.5) as i32 - BIAS as i32;
        let bf = fr.kf[k].clamp(0.0, last_bin);
        let b = (bf + 0.5) as u32 as f32;
        bin[k] = b as u16;
        off[k] = off_to_u8(bf - b);
    }
}

#[inline]
pub fn db_to_u8(db: f32) -> u8 {
    let t = (db - DB_FLOOR) / -DB_FLOOR; // 0 at floor, 1 at 0 dB
    (t.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[inline]
pub fn u8_to_db(v: u8) -> f32 {
    DB_FLOOR + (v as f32 / 255.0) * -DB_FLOOR
}

/// A high-resolution STFT of one frame range, computed on demand when the
/// view is zoomed in past the base hop.
///
/// The whole-file spectrogram is computed once at a fixed hop, so magnifying
/// beyond one column per pixel can only enlarge blocks. A detail tile
/// recomputes the visible range at whatever hop the current zoom deserves,
/// which is what actually adds resolution.
pub struct DetailTile {
    pub params: StftParams,
    pub sample_rate: u32,
    pub start_frame: f64,
    pub end_frame: f64,
    pub bins: usize,
    pub columns: usize,
    /// Frames between adjacent columns.
    pub stride: f64,
    data: Vec<u8>,
    offs: Option<Vec<u8>>,
}

impl DetailTile {
    /// Widest tile we will ever compute, to bound both time and memory.
    pub const MAX_COLUMNS: usize = 6000;

    pub fn compute(
        samples: &[f32],
        sample_rate: u32,
        params: StftParams,
        start_frame: f64,
        end_frame: f64,
        columns: usize,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Option<Self> {
        if end_frame <= start_frame {
            return None;
        }
        let columns = columns.clamp(1, Self::MAX_COLUMNS);
        let n = params.window_size;
        let bins = n / 2 + 1;
        let stride = (end_frame - start_frame) / columns as f64;
        let origin = start_frame + stride * 0.5;
        let centre = |col: usize| origin + stride * col as f64;
        let threads = worker_count().min(columns);
        let mut analysers: Vec<Analyser> = (0..threads).map(|_| Analyser::new(&params)).collect();
        let mut data = vec![0u8; columns * bins];
        let ok = AtomicBool::new(true);

        let Some(mut offs) = params.reassign.then(|| vec![OFF_ZERO; columns * bins]) else {
            // Plain STFT: every column is independent, so each worker fills
            // its own contiguous run of columns.
            let per = columns.div_ceil(threads);
            std::thread::scope(|s| {
                for (t, (chunk, an)) in data.chunks_mut(per * bins).zip(&mut analysers).enumerate()
                {
                    let ok = &ok;
                    s.spawn(move || {
                        for (i, dst) in chunk.chunks_mut(bins).enumerate() {
                            if i % 32 == 0 && (cancel() || !ok.load(Ordering::Relaxed)) {
                                ok.store(false, Ordering::Relaxed);
                                return;
                            }
                            if an.analyse(samples, centre(t * per + i)).is_none() {
                                ok.store(false, Ordering::Relaxed);
                                return;
                            }
                            for (d, p) in dst.iter_mut().zip(&an.power) {
                                *d = power_to_u8(*p);
                            }
                        }
                    });
                }
            });
            if !ok.load(Ordering::Relaxed) {
                return None;
            }
            return Some(Self {
                params,
                sample_rate,
                start_frame,
                end_frame,
                bins,
                columns,
                stride,
                data,
                offs: None,
            });
        };

        // Reassigned: the transforms and the per-cell arithmetic are
        // independent and run in parallel a block of columns at a time;
        // scattering into the grid is sequential because energy moves
        // between neighbouring columns, and is a byte compare per cell.
        let block = (2_000_000 / bins).clamp(threads, 512).min(columns);
        let mut pre = PrepBuf::new(block * bins);
        let reach = ((n as f64 / 2.0) / stride.max(1e-6)).ceil() as usize + 1;
        let mut grid = RingGrid::new(&mut data, &mut offs, columns, bins, reach);
        let mut b0 = 0;
        while b0 < columns {
            if cancel() {
                return None;
            }
            let nb = (columns - b0).min(block);
            let per = nb.div_ceil(threads);
            std::thread::scope(|s| {
                let runs = pre.lvl[..nb * bins]
                    .chunks_mut(per * bins)
                    .zip(pre.dc[..nb * bins].chunks_mut(per * bins))
                    .zip(pre.bin[..nb * bins].chunks_mut(per * bins))
                    .zip(pre.off[..nb * bins].chunks_mut(per * bins))
                    .zip(&mut analysers);
                for (t, ((((lc, dc), bc), oc), an)) in runs.enumerate() {
                    let ok = &ok;
                    s.spawn(move || {
                        let cols = lc
                            .chunks_mut(bins)
                            .zip(dc.chunks_mut(bins))
                            .zip(bc.chunks_mut(bins))
                            .zip(oc.chunks_mut(bins));
                        for (i, (((l, d), b), o)) in cols.enumerate() {
                            if an.analyse(samples, centre(b0 + t * per + i)).is_none() {
                                ok.store(false, Ordering::Relaxed);
                                return;
                            }
                            prepare_frame(an.frame(), stride as f32, l, d, b, o);
                        }
                    });
                }
            });
            if !ok.load(Ordering::Relaxed) {
                return None;
            }
            for i in 0..nb {
                let col = b0 + i;
                grid.advance(col);
                grid.scatter(pre.column(i, bins), col);
            }
            b0 += nb;
        }
        grid.finish();

        Some(Self {
            params,
            sample_rate,
            start_frame,
            end_frame,
            bins,
            columns,
            stride,
            data,
            offs: Some(offs),
        })
    }

    /// Fractional bin position of the energy in (column, bin); see
    /// [`Spectrogram::pos_at`].
    pub fn pos_at(&self, column: usize, bin: usize) -> f32 {
        match &self.offs {
            Some(o) if column < self.columns && bin < self.bins => {
                bin as f32 + u8_to_off(o[column * self.bins + bin])
            }
            _ => bin as f32,
        }
    }

    /// Stored dB value at (column, bin).
    pub fn db_at(&self, column: usize, bin: usize) -> f32 {
        if column >= self.columns || bin >= self.bins {
            return DB_FLOOR;
        }
        u8_to_db(self.data[column * self.bins + bin])
    }

    /// Whether this tile can serve the given range at the given settings.
    pub fn covers(&self, start: f64, end: f64, params: &StftParams) -> bool {
        self.params == *params && self.start_frame <= start && self.end_frame >= end
    }
}

/// A uniform view over STFT columns, so rendering does not care whether the
/// data came from the whole-file spectrogram or from a detail tile.
struct Src<'a> {
    data: &'a [u8],
    /// Sub-bin positions, when the source was reassigned.
    offs: Option<&'a [u8]>,
    bins: usize,
    columns: usize,
    /// Frame position of the centre of column 0.
    origin: f64,
    /// Frames between adjacent column centres.
    stride: f64,
}

impl Src<'_> {
    #[inline]
    fn col(&self, c: usize) -> &[u8] {
        &self.data[c * self.bins..(c + 1) * self.bins]
    }

    /// Fractional column index at a frame position.
    #[inline]
    fn col_at(&self, frame: f64) -> f64 {
        (frame - self.origin) / self.stride
    }

    /// Value for one output row from one column.
    ///
    /// When the row spans several bins, the peak over them. When the view
    /// magnifies past one bin per row, a tent kernel evaluated at the row's
    /// fractional bin: for a plain STFT every cell sits at its centre and
    /// this reduces to linear interpolation; for a reassigned one each cell
    /// sits at its stored sub-bin position, so a lone harmonic renders as a
    /// one-bin-wide line exactly where it belongs, and glides continuously.
    #[inline]
    fn bin_value(&self, c: usize, b0: usize, b1: usize, bin_f: f64, interp: bool) -> f32 {
        let col = self.col(c);
        if !interp {
            let mut m = 0u8;
            for &v in &col[b0..b1] {
                m = m.max(v);
            }
            return m as f32;
        }
        let offs = self.offs.map(|o| &o[c * self.bins..(c + 1) * self.bins]);
        let kb = bin_f.floor() as isize;
        let mut acc = 0.0f32;
        for k in (kb - 1)..=(kb + 2) {
            if k < 0 || k >= self.bins as isize {
                continue;
            }
            let ku = k as usize;
            let pos = ku as f64 + offs.map_or(0.0, |o| u8_to_off(o[ku]) as f64);
            let w = (1.0 - (bin_f - pos).abs()).max(0.0) as f32;
            if w <= 0.0 {
                continue;
            }
            let v = col[ku] as f32 * w;
            // Plain data: summing the tents is exactly linear interpolation.
            // Reassigned data: two cells can legitimately hold the same
            // position when an estimate sits on a rounding boundary, and
            // summing them would double the line; take the strongest.
            acc = if offs.is_some() { acc.max(v) } else { acc + v };
        }
        acc.min(255.0)
    }
}

#[inline]
fn u8f_to_db(v: f32) -> f32 {
    DB_FLOOR + (v / 255.0) * -DB_FLOOR
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
///
/// When a `detail` tile covers the view it is preferred over the whole-file
/// spectrogram, and wherever the view magnifies past one source cell per
/// pixel the sampling switches from peak-picking to linear interpolation, so
/// zooming in softens rather than blocking up.
pub fn render_view(
    spec: &Spectrogram,
    detail: Option<&DetailTile>,
    view: &ViewParams,
    width: usize,
    height: usize,
    lut: &[Color32],
) -> ColorImage {
    let mut img = ColorImage::filled([width.max(1), height.max(1)], lut[0]);
    if width == 0 || height == 0 || spec.columns() == 0 || view.end_frame <= view.start_frame {
        return img;
    }
    let frames_per_px = (view.end_frame - view.start_frame) / width as f64;

    // Coarsest whole-file level that still has at least one column per pixel.
    let hop = spec.params.hop() as f64;
    let cols_per_px = frames_per_px / hop;
    let mut level_idx = 0;
    let mut level_cols = 1.0;
    for i in 0..spec.levels.len() {
        let factor = LEVEL_FACTOR.pow(i as u32) as f64;
        if cols_per_px >= factor {
            level_idx = i;
            level_cols = factor;
        } else {
            break;
        }
    }
    let level = &spec.levels[level_idx];
    let base = Src {
        data: &level.data,
        offs: level.offs.as_deref(),
        bins: spec.bins,
        columns: level.columns,
        origin: 0.0,
        stride: hop * level_cols,
    };
    if base.columns == 0 || base.bins == 0 {
        return img;
    }

    // A detail tile with matching settings serves every pixel it covers,
    // even if it no longer covers the whole view. That keeps the picture
    // sharp while a replacement is computed instead of snapping back to the
    // coarse level and forward again.
    let tile = detail
        .filter(|d| {
            d.params == spec.params
                && d.columns > 0
                && d.end_frame > view.start_frame
                && d.start_frame < view.end_frame
        })
        .map(|d| {
            (
                Src {
                    data: &d.data,
                    offs: d.offs.as_deref(),
                    bins: d.bins,
                    columns: d.columns,
                    origin: d.start_frame + d.stride * 0.5,
                    stride: d.stride,
                },
                d.start_frame,
                d.end_frame,
            )
        });

    // Per-row bin ranges and centres (top row = max_hz).
    let bins = spec.bins;
    let nyq = spec.nyquist();
    let min_hz = view.min_hz.max(if view.log_frequency { 10.0 } else { 0.0 });
    let max_hz = view.max_hz.min(nyq).max(min_hz + 1.0);
    let hz_per_bin = spec.bin_hz(1).max(1e-6) as f64;
    let hz_at = |t: f32| -> f64 {
        if view.log_frequency {
            let (lmin, lmax) = (min_hz.ln(), max_hz.ln());
            (lmin + (lmax - lmin) * t).exp() as f64
        } else {
            (min_hz + (max_hz - min_hz) * t) as f64
        }
    };
    let rows: Vec<(usize, usize, f64, bool)> = (0..height)
        .map(|row| {
            let hz_top = hz_at(1.0 - row as f32 / height as f32);
            let hz_bot = hz_at(1.0 - (row + 1) as f32 / height as f32);
            let bf0 = hz_bot / hz_per_bin;
            let bf1 = hz_top / hz_per_bin;
            let b0 = (bf0.floor().max(0.0) as usize).min(bins - 1);
            let b1 = ((bf1.ceil().max(0.0) as usize).max(b0 + 1)).min(bins);
            let interp = (bf1 - bf0) <= 1.0;
            (b0, b1, (bf0 + bf1) * 0.5, interp)
        })
        .collect();

    // Which source each pixel column reads and how, decided once per column.
    struct Column {
        tile: bool,
        interp: bool,
        cf: f64,
        c0: usize,
        c1: usize,
    }
    let columns: Vec<Column> = (0..width)
        .map(|x| {
            let f0 = view.start_frame + frames_per_px * x as f64;
            let f1 = f0 + frames_per_px;
            let (src, is_tile) = match &tile {
                Some((t, a, b)) if f0 >= *a && f1 <= *b => (t, true),
                _ => (&base, false),
            };
            let last_col = src.columns - 1;
            let ca = src.col_at(f0);
            let cb = src.col_at(f1);
            let c0 = (ca.floor().max(0.0) as usize).min(last_col);
            Column {
                tile: is_tile,
                interp: (cb - ca) <= 1.0,
                cf: ((ca + cb) * 0.5).clamp(0.0, last_col as f64),
                c0,
                c1: ((cb.ceil().max(0.0) as usize).max(c0 + 1)).min(src.columns),
            }
        })
        .collect();

    // Level → colour, tabulated finely enough that interpolated values do
    // not band.
    let db_range = (view.db_max - view.db_min).max(1e-3);
    const SHADES: usize = 1024;
    let shade: Vec<Color32> = (0..SHADES)
        .map(|i| {
            let v = i as f32 * (255.0 / (SHADES - 1) as f32);
            let t = ((u8f_to_db(v) - view.db_min) / db_range).clamp(0.0, 1.0);
            lut[(t * (lut.len() - 1) as f32).round() as usize]
        })
        .collect();
    let shade_scale = (SHADES - 1) as f32 / 255.0;

    // Render a run of pixel columns, column-major so each STFT column is
    // read once, contiguously.
    let render_columns = |x0: usize, x1: usize, out: &mut [Color32]| {
        let run = x1 - x0;
        for (i, x) in (x0..x1).enumerate() {
            let cx = &columns[x];
            let src = if cx.tile {
                &tile.as_ref().expect("tile column without tile").0
            } else {
                &base
            };
            let last_col = src.columns - 1;
            for (row, &(b0, b1, bin_f, interp_y)) in rows.iter().enumerate() {
                let v = if cx.interp {
                    let a = cx.cf.floor() as usize;
                    let b = (a + 1).min(last_col);
                    let t = (cx.cf - a as f64) as f32;
                    src.bin_value(a, b0, b1, bin_f, interp_y) * (1.0 - t)
                        + src.bin_value(b, b0, b1, bin_f, interp_y) * t
                } else {
                    let mut m = 0.0f32;
                    for c in cx.c0..cx.c1 {
                        m = m.max(src.bin_value(c, b0, b1, bin_f, interp_y));
                    }
                    m
                };
                let idx = (v.clamp(0.0, 255.0) * shade_scale) as usize;
                out[row * run + i] = shade[idx];
            }
        }
    };

    let pixels = img.pixels.as_mut_slice();
    let threads = if width * height >= 1 << 16 {
        worker_count().min(width)
    } else {
        1
    };
    if threads <= 1 {
        render_columns(0, width, pixels);
        return img;
    }
    let per = width.div_ceil(threads);
    let parts: Vec<(usize, Vec<Color32>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..width)
            .step_by(per)
            .map(|x0| {
                let x1 = (x0 + per).min(width);
                let render_columns = &render_columns;
                s.spawn(move || {
                    let mut out = vec![Color32::TRANSPARENT; (x1 - x0) * height];
                    render_columns(x0, x1, &mut out);
                    (x0, out)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("render worker panicked"))
            .collect()
    });
    for (x0, part) in parts {
        let run = part.len() / height;
        for row in 0..height {
            pixels[row * width + x0..row * width + x0 + run]
                .copy_from_slice(&part[row * run..(row + 1) * run]);
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
    fn fast_log_is_within_a_fraction_of_a_step() {
        let mut worst = 0.0f32;
        let mut step_misses = 0;
        for i in 0..20_000 {
            let p = 10f32.powf(-12.0 + 12.5 * i as f32 / 20_000.0);
            let exact = 10.0 * p.log10();
            let approx = 3.010_3 * fast_log2(p);
            worst = worst.max((exact - approx).abs());
            if (power_to_u8(p) as i32 - db_to_u8(exact) as i32).abs() > 1 {
                step_misses += 1;
            }
        }
        assert!(worst < 0.02, "worst error {worst} dB");
        assert_eq!(step_misses, 0);
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
                ..Default::default()
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
            ..Default::default()
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
        let lut = super::super::ColorMap::Grey.lut(1.0);
        let img = render_view(&spec, None, &view, 64, 40, &lut);
        assert_eq!(img.size, [64, 40]);
        // 1 kHz of 4 kHz linear from the bottom = row 40 * (1 - 0.25) = 30.
        let bright = img.pixels[30 * 64 + 32].r();
        let dark = img.pixels[5 * 64 + 32].r();
        assert!(bright > 200, "tone row {bright}");
        assert!(dark < 60, "quiet row {dark}");
    }

    #[test]
    fn detail_tile_samples_finer_than_the_base_hop() {
        // Note on what a tile can and cannot do: the hop sets how densely the
        // STFT is *sampled*, but true time resolution is set by the window
        // length. A tile therefore removes blockiness at high zoom; it cannot
        // separate events closer together than one window.
        let sr = 48_000;
        let n = sr as usize / 4;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (std::f32::consts::TAU * (500.0 + 8000.0 * t) * t).sin()
            })
            .collect();
        let params = StftParams::default();
        let (a, b) = (5_000.0, 8_000.0);
        let tile = DetailTile::compute(&sig, sr, params, a, b, 600, &|| false).unwrap();

        assert_eq!(tile.columns, 600);
        assert!(
            tile.stride < params.hop() as f64 / 4.0,
            "stride {} vs base hop {}",
            tile.stride,
            params.hop()
        );
        assert!(tile.covers(a, b, &params));
        assert!(!tile.covers(a - 1.0, b, &params));
        let other = StftParams {
            window_size: 1024,
            ..params
        };
        assert!(
            !tile.covers(a, b, &other),
            "different settings must not reuse a tile"
        );

        // The sweep must rise across the tile.
        let peak_bin = |c: usize| {
            (0..tile.bins)
                .max_by_key(|&i| tile.data[c * tile.bins + i])
                .unwrap()
        };
        let (first, last) = (peak_bin(10), peak_bin(tile.columns - 10));
        assert!(last > first, "sweep should rise: {first} -> {last}");
    }

    #[test]
    fn magnified_render_is_not_blocky_and_a_tile_adds_detail() {
        let sr = 48_000;
        let sig: Vec<f32> = (0..sr as usize)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (std::f32::consts::TAU * (300.0 + 2000.0 * t) * t).sin()
            })
            .collect();
        let params = StftParams::default();
        let spec = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
        // 1920 frames across 400 px is 4.8 frames per pixel, so one base
        // column (512 frames) would otherwise span about 107 pixels.
        let view = ViewParams {
            start_frame: 10_000.0,
            end_frame: 11_920.0,
            min_hz: 20.0,
            max_hz: 12_000.0,
            log_frequency: true,
            db_min: -90.0,
            db_max: 0.0,
        };
        let lut = super::super::ColorMap::Grey.lut(1.0);
        let tile = DetailTile::compute(
            &sig,
            sr,
            params,
            view.start_frame,
            view.end_frame,
            400,
            &|| false,
        )
        .unwrap();

        let (w, h) = (400usize, 200usize);
        let base_img = render_view(&spec, None, &view, w, h, &lut);
        let fine_img = render_view(&spec, Some(&tile), &view, w, h, &lut);

        // Longest run of identical pixels along a row. Nearest-neighbour
        // sampling would leave runs near the 107 px block width.
        let longest_run = |img: &ColorImage| -> usize {
            let mut worst = 0;
            for row in 0..h {
                // Silent rows are legitimately constant; only rows carrying
                // signal say anything about sampling quality.
                let row_px = &img.pixels[row * w..(row + 1) * w];
                if row_px.iter().map(|p| p.r()).max().unwrap_or(0) < 60 {
                    continue;
                }
                let (mut run, mut prev) = (1usize, row_px[0]);
                for p in &row_px[1..] {
                    run = if *p == prev { run + 1 } else { 1 };
                    prev = *p;
                    worst = worst.max(run);
                }
            }
            worst
        };
        // With one tile column per pixel the render follows the data rather
        // than holding one base column across ~107 pixels. Absolute run
        // lengths are not asserted: a clean synthetic sweep has genuinely
        // flat stretches, so only the improvement is meaningful.
        let (rb, rf) = (longest_run(&base_img), longest_run(&fine_img));
        assert!(
            rf * 2 < rb,
            "detail render should break up the base's blocks: {rb} -> {rf}"
        );

        // Total variation along x: the tile carries real structure the base
        // can only interpolate through.
        let variation = |img: &ColorImage| -> u64 {
            let mut v = 0u64;
            for row in 0..h {
                for x in 1..w {
                    let a = img.pixels[row * w + x].r() as i32;
                    let b = img.pixels[row * w + x - 1].r() as i32;
                    v += a.abs_diff(b) as u64;
                }
            }
            v
        };
        let (vb, vf) = (variation(&base_img), variation(&fine_img));
        assert!(
            vf > vb,
            "detail should carry more structure: base {vb}, tile {vf}"
        );
    }

    #[test]
    fn reassignment_collapses_a_tone_between_bins() {
        // A tone exactly on bin 100 spreads across the main lobe in an
        // ordinary STFT; reassigned, every bin's estimate points at the true
        // frequency, so the energy collapses into bin 100 and its neighbours
        // go dark.
        let sr = 48_000;
        let n = 2048;
        let freq = 100.0 * sr as f32 / n as f32;
        let sig: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect();
        let plain = StftParams {
            window_size: n,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: false,
        };
        let sharp = StftParams {
            reassign: true,
            ..plain
        };
        let a = Spectrogram::compute(&sig, sr, plain, &|_| {}, &|| false).unwrap();
        let b = Spectrogram::compute(&sig, sr, sharp, &|_| {}, &|| false).unwrap();
        let col = a.columns() / 2;

        // Level at the true bin is preserved.
        assert!(
            (a.db_at(col, 100)).abs() < 1.5,
            "plain peak {}",
            a.db_at(col, 100)
        );
        assert!(
            (b.db_at(col, 100)).abs() < 1.5,
            "reassigned peak {}",
            b.db_at(col, 100)
        );
        // Plain leaks into the neighbour; reassigned does not.
        assert!(
            a.db_at(col, 101) > -20.0,
            "plain neighbour {}",
            a.db_at(col, 101)
        );
        for k in [97usize, 98, 99, 101, 102, 103] {
            assert!(
                b.db_at(col, k) < -100.0,
                "reassigned bin {k} should be empty, got {}",
                b.db_at(col, k)
            );
        }
    }

    #[test]
    fn reassignment_concentrates_an_impulse_in_time() {
        let sr = 48_000;
        let n = 2048;
        let hop = 512;
        let mut sig = vec![0.0f32; sr as usize];
        let t0 = 78 * hop; // exactly on a column centre
        sig[t0] = 1.0;
        let plain = StftParams {
            window_size: n,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: false,
        };
        assert_eq!(plain.hop(), hop);
        let sharp = StftParams {
            reassign: true,
            ..plain
        };
        let a = Spectrogram::compute(&sig, sr, plain, &|_| {}, &|| false).unwrap();
        let b = Spectrogram::compute(&sig, sr, sharp, &|_| {}, &|| false).unwrap();
        let home = (t0 as f64 / hop as f64).round() as usize;
        let bin = 200;

        // The ordinary spectrogram smears the click across neighbouring
        // columns; reassigned, they are empty and the home column keeps it.
        assert!(
            a.db_at(home - 1, bin) > -70.0,
            "plain neighbour {}",
            a.db_at(home - 1, bin)
        );
        assert!(a.db_at(home + 1, bin) > -70.0);
        assert!(
            b.db_at(home - 1, bin) < -110.0,
            "reassigned neighbour {}",
            b.db_at(home - 1, bin)
        );
        assert!(b.db_at(home + 1, bin) < -110.0);
        assert!(
            (a.db_at(home, bin) - b.db_at(home, bin)).abs() < 2.0,
            "home column level should be preserved: {} vs {}",
            a.db_at(home, bin),
            b.db_at(home, bin)
        );
    }

    #[test]
    fn reassigned_detail_tile_matches_the_base_on_a_tone() {
        let sr = 48_000;
        let n = 2048;
        let freq = 300.25 * sr as f32 / n as f32;
        let sig: Vec<f32> = (0..sr as usize / 2)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect();
        let params = StftParams {
            reassign: true,
            ..StftParams::default()
        };
        let tile =
            DetailTile::compute(&sig, sr, params, 8_000.0, 16_000.0, 400, &|| false).unwrap();
        let c = tile.columns / 2;
        let db = |k: usize| u8_to_db(tile.data[c * tile.bins + k]);
        let pos = tile.pos_at(c, 300);
        assert!((pos - 300.25).abs() < 0.05, "tile stored position {pos}");
        assert!(db(300).abs() < 1.5, "tile peak {}", db(300));
        assert!(
            db(299) < -100.0 && db(301) < -100.0,
            "{} {}",
            db(299),
            db(301)
        );
        // Different reassignment setting must not reuse a tile.
        assert!(!tile.covers(9_000.0, 10_000.0, &StftParams::default()));
    }

    #[test]
    fn reassignment_keeps_a_fractional_frequency() {
        // A tone at bin 100.4 must be stored in bin 100 at full level, with
        // its position recorded as 100.4, and nothing in the neighbours: the
        // line is one cell wide and exactly placed.
        let sr = 48_000;
        let n = 2048;
        let freq = 100.4 * sr as f32 / n as f32;
        let sig: Vec<f32> = (0..sr as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect();
        let params = StftParams {
            window_size: n,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: true,
        };
        let s = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
        let col = s.columns() / 2;
        assert!(s.db_at(col, 100).abs() < 1.5, "{}", s.db_at(col, 100));
        let pos = s.pos_at(col, 100);
        assert!((pos - 100.4).abs() < 0.05, "stored position {pos}");
        for k in [98usize, 99, 101, 102, 103] {
            assert!(s.db_at(col, k) < -100.0, "bin {k}: {}", s.db_at(col, k));
        }
        let plain = Spectrogram::compute(
            &sig,
            sr,
            StftParams {
                reassign: false,
                ..params
            },
            &|_| {},
            &|| false,
        )
        .unwrap();
        assert_eq!(plain.pos_at(col, 100), 100.0);
    }

    #[test]
    fn reassigned_glide_is_a_smooth_line_not_a_staircase() {
        // A slow linear chirp across ten bins. Following the brightest cell
        // per column, its recorded position must move in small steps and
        // must not sit on whole bins, which is what rounding produces.
        let sr = 48_000;
        let n = 2048;
        let secs = 2.0f32;
        let (f_a, f_b) = (95.0 * sr as f32 / n as f32, 105.0 * sr as f32 / n as f32);
        let len = (sr as f32 * secs) as usize;
        let sig: Vec<f32> = (0..len)
            .map(|i| {
                let t = i as f32 / sr as f32;
                let phase = std::f32::consts::TAU * (f_a * t + 0.5 * (f_b - f_a) / secs * t * t);
                phase.sin()
            })
            .collect();
        let params = StftParams {
            window_size: n,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: true,
        };
        let s = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
        let cols = s.columns();
        let track: Vec<f32> = (4..cols - 4)
            .map(|c| {
                let k = (88..112)
                    .max_by(|&a, &b| s.db_at(c, a).total_cmp(&s.db_at(c, b)))
                    .unwrap();
                s.pos_at(c, k)
            })
            .collect();
        let max_step = track
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(
            max_step < 0.25,
            "largest jump between adjacent columns: {max_step} bins"
        );
        let on_integer = track
            .iter()
            .filter(|c| (*c - c.round()).abs() < 0.02)
            .count();
        assert!(
            on_integer * 5 < track.len(),
            "{on_integer} of {} columns sit on whole bins: staircase",
            track.len()
        );
        assert!(
            track[0] < 96.0 && *track.last().unwrap() > 104.0,
            "glide should span the range"
        );
    }

    #[test]
    fn render_draws_a_fractional_tone_between_rows_and_keeps_it_thin() {
        // Two reassigned tones, on bin 100.0 and 100.5, rendered magnified on
        // a linear axis at 20 px per bin. The second must land half a bin
        // (10 px) higher on screen, and both lines must be equally thin.
        let sr = 48_000;
        let n = 2048;
        let params = StftParams {
            window_size: n,
            overlap_num: 3,
            overlap_den: 4,
            window: WindowKind::Hann,
            reassign: true,
        };
        let hz_bin = sr as f32 / n as f32;
        let lut = super::super::ColorMap::Grey.lut(1.0);
        let render_centre = |bin: f32| -> (f32, usize) {
            let sig: Vec<f32> = (0..sr as usize / 2)
                .map(|i| (std::f32::consts::TAU * bin * hz_bin * i as f32 / sr as f32).sin())
                .collect();
            let s = Spectrogram::compute(&sig, sr, params, &|_| {}, &|| false).unwrap();
            let view = ViewParams {
                start_frame: 6_000.0,
                end_frame: 18_000.0,
                min_hz: 95.0 * hz_bin,
                max_hz: 106.0 * hz_bin,
                log_frequency: false,
                db_min: -60.0,
                db_max: 0.0,
            };
            let (w, h) = (40usize, 220usize);
            let img = render_view(&s, None, &view, w, h, &lut);
            let x = w / 2;
            let col: Vec<f32> = (0..h).map(|r| img.pixels[r * w + x].r() as f32).collect();
            let peak = col.iter().copied().fold(0.0, f32::max);
            let (num, den) = col
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(n_, d), (r, v)| (n_ + r as f32 * v, d + v));
            let thick = col.iter().filter(|&&v| v > peak * 0.5).count();
            (num / den, thick)
        };
        let (c0, t0) = render_centre(100.0);
        let (c1, t1) = render_centre(100.5);
        assert!(
            (c0 - c1 - 10.0).abs() < 3.0,
            "centres {c0} and {c1}: expected ~10 px apart"
        );
        assert!(t0 <= 40 && t1 <= 40, "lines too thick: {t0} {t1} px");
        assert!(
            (t0 as i32 - t1 as i32).abs() <= 6,
            "fractional line should be as thin as the on-bin one: {t0} vs {t1}"
        );
    }

    #[test]
    fn cancel_stops_early() {
        let sig = vec![0.0f32; 1 << 20];
        let r = Spectrogram::compute(&sig, 48_000, StftParams::default(), &|_| {}, &|| true);
        assert!(r.is_none());
    }
}
