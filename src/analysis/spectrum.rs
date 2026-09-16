//! Realtime spectrum of whatever is going to the output device.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use super::window::WindowKind;

pub struct LiveSpectrum {
    size: usize,
    sample_rate: u32,
    window: Vec<f32>,
    gain: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    history: Vec<f32>,
    write: usize,
    input: Vec<f32>,
    output: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// Smoothed dB per bin.
    pub db: Vec<f32>,
    /// Peak-hold dB per bin.
    pub peak: Vec<f32>,
    /// 0 = no averaging, 0.9 = very slow.
    pub averaging: f32,
    pub peak_decay_db_per_s: f32,
    fresh: bool,
}

impl LiveSpectrum {
    pub fn new(size: usize, sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(size);
        let bins = size / 2 + 1;
        Self {
            size,
            sample_rate,
            window: WindowKind::Hann.make(size),
            gain: 2.0 / WindowKind::Hann.coherent_gain(size),
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            history: vec![0.0; size],
            write: 0,
            db: vec![-120.0; bins],
            peak: vec![-120.0; bins],
            averaging: 0.6,
            peak_decay_db_per_s: 12.0,
            fresh: false,
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn bins(&self) -> usize {
        self.db.len()
    }

    pub fn bin_hz(&self, i: usize) -> f32 {
        i as f32 * self.sample_rate as f32 / self.size as f32
    }

    /// Feed mono samples at the device rate.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.history[self.write] = s;
            self.write = (self.write + 1) % self.size;
        }
        if !samples.is_empty() {
            self.fresh = true;
        }
    }

    /// Recompute the spectrum from the most recent `size` samples.
    /// `dt` is the wall time since the last call, for peak decay.
    pub fn update(&mut self, dt: f32) {
        if !self.fresh {
            let decay = self.peak_decay_db_per_s * dt;
            for p in &mut self.peak {
                *p -= decay;
            }
            return;
        }
        self.fresh = false;
        for i in 0..self.size {
            let idx = (self.write + i) % self.size;
            self.input[i] = self.history[idx] * self.window[i];
        }
        if self
            .fft
            .process_with_scratch(&mut self.input, &mut self.output, &mut self.scratch)
            .is_err()
        {
            return;
        }
        let a = self.averaging.clamp(0.0, 0.99);
        let decay = self.peak_decay_db_per_s * dt;
        for (i, c) in self.output.iter().enumerate() {
            let db = 20.0 * (c.norm() * self.gain).max(1e-10).log10();
            self.db[i] = a * self.db[i] + (1.0 - a) * db;
            self.peak[i] = (self.peak[i] - decay).max(self.db[i]);
        }
    }

    pub fn clear(&mut self) {
        self.history.fill(0.0);
        self.db.fill(-120.0);
        self.peak.fill(-120.0);
        self.fresh = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_spectrum_finds_tone() {
        let sr = 48_000;
        let size = 2048;
        let mut ls = LiveSpectrum::new(size, sr);
        ls.averaging = 0.0;
        let freq = 200.0 * sr as f32 / size as f32; // bin 200 exactly
        let sig: Vec<f32> = (0..size * 2)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr as f32).sin())
            .collect();
        ls.push(&sig);
        ls.update(0.016);
        let (peak, _) = ls
            .db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert_eq!(peak, 200);
        assert!(ls.db[200].abs() < 1.0, "{}", ls.db[200]);
    }
}
