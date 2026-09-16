//! Offline and realtime analysis. Everything here is pure computation on
//! slices and is exercised by tests with synthesised signals.

pub mod colormap;
pub mod pyramid;
pub mod spectrum;
pub mod stats;
pub mod stft;
pub mod window;

pub use colormap::ColorMap;
pub use pyramid::{Bin, WaveformPyramid};
pub use spectrum::LiveSpectrum;
pub use stats::{ChannelStats, FileStats, compute_stats};
pub use stft::{DetailTile, Spectrogram, StftParams, ViewParams, render_view};
pub use window::WindowKind;

/// Decibels from a linear amplitude, floored so silence does not become -inf.
#[inline]
pub fn amp_to_db(a: f32) -> f32 {
    20.0 * a.max(1e-10).log10()
}

#[inline]
pub fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
