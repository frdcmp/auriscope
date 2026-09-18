//! Offline and realtime analysis. Everything here is pure computation on
//! slices and is exercised by tests with synthesised signals.

pub mod colormap;
pub mod defects;
pub mod envelope;
pub mod pyramid;
pub mod segments;
pub mod spectral;
pub mod spectrum;
pub mod stats;
pub mod stft;
pub mod window;

pub use colormap::{ColorMap, CustomStops, DEFAULT_CUSTOM};
pub use defects::{Click, Seam, Truncation, ZeroRun, clicks, floor_steps, truncation, zero_runs};
pub use envelope::{Biquad, Envelope, SPEECH_BAND, band_limit, high_pass};
pub use pyramid::{Bin, WaveformPyramid};
pub use segments::{
    Kind, Params as SegmentParams, Pause, Segment, Segmentation, Structure, segment,
};
pub use spectral::{Band, Spectral, Spread, Tone};
pub use spectrum::LiveSpectrum;
pub use stats::{ChannelStats, ChannelTimeline, FileStats, Timeline, compute_stats};
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
