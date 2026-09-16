//! Saving the views as a picture: what the camera button in the transport bar
//! sets in motion, the PNG it writes, and the JSON that goes beside it.
//!
//! What is kept is the waveform, the spectrogram and the spectrum — the panels
//! the file is *drawn* in — cropped out of a grab of the whole window. The
//! bars, the sidebar and the settings dialog are chrome, and a picture of the
//! signal is more use without them. Everything the sidebar would have said
//! goes into a `.json` of the same name instead, where it can be read by
//! something other than a person.
//!
//! A capture cannot be taken on the spot. egui asks the render backend for the
//! pixels of a frame and they come back as an event a frame or two later, so a
//! capture is a small state machine rather than a function call: pick the file,
//! draw one clean frame, grab it, write it. [`Capture`] is where the chosen
//! path waits meanwhile, and its variant says which beat we are on.
//!
//! The clean frame is not fussiness. The click that starts a capture leaves the
//! button hovered with its tooltip coming up, and the frame after the file
//! dialog closes still has both; grabbing that one would put a tooltip in the
//! middle of the saved picture. `App` suppresses the tooltip for as long as a
//! capture is pending, and the machine skips a frame so that suppression has
//! landed before the grab goes out.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use eframe::egui;
use serde_json::{Value, json};

use auriscope::analysis::WindowKind;

use super::views::channel_name;
use super::{App, update};

/// A picture on its way to disk, one frame per beat.
pub enum Capture {
    /// The frame the path was picked in: it still carries the button's hover
    /// state and tooltip, so it is not the one to keep.
    Settling(PathBuf),
    /// The frame drawn without them, and the one the grab is asked for at the
    /// end of.
    Grabbing(PathBuf),
    /// Asked for; waiting for the pixels to come back as an event.
    Waiting {
        path: PathBuf,
        /// Where the views were when the grab went out, which is the layout
        /// the pixels belong to rather than whatever is on screen when they
        /// arrive.
        views: egui::Rect,
        /// When it was asked for, so a backend that never answers is given up
        /// on rather than waited for forever.
        asked: std::time::Instant,
    },
}

/// How long to wait for the pixels before calling it a day. Generous: it is a
/// frame or two in practice, and the only thing this catches is a backend that
/// cannot take a screenshot at all.
pub const GRAB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// What the save dialog opens with: the audio file's own name with a `.png` on
/// it, so a capture lands beside the file it is of and says what it shows.
pub fn default_name(source: Option<&Path>) -> String {
    let stem = source
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "auriscope".into());
    format!("{stem}.png")
}

/// `.png` on the end of whatever the dialog came back with. GTK's save dialog
/// hands back exactly what was typed, filter or no filter, and a file named
/// after nothing in particular is not obviously an image to anything else.
pub fn with_png_extension(path: PathBuf) -> PathBuf {
    match path.extension() {
        Some(e) if e.eq_ignore_ascii_case("png") => path,
        _ => {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".png");
            path.with_file_name(name)
        }
    }
}

/// Write a captured frame out as a PNG.
///
/// Three channels, not four: the window is opaque, so the alpha carries no
/// information, and a viewer that reads it differently cannot turn the picture
/// blank. egui's pixels are premultiplied, which at full alpha is the same
/// bytes anyway.
pub fn write_png(path: &Path, img: &egui::ColorImage) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        img.width() as u32,
        img.height() as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut rgb = Vec::with_capacity(img.pixels.len() * 3);
    for px in &img.pixels {
        rgb.extend_from_slice(&[px.r(), px.g(), px.b()]);
    }
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&rgb))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Where the metadata goes: the picture's name with `.json` on it, so the two
/// travel together and neither has to be looked up from the other.
pub fn json_path(png: &Path) -> PathBuf {
    png.with_extension("json")
}

/// Cut `region` — in points, as egui lays panels out — out of a grab of the
/// whole window, which comes in physical pixels.
///
/// Clamped rather than trusted: at fractional scaling a panel's rect and the
/// backend's idea of the window can disagree by a pixel, and an empty or
/// impossible region means the whole grab is the honest answer.
pub fn crop(img: &egui::ColorImage, region: egui::Rect, pixels_per_point: f32) -> egui::ColorImage {
    let (w, h) = (img.width(), img.height());
    let px = |v: f32, max: usize| (v * pixels_per_point).round().clamp(0.0, max as f32) as usize;
    let (x0, x1) = (px(region.left(), w), px(region.right(), w));
    let (y0, y1) = (px(region.top(), h), px(region.bottom(), h));
    if !region.is_positive() || x1 <= x0 || y1 <= y0 {
        return img.clone();
    }
    let mut pixels = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for row in y0..y1 {
        pixels.extend_from_slice(&img.pixels[row * w + x0..row * w + x1]);
    }
    egui::ColorImage::new([x1 - x0, y1 - y0], pixels)
}

