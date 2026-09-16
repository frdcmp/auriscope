//! Whole-file decoding through Symphonia.
//!
//! The open pass decodes the entire file into planar `f32` once. Everything
//! downstream (playback, waveform pyramid, spectrogram, loudness) reads from
//! that cache, so the decoder is never touched again while the file is open.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::{MetadataOptions, RawValue};

/// What the file actually is.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub container: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: Option<u32>,
    pub frames: usize,
    /// Tag key/value pairs as found in the container, in file order.
    pub tags: Vec<(String, String)>,
}

impl FileInfo {
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frames as f64 / self.sample_rate as f64
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// A fully decoded file: planar samples, one `Vec<f32>` per channel.
pub struct DecodedAudio {
    pub info: FileInfo,
    pub channels: Vec<Vec<f32>>,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    /// Mono mixdown, allocated. Used by the live spectrum tests and nothing hot.
    pub fn mono(&self) -> Vec<f32> {
        let n = self.frames();
        let scale = 1.0 / self.channels.len().max(1) as f32;
        let mut out = vec![0.0f32; n];
        for ch in &self.channels {
            for (o, s) in out.iter_mut().zip(ch) {
                *o += *s * scale;
            }
        }
        out
    }
}

/// Decode a whole file. `progress` receives values in `0.0..=1.0` when the
/// container reports a frame count, and is otherwise not called.
pub fn decode_file(path: &Path, progress: &dyn Fn(f32)) -> Result<Arc<DecodedAudio>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("no supported container found")?;

    let container = reader.format_info().long_name.to_string();

    let track = reader
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("file has no audio track"))?;
    let track_id = track.id;
    let expected_frames = track.num_frames;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .cloned()
        .ok_or_else(|| anyhow!("audio track has no codec parameters"))?;

    let mut tags = Vec::new();
    if let Some(rev) = reader.metadata().skip_to_latest() {
        for tag in &rev.media.tags {
            let key = match &tag.std {
                Some(std) => format!("{std:?}")
                    .split('(')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                None => tag.raw.key.clone(),
            };
            if let Some(value) = raw_value_to_string(&tag.raw.value) {
                tags.push((key, value));
            }
        }
    }

    let opts = AudioDecoderOptions::default().gapless(true).verify(false);
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &opts)
        .context("no decoder for this codec")?;
    let codec = decoder.codec_info().long_name.to_string();

    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    let mut sample_rate = params.sample_rate.unwrap_or(0);

    loop {
        let packet = match reader.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(e).context("reading packet"),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(msg)) => {
                log::warn!("decode error, skipping packet: {msg}");
                continue;
            }
            Err(SymError::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(e).context("decoding packet"),
        };
        if buf.is_empty() {
            continue;
        }
        if channels.is_empty() {
            let spec = buf.spec();
            sample_rate = spec.rate();
            let n = spec.channels().count();
            if n == 0 {
                bail!("decoder reported zero channels");
            }
            let cap = expected_frames.unwrap_or(0) as usize;
            channels = (0..n).map(|_| Vec::with_capacity(cap)).collect();
            scratch = vec![Vec::new(); n];
        }
        buf.copy_to_vecs_planar::<f32>(&mut scratch);
        for (dst, src) in channels.iter_mut().zip(&scratch) {
            dst.extend_from_slice(src);
        }
        if let Some(total) = expected_frames
            && total > 0
        {
            let done = channels[0].len() as f64 / total as f64;
            progress(done.min(1.0) as f32);
        }
    }

    if channels.is_empty() {
        bail!("file decoded to zero frames");
    }
    if sample_rate == 0 {
        bail!("could not determine sample rate");
    }
    let frames = channels[0].len();
    // Some containers deliver ragged final blocks; keep every channel equal.
    for ch in &mut channels {
        ch.resize(frames, 0.0);
    }
    progress(1.0);

    let info = FileInfo {
        path: path.to_path_buf(),
        container,
        codec,
        sample_rate,
        channels: channels.len(),
        bits_per_sample: params.bits_per_sample,
        frames,
        tags,
    };
    Ok(Arc::new(DecodedAudio { info, channels }))
}

fn raw_value_to_string(v: &RawValue) -> Option<String> {
    Some(match v {
        RawValue::String(s) => s.to_string(),
        RawValue::StringList(l) => l.join(", "),
        RawValue::Boolean(b) => b.to_string(),
        RawValue::Flag => "yes".to_string(),
        RawValue::Float(f) => format!("{f}"),
        RawValue::SignedInt(i) => i.to_string(),
        RawValue::UnsignedInt(u) => u.to_string(),
        RawValue::Binary(b) => format!("<{} bytes>", b.len()),
        _ => return None,
    })
}

/// Write a 16-bit PCM WAV. Only used by tests and tools; kept here so the
/// decoder tests have no extra dependency.
pub fn write_wav_i16(path: &Path, sample_rate: u32, channels: &[Vec<f32>]) -> Result<()> {
    use std::io::Write;
    let nch = channels.len() as u16;
    let frames = channels.first().map_or(0, Vec::len);
    let data_len = (frames * nch as usize * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&nch.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * nch as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(nch * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        for ch in channels {
            let s = (ch[i].clamp(-1.0, 1.0) * 32767.0).round() as i16;
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    File::create(path)?.write_all(&out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("auriscope-test-{}-{name}", std::process::id()));
        p
    }

    #[test]
    fn wav_round_trips_sample_exact() {
        let sr = 48_000;
        let n = 4800;
        let left: Vec<f32> = (0..n).map(|i| (i as f32 / 100.0).sin() * 0.5).collect();
        let right: Vec<f32> = (0..n).map(|i| ((i % 200) as f32 / 200.0) - 0.5).collect();
        let path = temp_path("roundtrip.wav");
        write_wav_i16(&path, sr, &[left.clone(), right.clone()]).unwrap();

        let decoded = decode_file(&path, &|_| {}).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(decoded.info.sample_rate, sr);
        assert_eq!(decoded.info.channels, 2);
        assert_eq!(decoded.frames(), n);
        assert_eq!(decoded.info.bits_per_sample, Some(16));
        for (a, b) in decoded.channels[0].iter().zip(&left) {
            assert!((a - b).abs() <= 1.0 / 32767.0, "{a} vs {b}");
        }
        for (a, b) in decoded.channels[1].iter().zip(&right) {
            assert!((a - b).abs() <= 1.0 / 32767.0, "{a} vs {b}");
        }
    }

    #[test]
    fn unsupported_file_is_an_error() {
        let path = temp_path("garbage.bin");
        std::fs::write(&path, b"this is not audio at all, not even close").unwrap();
        let r = decode_file(&path, &|_| {});
        std::fs::remove_file(&path).ok();
        assert!(r.is_err());
    }
}
