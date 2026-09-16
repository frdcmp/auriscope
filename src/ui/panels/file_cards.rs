//! The sidebar cards describing the file itself: its name and size, the
//! format header, broadcast metadata, tags and cue markers. Everything here
//! is read once when the file is opened and does not change while it plays.

use eframe::egui;
use egui::{RichText, Sense};

use crate::ui::help::Topic;
use crate::ui::util::{
    fmt_bytes, fmt_int, fmt_system_time, fmt_time, lufs_str, reveal, short_codec,
};
use crate::ui::{App, fonts};

use super::theme::{ACCENT, KEY, VAL};
use super::widgets::{card, kv, kv_rows, mono, subhead};

pub(super) fn file_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let info = &audio.info;
    let ext = info
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase());
    card(
        ui,
        fonts::icon::FILE_AUDIO,
        "File",
        Topic::FileCard,
        ext,
        |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(info.file_name())
                        .family(fonts::bold())
                        .size(fonts::BODY)
                        .color(VAL),
                )
                .truncate(),
            )
            .on_hover_text(info.path.display().to_string());
            // Wrapped, not truncated: a deep path is worth two lines, and the
            // hover tooltip was the only way to read the tail of a cut one.
            // Clicking it hands the file to the desktop's file manager, which
            // is where a path on screen usually wants to be followed.
            let dir = ui
                .add(
                    egui::Label::new(RichText::new(info.directory()).small().color(KEY))
                        .wrap()
                        .sense(Sense::click()),
                )
                .on_hover_text("Show in the file manager")
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if dir.clicked() {
                reveal(&info.path);
            }
            ui.add_space(2.0);
            let layout = match info.channels {
                1 => "mono".to_string(),
                2 => "stereo".into(),
                n => format!("{n} ch"),
            };
            kv_rows(ui, |rows| {
                kv(rows, "Container", &info.container).help(Topic::Container);
                kv(rows, "Codec", short_codec(&info.codec))
                    .help(Topic::Codec)
                    .on_hover_text(&info.codec);
                kv(
                    rows,
                    "Sample rate",
                    format!("{} Hz", fmt_int(info.sample_rate as u64)),
                )
                .help(Topic::SampleRate);
                kv(rows, "Channels", format!("{} ({layout})", info.channels)).help(Topic::Channels);
                kv(
                    rows,
                    "Bit depth",
                    info.bits_per_sample
                        .map_or("—".into(), |b| format!("{b} bit")),
                )
                .help(Topic::BitDepth);
                kv(rows, "Duration", fmt_time(info.duration_secs())).help(Topic::Duration);
                kv(rows, "Frames", fmt_int(info.frames as u64)).help(Topic::Frames);
                kv(rows, "Size", fmt_bytes(info.file_size)).help(Topic::FileSize);
                if let Some(kbps) = info.bitrate_kbps() {
                    kv(rows, "Bit rate", format!("{kbps:.0} kb/s")).help(Topic::BitRate);
                }
                kv(
                    rows,
                    "In memory",
                    fmt_bytes((info.frames * info.channels * 4) as u64),
                )
                .help(Topic::InMemory);
                if let Some(m) = info.modified {
                    kv(rows, "Modified", fmt_system_time(m)).help(Topic::Modified);
                }
            });
        },
    );
}

pub(super) fn header_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let Some(wav) = &audio.info.wav else { return };
    let kind = if wav.rf64 { "RF64" } else { "RIFF" };
    card(
        ui,
        fonts::icon::LIST,
        "WAVE header",
        Topic::WaveHeaderCard,
        Some(format!("{kind} · {} chunks", wav.chunks.len())),
        |ui| {
            if let Some(f) = &wav.fmt {
                kv_rows(ui, |rows| {
                    kv(
                        rows,
                        "Format",
                        format!("{} (0x{:04X})", f.format_name(), f.format_tag),
                    )
                    .help(Topic::WavFormat);
                    kv(rows, "Block align", format!("{} B", f.block_align)).help(Topic::BlockAlign);
                    kv(
                        rows,
                        "Byte rate",
                        format!("{} kB/s", f.byte_rate as f64 / 1000.0),
                    )
                    .help(Topic::ByteRate);
                    if let Some(v) = f.valid_bits {
                        kv(rows, "Valid bits", format!("{v} of {}", f.bits_per_sample))
                            .help(Topic::ValidBits);
                    }
                    if let Some(m) = f.channel_mask {
                        kv(rows, "Channel mask", format!("0x{m:08X}")).help(Topic::ChannelMask);
                    }
                    kv(rows, "Body", fmt_bytes(wav.riff_size)).help(Topic::RiffBody);
                });
            }
            subhead(ui, "Chunks", Topic::Chunks);
            egui::Grid::new("chunks")
                .num_columns(3)
                .spacing([12.0, 2.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("chunk").small().color(KEY));
                    ui.label(RichText::new("offset").small().color(KEY));
                    ui.label(RichText::new("size").small().color(KEY));
                    ui.end_row();
                    for c in &wav.chunks {
                        let hot = c.id == "data";
                        let col = if hot { ACCENT } else { VAL };
                        ui.label(mono(&c.id).color(col));
                        ui.label(mono(format!("0x{:X}", c.offset)).color(KEY));
                        ui.label(mono(fmt_bytes(c.size)).color(col));
                        ui.end_row();
                    }
                });
        },
    );
}