/// Write the metadata out, indented: it is meant to be read as well as parsed.
pub fn write_json(path: &Path, value: &Value) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(std::io::BufWriter::new(file), value)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Everything the sidebar knows about the open file, plus how the picture next
/// to it was framed: what the file is, what its header holds, what it measures,
/// and the analysis and view the capture was taken through.
pub fn metadata(app: &App, image: &Path) -> Value {
    json!({
        "auriscope": {
            "version": update::CURRENT,
            "captured_unix": update::unix_now(),
            "image": image.file_name().map(|n| n.to_string_lossy().into_owned()),
        },
        "file": file_json(app),
        "wave": wave_json(app),
        "broadcast_wave": bext_json(app),
        "markers": markers_json(app),
        "loudness": loudness_json(app),
        "channels": channels_json(app),
        "view": view_json(app),
        "analysis": analysis_json(app),
    })
}

fn file_json(app: &App) -> Value {
    let Some(info) = app.audio.as_ref().map(|a| &a.info) else {
        return Value::Null;
    };
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

fn wave_json(app: &App) -> Value {
    let Some(wav) = app.audio.as_ref().and_then(|a| a.info.wav.as_ref()) else {
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

fn bext_json(app: &App) -> Value {
    let Some(bext) = app
        .audio
        .as_ref()
        .and_then(|a| a.info.wav.as_ref())
        .and_then(|w| w.bext.as_ref())
    else {
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

fn markers_json(app: &App) -> Value {
    let Some(audio) = &app.audio else {
        return Value::Null;
    };
    let Some(wav) = audio.info.wav.as_ref() else {
        return Value::Null;
    };
    let sr = audio.info.sample_rate.max(1) as f64;
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

fn loudness_json(app: &App) -> Value {
    let Some(stats) = &app.stats else {
        return Value::Null;
    };
    let fold = |f: fn(&auriscope::analysis::ChannelStats) -> f32| {
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
        // The two the Loudness card works out for itself, so a reader of the
        // JSON does not have to know how they were arrived at.
        "headroom_db": rounded(-true_peak, 2),
        "crest_factor_db": rounded(peak - rms, 2),
    })
}

fn channels_json(app: &App) -> Value {
    let Some(stats) = &app.stats else {
        return Value::Null;
    };
    let nch = stats.channels.len();
    json!(
        stats
            .channels
            .iter()
            .enumerate()
            .map(|(i, c)| json!({
                "index": i,
                "name": channel_name(i, nch),
                "sample_peak_dbfs": rounded(c.sample_peak_db, 2),
                "true_peak_dbtp": rounded(c.true_peak_dbtp, 2),
                "rms_dbfs": rounded(c.rms_db, 2),
                "dc_offset": rounded(c.dc_offset, 6),
                "clipped_samples": c.clipped_samples,
                "clipped_runs": c.clipped_runs,
                "muted": app.channel_muted(i),
            }))
            .collect::<Vec<_>>()
    )
}

/// What the picture actually shows: the span of the file drawn across it, and
/// the range on the ruler if one is set. Without this the image is a stretch
/// of audio with no way back to where it came from.
fn view_json(app: &App) -> Value {
    let sr = app.sample_rate();
    let range = app.range.map(|(a, b)| {
        json!({
            "start_secs": a.min(b) / sr,
            "end_secs": a.max(b) / sr,
            "looping": app.loop_enabled,
        })
    });
    json!({
        "start_secs": app.view.start / sr,
        "end_secs": app.view.end / sr,
        "start_frames": app.view.start.round() as i64,
        "end_frames": app.view.end.round() as i64,
        "playhead_secs": app
            .engine
            .as_ref()
            .map(|e| e.playhead() as f64 / sr),
        "range": range,
        "isolated_channel": app.isolated,
        "soloed_channel": app.solo,
    })
}

/// How the spectrogram in the picture was computed and coloured — the numbers
/// that decide what it shows, so the same picture can be made again.
fn analysis_json(app: &App) -> Value {
    let s = &app.settings;
    let stft = &s.stft;
    json!({
        "window_size": stft.window_size,
        "window": window_name(stft.window),
        "overlap": format!("{}/{}", stft.overlap_num, stft.overlap_den),
        "hop": stft.hop(),
        "reassigned": stft.reassign,
        "db_floor": rounded(s.db_min, 1),
        "db_ceiling": rounded(s.db_max, 1),
        "contrast": rounded(s.spec_contrast, 2),
        "log_frequency": s.log_frequency,
        "min_hz": rounded(s.min_hz, 1),
        "colormap": format!("{:?}", s.colormap),
        "panes": {
            "waveform": s.show_waveform,
            "spectrogram": s.show_spectrogram,
            "spectrum": s.show_spectrum,
            "merged": s.merge_views,
        },
    })
}

fn window_name(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Rectangular => "Rectangular",
        WindowKind::Hann => "Hann",
        WindowKind::Hamming => "Hamming",
        WindowKind::Blackman => "Blackman",
        WindowKind::BlackmanHarris => "Blackman-Harris",
    }
}

/// A number for JSON, which has no infinity: `null` where a measurement has
/// none — a silent file integrates to −inf LUFS — and otherwise the value at
/// the precision it is read at, rather than whatever an f32 widens to.
fn rounded(v: f32, places: i32) -> Value {
    if !v.is_finite() {
        return Value::Null;
    }
    let f = 10f64.powi(places);
    json!(((v as f64) * f).round() / f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_follows_the_audio_file() {
        assert_eq!(
            default_name(Some(Path::new("/takes/scene 4 take 02.wav"))),
            "scene 4 take 02.png"
        );
        assert_eq!(default_name(None), "auriscope.png");
    }

    /// The bytes on disk are a PNG anything else can open, at the size and
    /// the colours that went in.
    #[test]
    fn png_round_trips() {
        let path =
            std::env::temp_dir().join(format!("auriscope-capture-{}.png", std::process::id()));
        let pixels = vec![
            egui::Color32::from_rgb(10, 20, 30),
            egui::Color32::from_rgb(200, 100, 50),
            egui::Color32::BLACK,
            egui::Color32::WHITE,
        ];
        let img = egui::ColorImage::new([2, 2], pixels.clone());
        write_png(&path, &img).expect("write");

        let file = std::io::BufReader::new(std::fs::File::open(&path).expect("open"));
        let decoder = png::Decoder::new(file);
        let mut reader = decoder.read_info().expect("header");
        let mut buf = vec![0; reader.output_buffer_size().expect("size")];
        let info = reader.next_frame(&mut buf).expect("frame");
        std::fs::remove_file(&path).ok();

        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        let want: Vec<u8> = pixels.iter().flat_map(|p| [p.r(), p.g(), p.b()]).collect();
        assert_eq!(&buf[..info.buffer_size()], &want[..]);
    }

    /// The crop is in points against an image in pixels, and a region that
    /// runs off the frame is cut to it rather than read past the end.
    #[test]
    fn crop_takes_the_region_and_clamps_it() {
        // 4x4 pixels at 2x scaling: 2x2 points.
        let px = |i: u8| egui::Color32::from_gray(i);
        let img = egui::ColorImage::new([4, 4], (0..16).map(|i| px(i as u8)).collect());

        let top_left = crop(
            &img,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            2.0,
        );
        assert_eq!(top_left.size, [2, 2]);
        assert_eq!(top_left.pixels, vec![px(0), px(1), px(4), px(5)]);

        let past_the_edge = crop(
            &img,
            egui::Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(9.0, 9.0)),
            2.0,
        );
        assert_eq!(past_the_edge.size, [2, 2]);
        assert_eq!(past_the_edge.pixels, vec![px(10), px(11), px(14), px(15)]);

        // Nothing laid out yet: the whole grab beats an empty file.
        let nothing = crop(&img, egui::Rect::NOTHING, 2.0);
        assert_eq!(nothing.size, [4, 4]);
    }

    #[test]
    fn metadata_sits_beside_the_picture() {
        assert_eq!(
            json_path(Path::new("/takes/scene 4.png")),
            PathBuf::from("/takes/scene 4.json")
        );
    }

    #[test]
    fn extension_added_once() {
        let p = |s: &str| with_png_extension(PathBuf::from(s));
        assert_eq!(p("/tmp/shot"), PathBuf::from("/tmp/shot.png"));
        assert_eq!(p("/tmp/shot.png"), PathBuf::from("/tmp/shot.png"));
        assert_eq!(p("/tmp/shot.PNG"), PathBuf::from("/tmp/shot.PNG"));
        // A name with a full stop in it keeps it: "take.2" is not an extension
        // anyone meant, but it is part of the name they typed.
        assert_eq!(p("/tmp/take.2"), PathBuf::from("/tmp/take.2.png"));
    }
}
