//! What Auriscope knows about a file, as JSON.
//!
//! The same blocks serve two callers: the sidecar the camera button writes
//! beside a capture, and `auriscope-cli analyze`, which is that sidecar
//! without the window. Everything here reads library types only — a decoded
//! file's [`FileInfo`] and the [`FileStats`] measured from its samples — so
//! nothing in it needs a screen.
//!
//! The shapes are meant to be read by something other than a person: flat
//! keys, units in the names, and `null` rather than a missing key wherever a
//! block does not apply.

use serde_json::{Value, json};

use crate::analysis::defects::{Click, Seam, Truncation, ZeroRun};
use crate::analysis::segments::{Kind, Segmentation};
use crate::analysis::spectral::{Spectral, Spread, Tone};
use crate::analysis::{ChannelStats, FileStats, Timeline};
use crate::audio::FileInfo;

/// The version the report was written by. Same crate, so the same number the
/// window's about box shows.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Seconds since the epoch, or 0 on a clock set before it.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What a channel is called, given how many there are.
pub fn channel_name(ch: usize, nch: usize) -> String {
    match (nch, ch) {
        (1, _) => "Mono".into(),
        (2, 0) => "Left".into(),
        (2, 1) => "Right".into(),
        _ => format!("Channel {}", ch + 1),
    }
}

/// A number for JSON, which has no infinity: `null` where a measurement has
/// none — a silent file integrates to −inf LUFS — and otherwise the value at
/// the precision it is read at, rather than whatever an f32 widens to.
pub fn rounded(v: f32, places: i32) -> Value {
    if !v.is_finite() {
        return Value::Null;
    }
    let f = 10f64.powi(places);
    json!(((v as f64) * f).round() / f)
}

/// Everything about the file that is not a measurement: what it is, what its
/// header holds, what it measures, and what is marked in it.
///
/// The whole report, less the parts only a window knows — which view was on
/// screen and how the spectrogram in the picture was drawn. Those are added by
/// the caller that has them.
pub fn file_report(info: &FileInfo, stats: Option<&FileStats>) -> Value {
    json!({
        "auriscope": {
            "version": VERSION,
            "analysed_unix": unix_now(),
            // What the percentile levels were measured over, so a reader can
            // tell what "exceeded 10% of the time" is ten per cent of.
            "level_frame_ms": crate::analysis::stats::FRAME_MS,
        },
        "file": file_json(info),
        "wave": wave_json(info),
        "broadcast_wave": bext_json(info),
        "markers": markers_json(info),
        "loudness": loudness_json(stats),
        "channels": channels_json(stats, &|_| None),
    })
}

pub fn file_json(info: &FileInfo) -> Value {
    json!({
        "name": info.file_name(),
        "path": info.path.display().to_string(),
        "container": info.container,
        "codec": info.codec,
        "sample_rate_hz": info.sample_rate,
        "channels": info.channels,
        "bits_per_sample": info.bits_per_sample,
        "frames": info.frames,
        "duration_secs": rounded(info.duration_secs() as f32, 6),
        "size_bytes": info.file_size,
        "bitrate_kbps": info.bitrate_kbps().map(|b| rounded(b as f32, 1)),
        "modified_unix": info.modified.and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
        }),
        "tags": info
            .tags
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect::<Vec<_>>(),
    })
}

/// The RIFF side of a WAVE file: how it is laid out and what the header says
/// the samples are. `null` for anything that is not one.
pub fn wave_json(info: &FileInfo) -> Value {
    let Some(wav) = info.wav.as_ref() else {
        return Value::Null;
    };
    let fmt = wav.fmt.as_ref().map(|f| {
        json!({
            "format": f.format_name(),
            "format_tag": f.effective_tag(),
            "extensible": f.is_extensible(),
            "channels": f.channels,
            "sample_rate_hz": f.sample_rate,
            "byte_rate": f.byte_rate,
            "block_align": f.block_align,
            "bits_per_sample": f.bits_per_sample,
            "valid_bits": f.valid_bits,
            "channel_mask": f.channel_mask,
        })
    });
    json!({
        "rf64": wav.rf64,
        "riff_size": wav.riff_size,
        "fmt": fmt,
        "info": wav
            .info
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect::<Vec<_>>(),
        "chunks": wav
            .chunks
            .iter()
            .map(|c| json!({ "id": c.id, "size": c.size, "offset": c.offset }))
            .collect::<Vec<_>>(),
    })
}

