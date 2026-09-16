//! Analysis window functions.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    BlackmanHarris,
}

impl WindowKind {
    pub const ALL: [WindowKind; 5] = [
        WindowKind::Hann,
        WindowKind::Hamming,
        WindowKind::Blackman,
        WindowKind::BlackmanHarris,
        WindowKind::Rectangular,
    ];

    pub fn name(self) -> &'static str {
        match self {
            WindowKind::Rectangular => "Rectangular",
            WindowKind::Hann => "Hann",
            WindowKind::Hamming => "Hamming",
            WindowKind::Blackman => "Blackman",
            WindowKind::BlackmanHarris => "Blackman-Harris",
        }
    }

    /// Periodic window of length `n`, as used for STFT analysis.
    pub fn make(self, n: usize) -> Vec<f32> {
        let nf = n as f32;
        (0..n)
            .map(|i| {
                let x = std::f32::consts::TAU * i as f32 / nf;
                match self {
                    WindowKind::Rectangular => 1.0,
                    WindowKind::Hann => 0.5 - 0.5 * x.cos(),
                    WindowKind::Hamming => 0.54 - 0.46 * x.cos(),
                    WindowKind::Blackman => 0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos(),
                    WindowKind::BlackmanHarris => {
                        0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos()
                            - 0.01168 * (3.0 * x).cos()
                    }
                }
            })
            .collect()
    }

    /// Sum of the window's coefficients; dividing a magnitude spectrum by
    /// this makes a full-scale sine read 0 dB regardless of window or length.
    pub fn coherent_gain(self, n: usize) -> f32 {
        self.make(n).iter().sum::<f32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_are_bounded_and_symmetric() {
        for kind in WindowKind::ALL {
            let w = kind.make(1024);
            assert_eq!(w.len(), 1024);
            for v in &w {
                assert!(*v >= -1e-6 && *v <= 1.0 + 1e-6, "{kind:?} {v}");
            }
            // periodic windows: w[i] == w[n - i]
            for i in 1..512 {
                assert!((w[i] - w[1024 - i]).abs() < 1e-5, "{kind:?} at {i}");
            }
        }
    }

    #[test]
    fn hann_peaks_at_one() {
        let w = WindowKind::Hann.make(256);
        assert!((w[128] - 1.0).abs() < 1e-6);
        assert!(w[0].abs() < 1e-6);
    }
}
