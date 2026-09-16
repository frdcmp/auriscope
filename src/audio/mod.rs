//! Decoding and playback.

pub mod decoder;
pub mod engine;

pub use decoder::{DecodedAudio, FileInfo, decode_file};
pub use engine::{Engine, Shared};