pub(super) fn bwf_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let Some(b) = audio.info.wav.as_ref().and_then(|w| w.bext.as_ref()) else {
        return;
    };
    let sr = audio.info.sample_rate.max(1) as f64;
    card(
        ui,
        fonts::icon::TAG,
        "Broadcast Wave",
        Topic::BwfCard,
        Some(format!("bext v{}", b.version)),
        |ui| {
            if !b.description.is_empty() {
                ui.add(egui::Label::new(RichText::new(&b.description).color(VAL)).wrap());
                ui.add_space(2.0);
            }
            kv_rows(ui, |rows| {
                if !b.originator.is_empty() {
                    kv(rows, "Originator", &b.originator).help(Topic::BwfOriginator);
                }
                if !b.originator_reference.is_empty() {
                    kv(rows, "Reference", &b.originator_reference).help(Topic::BwfReference);
                }
                if !b.origination_date.is_empty() || !b.origination_time.is_empty() {
                    kv(
                        rows,
                        "Originated",
                        format!("{} {}", b.origination_date, b.origination_time),
                    )
                    .help(Topic::BwfOriginated);
                }
                let tr = b.time_reference as f64 / sr;
                kv(
                    rows,
                    "Time ref",
                    format!("{} ({} smp)", fmt_time(tr), fmt_int(b.time_reference)),
                )
                .help(Topic::BwfTimeRef);
                if let Some(u) = &b.umid {
                    kv(rows, "UMID", format!("{}…", &u[..u.len().min(16)])).help(Topic::BwfUmid);
                }
                if let Some(l) = b.loudness {
                    kv(rows, "Integrated", lufs_str(l.integrated_lufs))
                        .help(Topic::BwfStoredLoudness);
                    kv(rows, "Range", format!("{:+.1} LU", l.range_lu))
                        .help(Topic::BwfStoredLoudness);
                    kv(
                        rows,
                        "Max true pk",
                        format!("{:+.1} dBTP", l.max_true_peak_dbtp),
                    )
                    .help(Topic::BwfStoredLoudness);
                }
            });
            if !b.coding_history.is_empty() {
                ui.collapsing(RichText::new("Coding history").small().color(KEY), |ui| {
                    for line in b.coding_history.lines().filter(|l| !l.trim().is_empty()) {
                        ui.add(egui::Label::new(mono(line.trim()).size(fonts::SMALL)).wrap());
                    }
                });
            }
        },
    );
}

pub(super) fn tags_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let info = &audio.info;
    let riff_info: &[(String, String)] = info.wav.as_ref().map_or(&[], |w| &w.info);
    let n = riff_info.len() + info.tags.len();
    if n == 0 {
        return;
    }
    card(
        ui,
        fonts::icon::TAG,
        "Tags",
        Topic::TagsCard,
        Some(n.to_string()),
        |ui| {
            kv_rows(ui, |rows| {
                for (k, v) in riff_info.iter().chain(&info.tags) {
                    kv(rows, k, v);
                }
            });
        },
    );
}

pub(super) fn markers_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let Some(wav) = &audio.info.wav else { return };
    if wav.cues.is_empty() {
        return;
    }
    let sr = audio.info.sample_rate.max(1) as f64;
    card(
        ui,
        fonts::icon::MARKER,
        "Markers",
        Topic::MarkersCard,
        Some(wav.cues.len().to_string()),
        |ui| {
            egui::Grid::new("cues")
                .num_columns(3)
                .spacing([12.0, 2.0])
                .striped(true)
                .show(ui, |ui| {
                    for c in &wav.cues {
                        ui.label(mono(format!("{}", c.id)).color(KEY));
                        ui.label(mono(fmt_time(c.position as f64 / sr)));
                        ui.add(
                            egui::Label::new(
                                RichText::new(c.label.clone().unwrap_or_default()).color(VAL),
                            )
                            .truncate(),
                        );
                        ui.end_row();
                    }
                });
        },
    );
}