/// The `bext` chunk: who recorded it, when, and what they measured it at.
pub fn bext_json(info: &FileInfo) -> Value {
    let Some(bext) = info.wav.as_ref().and_then(|w| w.bext.as_ref()) else {
        return Value::Null;
    };
    json!({
        "description": bext.description,
        "originator": bext.originator,
        "originator_reference": bext.originator_reference,
        "origination_date": bext.origination_date,
        "origination_time": bext.origination_time,
        "time_reference_samples": bext.time_reference,
        "version": bext.version,
        "umid": bext.umid,
        "coding_history": bext.coding_history,
        "loudness": bext.loudness.map(|l| json!({
            "integrated_lufs": rounded(l.integrated_lufs, 2),
            "range_lu": rounded(l.range_lu, 2),
            "max_true_peak_dbtp": rounded(l.max_true_peak_dbtp, 2),
            "max_momentary_lufs": rounded(l.max_momentary_lufs, 2),
            "max_short_term_lufs": rounded(l.max_short_term_lufs, 2),
        })),
    })
}

/// Cue points, in frames and in seconds, so neither has to be worked out from
/// the other.
pub fn markers_json(info: &FileInfo) -> Value {
    let Some(wav) = info.wav.as_ref() else {
        return Value::Null;
    };
    let sr = info.sample_rate.max(1) as f64;
    json!(
        wav.cues
            .iter()
            .map(|c| json!({
                "id": c.id,
                "position_frames": c.position,
                "position_secs": (c.position as f64 / sr),
                "label": c.label,
            }))
            .collect::<Vec<_>>()
    )
}

/// Loudness over the whole file, plus the two figures the Loudness card works
/// out for itself, so a reader of the JSON does not have to know how they were
/// arrived at.
pub fn loudness_json(stats: Option<&FileStats>) -> Value {
    let Some(stats) = stats else {
        return Value::Null;
    };
    let fold = |f: fn(&ChannelStats) -> f32| {
        stats
            .channels
            .iter()
            .map(f)
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let true_peak = fold(|c| c.true_peak_dbtp);
    let peak = fold(|c| c.sample_peak_db);
    let rms = fold(|c| c.rms_db);
    json!({
        "integrated_lufs": rounded(stats.integrated_lufs, 2),
        "range_lu": rounded(stats.loudness_range_lu, 2),
        "max_momentary_lufs": rounded(stats.max_momentary_lufs, 2),
        "max_short_term_lufs": rounded(stats.max_short_term_lufs, 2),
        "correlation": stats.correlation.map(|c| rounded(c, 3)),
        "headroom_db": rounded(-true_peak, 2),
        "crest_factor_db": rounded(peak - rms, 2),
        // Peak to loudness: how much room the loudest moment keeps over the
        // average loudness. Small means heavily compressed.
        "plr_db": rounded(true_peak - stats.integrated_lufs, 2),
    })
}

/// Per-channel measurements. `muted` is how the window says which channels
/// were silenced when a capture was taken; a caller with no such notion
/// returns `None` and the key is left off.
pub fn channels_json(stats: Option<&FileStats>, muted: &dyn Fn(usize) -> Option<bool>) -> Value {
    let Some(stats) = stats else {
        return Value::Null;
    };
    let nch = stats.channels.len();
    json!(
        stats
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut v = json!({
                    "index": i,
                    "name": channel_name(i, nch),
                    "sample_peak_dbfs": rounded(c.sample_peak_db, 2),
                    "true_peak_dbtp": rounded(c.true_peak_dbtp, 2),
                    "rms_dbfs": rounded(c.rms_db, 2),
                    "dc_offset": rounded(c.dc_offset, 6),
                    "clipped_samples": c.clipped_samples,
                    "clipped_runs": c.clipped_runs,
                    // Exceeded 10%, 50% and 90% of the time. Digital-zero
                    // frames take no part, so a padded file still has a floor.
                    "l10_dbfs": rounded(c.l10_db, 2),
                    "l50_dbfs": rounded(c.l50_db, 2),
                    "l90_dbfs": rounded(c.l90_db, 2),
                    "noise_floor_dbfs": rounded(c.noise_floor_db, 2),
                    "level_frames": c.level_frames,
                    "zero_frames": c.zero_frames,
                });
                if let (Some(obj), Some(m)) = (v.as_object_mut(), muted(i)) {
                    obj.insert("muted".into(), json!(m));
                }
                v
            })
            .collect::<Vec<_>>()
    )
}

