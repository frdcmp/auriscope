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

use crate::analysis::{ChannelStats, FileStats};
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
                });
                if let (Some(obj), Some(m)) = (v.as_object_mut(), muted(i)) {
                    obj.insert("muted".into(), json!(m));
                }
                v
            })
            .collect::<Vec<_>>()
    )
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
