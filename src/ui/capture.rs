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

use auriscope::report::{self, rounded};

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
    let info = app.audio.as_ref().map(|a| &a.info);
    let block = |f: fn(&auriscope::audio::FileInfo) -> Value| info.map_or(Value::Null, f);
    json!({
        "auriscope": {
            "version": update::CURRENT,
            "captured_unix": update::unix_now(),
            "image": image.file_name().map(|n| n.to_string_lossy().into_owned()),
        },
        "file": block(report::file_json),
        "wave": block(report::wave_json),
        "broadcast_wave": block(report::bext_json),
        "markers": block(report::markers_json),
        "loudness": report::loudness_json(app.stats.as_ref()),
        // The window is the only caller that knows about muting, so it is the
        // only one whose channels say whether they were.
        "channels": report::channels_json(app.stats.as_ref(), &|i| Some(app.channel_muted(i))),
        "view": view_json(app),
        "analysis": analysis_json(app),
    })
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
        "window": stft.window.name(),
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