/// Loudness and level against time. Its own block because it is long: a
/// two-minute file is more than a thousand entries per series, which is worth
/// having and not worth printing unless it was asked for.
pub fn timeline_json(stats: &FileStats) -> Value {
    let t: &Timeline = &stats.timeline;
    let series = |v: &[Option<f32>]| -> Value {
        json!(
            v.iter()
                .map(|x| x.map_or(Value::Null, |v| rounded(v, 2)))
                .collect::<Vec<_>>()
        )
    };
    let nch = t.channels.len();
    json!({
        "block_secs": rounded(t.block_secs, 4),
        "blocks": t.momentary_lufs.len(),
        "momentary_lufs": series(&t.momentary_lufs),
        "short_term_lufs": series(&t.short_term_lufs),
        "channels": t
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| json!({
                "index": i,
                "name": channel_name(i, nch),
                "rms_dbfs": c.rms_db.iter().map(|v| rounded(*v, 2)).collect::<Vec<_>>(),
                "peak_dbfs": c.peak_db.iter().map(|v| rounded(*v, 2)).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
    })
}

/// Where the speech is, how long the silences are, and what sits in them.
pub fn structure_json(seg: &Segmentation, sample_rate: u32) -> Value {
    let st = &seg.structure;
    json!({
        "leading_silence_secs": st.leading_silence_secs,
        "trailing_silence_secs": st.trailing_silence_secs,
        // Measured to the first and last sample that is not zero, which is a
        // different question from where the speech starts and a different
        // answer on a file with digital padding.
        "leading_nonzero_secs": st.leading_nonzero_secs,
        "trailing_nonzero_secs": st.trailing_nonzero_secs,
        "speech_secs": st.speech_secs,
        "speech_ratio": rounded(st.speech_ratio, 4),
        "longest_pause_secs": st
            .pauses
            .iter()
            .map(|p| p.duration_secs())
            .fold(0.0f64, f64::max),
        "pauses": st
            .pauses
            .iter()
            .map(|p| json!({
                "start_secs": p.start_secs,
                "end_secs": p.end_secs,
                "duration_secs": p.duration_secs(),
                "event_inside": p.event_inside,
            }))
            .collect::<Vec<_>>(),
        // An estimate, and a property of how the file was made rather than a
        // fault in it: the quiet moments inside speech against the silence
        // beside them.
        "boundary_step_db": rounded(st.boundary_step_db, 2),
        "floor_dbfs": rounded(seg.floor_db, 2),
        "speech_threshold_dbfs": rounded(seg.threshold_db, 2),
        "sample_rate_hz": sample_rate,
    })
}

/// Every stretch of the file, in order.
pub fn segments_json(seg: &Segmentation) -> Value {
    json!(
        seg.segments
            .iter()
            .map(|s| json!({
                "kind": match s.kind {
                    Kind::Speech => "speech",
                    Kind::Silence => "silence",
                },
                "start_secs": s.start_secs,
                "end_secs": s.end_secs,
                "duration_secs": s.duration_secs(),
                "rms_dbfs": rounded(s.rms_db, 2),
                "rms_above_80hz_dbfs": rounded(s.rms_above_80hz_db, 2),
            }))
            .collect::<Vec<_>>()
    )
}

/// What is wrong with the file, where, and by how much.
pub fn defects_json(
    zero: &[ZeroRun],
    clicks: &[Click],
    seams: &[Seam],
    truncation: Truncation,
    speech_at: &dyn Fn(f64) -> bool,
    sample_rate: u32,
) -> Value {
    let sr = sample_rate.max(1) as f64;
    json!({
        "digital_silence": {
            "count": zero.len(),
            "total_secs": zero.iter().map(|z| z.len()).sum::<usize>() as f64 / sr,
            "runs": zero
                .iter()
                .map(|z| json!({
                    "start_secs": z.start_frame as f64 / sr,
                    "end_secs": z.end_frame as f64 / sr,
                    "duration_secs": z.len() as f64 / sr,
                }))
                .collect::<Vec<_>>(),
        },
        "clicks": clicks
            .iter()
            .map(|c| {
                let secs = c.frame as f64 / sr;
                json!({
                    "secs": secs,
                    "ratio_db": rounded(c.ratio_db, 1),
                    // Inside a word this is a candidate for a mouth click or a
                    // plosive; outside one it is an edit. The metric cannot
                    // tell them apart, so it says where it happened instead.
                    "in_speech": speech_at(secs),
                })
            })
            .collect::<Vec<_>>(),
        "seams": seams
            .iter()
            .map(|s| json!({
                "secs": s.frame as f64 / sr,
                "floor_step_db": rounded(s.floor_step_db, 2),
            }))
            .collect::<Vec<_>>(),
        "truncation": {
            "head": truncation.head,
            "tail": truncation.tail,
            "head_dbfs": rounded(truncation.head_db, 2),
            "tail_dbfs": rounded(truncation.tail_db, 2),
        },
    })
}

fn spread_json(s: Spread, places: i32) -> Value {
    json!({
        "median": rounded(s.median, places),
        "p10": rounded(s.p10, places),
        "p90": rounded(s.p90, places),
    })
}

fn tone_json(t: &Tone) -> Value {
    json!({
        "hz": rounded(t.hz, 1),
        "level_db": rounded(t.level_db, 1),
        "prominence_db": rounded(t.prominence_db, 1),
    })
}

/// What the spectrum says: shape, ceiling, and any tone standing out of it.
pub fn spectral_json(s: &Spectral) -> Value {
    json!({
        "window_size": s.window_size,
        "frames": s.frames,
        "centroid_hz": spread_json(s.centroid_hz, 1),
        "rolloff85_hz": spread_json(s.rolloff85_hz, 1),
        "rolloff95_hz": spread_json(s.rolloff95_hz, 1),
        // 0 is a pure tone, 1 is white noise.
        "flatness": spread_json(s.flatness, 4),
        "cutoff_hz": rounded(s.cutoff_hz, 1),
        // A heuristic with a stated threshold, not a proof: a ceiling below
        // three quarters of Nyquist with a wall in front of it.
        "transcode_suspect": s.transcode_suspect,
        "hum": s.hum.as_ref().map(tone_json),
        "hum_harmonics": s.hum_harmonics.iter().map(tone_json).collect::<Vec<_>>(),
        "bands_third_octave": s
            .bands
            .iter()
            .map(|b| json!({
                "centre_hz": rounded(b.centre_hz, 1),
                "level_db": rounded(b.level_db, 1),
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_named_for_how_many_there_are() {
        assert_eq!(channel_name(0, 1), "Mono");
        assert_eq!(channel_name(1, 2), "Right");
        assert_eq!(channel_name(4, 6), "Channel 5");
    }

    /// JSON has no infinity, and a silent file integrates to one.
    #[test]
    fn unmeasurable_values_are_null() {
        assert_eq!(rounded(f32::NEG_INFINITY, 2), Value::Null);
        assert_eq!(rounded(f32::NAN, 2), Value::Null);
        assert_eq!(rounded(-23.456, 2), json!(-23.46));
    }

    /// Nothing measured yet is a null block rather than an empty one, so a
    /// reader can tell "not analysed" from "analysed and silent".
    #[test]
    fn missing_stats_are_null_blocks() {
        assert_eq!(loudness_json(None), Value::Null);
        assert_eq!(channels_json(None, &|_| None), Value::Null);
    }

    /// The window's idea of a muted channel is the only thing that puts the
    /// key there; the CLI's report has no such notion and leaves it off.
    #[test]
    fn muted_only_where_there_is_a_window() {
        let stats = FileStats {
            channels: vec![ChannelStats::default(), ChannelStats::default()],
            ..Default::default()
        };
        let without = channels_json(Some(&stats), &|_| None);
        assert!(without[0].get("muted").is_none());
        let with = channels_json(Some(&stats), &|i| Some(i == 1));
        assert_eq!(with[0]["muted"], json!(false));
        assert_eq!(with[1]["muted"], json!(true));
    }
}
