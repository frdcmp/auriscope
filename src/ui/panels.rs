//! Transport bar, side panel (file, loudness, view settings) and status bar.

use eframe::egui;
use egui::{
    Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Shape, Stroke, pos2,
    vec2,
};

use auriscope::analysis::{ColorMap, StftParams, WindowKind};

use super::fonts;
use super::help::{self, Topic};
use super::update;
use super::util::{
    db_str, file_label, fmt_bytes, fmt_hz_unit, fmt_int, fmt_ms, fmt_system_time, fmt_time,
    lufs_str, reveal, short_codec,
};
use super::views::{V_ZOOM_MAX, V_ZOOM_MIN, channel_name};
use super::{App, RECENT_MAX};

mod chrome;
pub(super) mod theme;
mod transport;
mod widgets;

pub use chrome::{resize_borders, title_bar};
use theme::{ACCENT, BAD, CARD_BG, GOOD, KEY, SIDE_BG, VAL, WARN};
pub use transport::top_bar;
use widgets::{
    CARD_HEAD_BG, CARD_HEAD_RULE, card, kv, kv_colored, kv_rows, mono, subhead, wide_card,
};
pub fn status_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::bottom("status").show(root, |ui| {
        ui.horizontal(|ui| {
            if let Some(job) = &app.job {
                ui.add(
                    egui::ProgressBar::new(job.progress)
                        .desired_width(160.0)
                        .text(job.stage.clone()),
                );
                ui.separator();
            }
            if let Some(err) = &app.error {
                ui.colored_label(Color32::from_rgb(255, 110, 110), err);
                ui.separator();
            }
            if let Some(e) = &app.engine {
                let under = e
                    .shared
                    .underruns
                    .load(std::sync::atomic::Ordering::Relaxed);
                let mut s = format!(
                    "{} · {} Hz · {} ch · {:?}",
                    e.device_name, e.device_rate, e.device_channels, e.sample_format
                );
                if e.resampling {
                    s.push_str(" · resampling");
                }
                if under > 0 {
                    s.push_str(&format!(" · {under} underruns"));
                }
                ui.label(RichText::new(s).small().color(Color32::from_gray(150)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(&app.hover_info).monospace());
            });
        });
    });
}

pub fn side_panel(app: &mut App, root: &mut egui::Ui) {
    // Bound the width against the window. Panel widths are persisted, so an
    // unbounded one that goes wrong once stays wrong across restarts.
    let max_w = (root.ctx().viewport_rect().width() * 0.45).max(240.0);
    egui::Panel::right("side")
        .resizable(true)
        .default_size(320.0)
        .min_size(240.0)
        .max_size(max_w)
        .frame(
            egui::Frame::new()
                .fill(SIDE_BG)
                .inner_margin(Margin::symmetric(8, 8)),
        )
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                // No scrollbar: it ate width from an already narrow column and
                // the wheel scrolls the cards just the same.
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if app.audio.is_none() {
                        empty_card(ui);
                        return;
                    }
                    file_card(app, ui);
                    header_card(app, ui);
                    bwf_card(app, ui);
                    tags_card(app, ui);
                    markers_card(app, ui);
                    loudness_card(app, ui);
                    levels_card(app, ui);
                    analysis_card(app, ui);
                    cursor_card(app, ui);
                });
        });
}

/// Horizontal level bar on a −60…0 dB scale with a value readout. `dim` is
/// for a channel that is not being heard: bar, label and number all go grey.
fn level_bar(ui: &mut egui::Ui, label: &str, db: f32, color: Color32, topic: Topic, dim: bool) {
    let (color, key, val) = if dim {
        (dull(color), dull(KEY), dull(VAL))
    } else {
        (color, KEY, VAL)
    };
    let h = 15.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let label_w = 34.0;
    let val_w = 52.0;
    let bar = Rect::from_min_max(
        pos2(rect.left() + label_w, rect.top() + 3.0),
        pos2(rect.right() - val_w, rect.bottom() - 3.0),
    );
    p.rect_filled(bar, 2.0, Color32::from_gray(if dim { 36 } else { 44 }));
    let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    if t > 0.0 {
        let fill = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * t, bar.bottom()));
        p.rect_filled(fill, 2.0, color);
    }
    for tick in [-48.0f32, -36.0, -24.0, -12.0, -6.0, -3.0] {
        let x = bar.left() + bar.width() * (tick + 60.0) / 60.0;
        p.vline(
            x,
            bar.y_range(),
            Stroke::new(1.0, Color32::from_black_alpha(110)),
        );
    }
    let label_at = p.text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(fonts::SMALL, egui::FontFamily::Proportional),
        key,
    );
    help::offer(ui, label_at, rect, topic);
    p.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        if db.is_finite() {
            format!("{db:.1}")
        } else {
            "—".into()
        },
        FontId::monospace(fonts::SMALL),
        val,
    );
}

/// Pull a colour to grey and darken it: how a channel's readouts are drawn
/// when that channel is not being heard — muted, soloed out, or left behind
/// by another channel picked out on its own. Dull rather than hidden, because
/// the numbers are still true, they are just not what is playing.
fn dull(c: Color32) -> Color32 {
    let lum = c.r() as f32 * 0.30 + c.g() as f32 * 0.59 + c.b() as f32 * 0.11;
    let v = (lum * 0.55) as u8;
    Color32::from_gray(v)
}

fn peak_color(dbtp: f32) -> Color32 {
    if dbtp > -1.0 {
        BAD
    } else if dbtp > -3.0 {
        WARN
    } else {
        ACCENT
    }
}

// ---- Cards -----------------------------------------------------------------

fn empty_card(ui: &mut egui::Ui) {
    card(
        ui,
        fonts::icon::INFO,
        "No file",
        Topic::FileCard,
        None,
        |ui| {
            ui.label(
                RichText::new("Drop an audio file on the window, or press Ctrl+O.")
                    .small()
                    .color(KEY),
            );
        },
    );
}

fn file_card(app: &App, ui: &mut egui::Ui) {
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

fn header_card(app: &App, ui: &mut egui::Ui) {
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

fn bwf_card(app: &App, ui: &mut egui::Ui) {
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

fn tags_card(app: &App, ui: &mut egui::Ui) {
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

fn markers_card(app: &App, ui: &mut egui::Ui) {
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

fn loudness_card(app: &App, ui: &mut egui::Ui) {
    let trailing = app.stats.is_none().then(|| "measuring…".to_string());
    card(
        ui,
        fonts::icon::GAUGE,
        "Loudness",
        Topic::LoudnessCard,
        trailing,
        |ui| {
            let Some(stats) = &app.stats else {
                return;
            };
            kv_rows(ui, |rows| {
                kv(rows, "Integrated", lufs_str(stats.integrated_lufs)).help(Topic::Integrated);
                kv(rows, "Range", format!("{:+.1} LU", stats.loudness_range_lu))
                    .help(Topic::LoudnessRange);
                kv(rows, "Max momentary", lufs_str(stats.max_momentary_lufs))
                    .help(Topic::MaxMomentary);
                kv(rows, "Max short-term", lufs_str(stats.max_short_term_lufs))
                    .help(Topic::MaxShortTerm);
                if let Some(c) = stats.correlation {
                    kv(rows, "Correlation", format!("{c:+.2}")).help(Topic::Correlation);
                }
            });
            if !stats.integrated_lufs.is_finite() {
                return;
            }
            subhead(ui, "Against targets", Topic::AgainstTargets);
            kv_rows(ui, |rows| {
                for (name, target) in [
                    ("EBU R128 −23 LUFS", -23.0f32),
                    ("Podcast −16 LUFS", -16.0),
                    ("Streaming −14 LUFS", -14.0),
                ] {
                    let d = stats.integrated_lufs - target;
                    let color = if d.abs() <= 1.0 {
                        GOOD
                    } else if d.abs() <= 3.0 {
                        WARN
                    } else {
                        VAL
                    };
                    kv_colored(rows, name, format!("{d:+.1} LU"), color).help(Topic::TargetDelta);
                }
                let tp = stats
                    .channels
                    .iter()
                    .map(|c| c.true_peak_dbtp)
                    .fold(f32::NEG_INFINITY, f32::max);
                let pk = stats
                    .channels
                    .iter()
                    .map(|c| c.sample_peak_db)
                    .fold(f32::NEG_INFINITY, f32::max);
                let rms = stats
                    .channels
                    .iter()
                    .map(|c| c.rms_db)
                    .fold(f32::NEG_INFINITY, f32::max);
                kv_colored(rows, "Headroom", db_str(-tp), peak_color(tp)).help(Topic::Headroom);
                kv(rows, "Crest factor", db_str(pk - rms)).help(Topic::CrestFactor);
            });
        },
    );
}

/// Mute and solo, lit the way a mixer strip lights them: dark while off, the
/// colour of the function while on.
const MUTE_LIT: Color32 = Color32::from_rgb(214, 74, 74);
const SOLO_LIT: Color32 = Color32::from_rgb(235, 186, 62);
/// The pill behind a channel name that is being drawn on its own: the accent
/// at a wash, so the name still reads as a name and not as a third button.
const ISOLATE_BG: Color32 = Color32::from_rgba_premultiplied(24, 43, 59, 70);
/// Hover feedback on anything that paints its own background.
const HOVER_WASH: Color32 = Color32::from_rgba_premultiplied(16, 16, 16, 16);

/// A latching mixer button. `dim` greys an unlit one, for a channel that is
/// not being heard; a lit one keeps its colour, since it is often the reason
/// the channel is not being heard. Returns true on the click that toggles it.
fn mixer_button(
    ui: &mut egui::Ui,
    on: bool,
    dim: bool,
    text: &str,
    lit: Color32,
    tip: &str,
) -> bool {
    let (fill, fg, stroke) = if on {
        (lit, Color32::from_gray(20), Stroke::new(1.0, lit))
    } else if dim {
        (CARD_BG, dull(KEY), Stroke::new(1.0, Color32::from_gray(48)))
    } else {
        (CARD_HEAD_BG, KEY, Stroke::new(1.0, CARD_HEAD_RULE))
    };
    let resp = ui
        .add(
            egui::Button::new(
                RichText::new(text)
                    .size(fonts::SMALL)
                    .family(fonts::bold())
                    .color(fg),
            )
            .fill(fill)
            .stroke(stroke)
            .corner_radius(4.0)
            .min_size(vec2(44.0, 19.0)),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip);
    // An explicit fill overrides egui's own hover shading, so the feedback is
    // painted back on top.
    if resp.hovered() {
        ui.painter().rect_filled(resp.rect, 4.0, HOVER_WASH);
    }
    resp.clicked()
}

fn levels_card(app: &mut App, ui: &mut egui::Ui) {
    let Some(audio) = app.audio.clone() else {
        return;
    };
    let nch = audio.channels.len();
    let mut changed = false;
    card(
        ui,
        fonts::icon::BARS,
        "Levels",
        Topic::LevelsCard,
        None,
        |ui| {
            let Some(stats) = app.stats.clone() else {
                ui.label(RichText::new("measuring…").small().color(KEY));
                return;
            };
            for ch in 0..nch {
                let Some(cs) = stats.channels.get(ch) else {
                    continue;
                };
                if ch > 0 {
                    ui.add_space(4.0);
                }
                // Everything about a channel nobody is hearing is drawn back,
                // so the strip that is playing is the one the eye lands on.
                let dim = app.channel_muted(ch);
                ui.horizontal(|ui| {
                    // The channel's name, then its two controls spelled out:
                    // "L M S" over an RMS row read as one more abbreviation.
                    // The name is itself a control: click it to draw that
                    // channel alone. Its pill is painted once the text has
                    // been laid out, so it lands behind rather than over it.
                    let name = channel_name(ch, nch);
                    let alone = app.isolated == Some(ch);
                    let pill = ui.painter().add(Shape::Noop);
                    let mut label = egui::Label::new(
                        RichText::new(&name)
                            .family(fonts::bold())
                            .size(fonts::BODY)
                            .color(if dim { dull(ACCENT) } else { ACCENT }),
                    );
                    if nch > 1 {
                        label = label.sense(Sense::click());
                    }
                    let resp = ui.add(label);
                    if nch > 1 {
                        let resp = resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(if alone {
                                "Back to every channel".to_string()
                            } else {
                                format!("{name} alone, as if it were a mono file")
                            });
                        help::offer_response(ui, &resp, Topic::ChannelIsolate);
                        if resp.clicked() {
                            app.isolated = if alone { None } else { Some(ch) };
                            // Two ways of asking to hear one channel; holding
                            // both at once would only be a way to disagree.
                            app.solo = None;
                            changed = true;
                        }
                        if alone || resp.hovered() {
                            ui.painter().set(
                                pill,
                                Shape::rect_filled(
                                    resp.rect.expand2(vec2(5.0, 2.0)),
                                    4.0,
                                    if alone { ISOLATE_BG } else { HOVER_WASH },
                                ),
                            );
                        }
                        ui.add_space(4.0);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let muted = app.mutes[ch];
                        if mixer_button(ui, muted, dim, "Mute", MUTE_LIT, "Silence this channel") {
                            app.mutes[ch] = !muted;
                            changed = true;
                        }
                        let soloed = app.solo == Some(ch);
                        if mixer_button(
                            ui,
                            soloed,
                            dim,
                            "Solo",
                            SOLO_LIT,
                            "Hear this channel alone, in its own speaker",
                        ) {
                            app.solo = if soloed { None } else { Some(ch) };
                            app.isolated = None;
                            changed = true;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let note = if cs.clipped_samples > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "{} {} clipped · {} runs",
                                    fonts::icon::WARN,
                                    fmt_int(cs.clipped_samples as u64),
                                    cs.clipped_runs
                                ))
                                .small()
                                .color(if dim {
                                    dull(BAD)
                                } else {
                                    BAD
                                }),
                            )
                        } else {
                            let c = if dim { dull(KEY) } else { KEY };
                            ui.label(RichText::new("no clipping").small().color(c))
                        };
                        help::offer_response(ui, &note, Topic::Clipping);
                    });
                });
                let peak = peak_color(cs.true_peak_dbtp);
                level_bar(ui, "Peak", cs.sample_peak_db, peak, Topic::PeakLevel, dim);
                level_bar(ui, "TP", cs.true_peak_dbtp, peak, Topic::TruePeakLevel, dim);
                let rms = Color32::from_rgb(96, 118, 172);
                level_bar(ui, "RMS", cs.rms_db, rms, Topic::RmsLevel, dim);
                let dc_c = if cs.dc_offset.abs() > 0.01 { WARN } else { KEY };
                let dc = ui.label(
                    RichText::new(format!("DC offset {:+.4}", cs.dc_offset))
                        .small()
                        .color(if dim { dull(dc_c) } else { dc_c }),
                );
                help::offer_response(ui, &dc, Topic::DcOffset);
            }
        },
    );
    if changed {
        app.apply_channel_routing();
    }
}

fn analysis_card(app: &App, ui: &mut egui::Ui) {
    let sr = app.sample_rate().max(1.0);
    let st = &app.settings.stft;
    let hop = st.hop();
    card(
        ui,
        fonts::icon::COGS,
        "Analysis",
        Topic::AnalysisCard,
        Some(format!("{:?}", app.settings.colormap)),
        |ui| {
            kv_rows(ui, |rows| {
                kv(
                    rows,
                    "Window",
                    format!(
                        "{} {:?} · {}",
                        st.window_size,
                        st.window,
                        st.overlap_label()
                    ),
                )
                .help(Topic::AnWindow);
                kv(
                    rows,
                    "Hop",
                    format!("{hop} smp · {:.1} ms", hop as f64 / sr * 1000.0),
                )
                .help(Topic::AnHop);
                kv(
                    rows,
                    "Resolution",
                    format!(
                        "{:.1} Hz · {:.1} ms",
                        sr / st.window_size as f64,
                        st.window_size as f64 / sr * 1000.0
                    ),
                )
                .help(Topic::Resolution);
                kv(rows, "Reassignment", if st.reassign { "on" } else { "off" })
                    .help(Topic::Reassignment);
                let tile = app.detail.iter().flatten().next();
                kv(
                    rows,
                    "Detail tile",
                    tile.map_or("—".into(), |t| {
                        format!("{} cols · {:.2} smp", fmt_int(t.columns as u64), t.stride)
                    }),
                )
                .help(Topic::DetailTile);
                let span = app.view.len() / sr;
                kv(
                    rows,
                    "View",
                    format!(
                        "{} – {}",
                        fmt_time(app.view.start / sr),
                        fmt_time(app.view.end / sr)
                    ),
                )
                .help(Topic::ViewRange);
                kv(rows, "Span", format!("{span:.3} s")).help(Topic::ViewSpan);
                kv(
                    rows,
                    "Floor / ceiling",
                    format!("{:.0} / {:.0} dB", app.settings.db_min, app.settings.db_max),
                )
                .help(Topic::FloorCeiling);
            });
        },
    );
}

fn cursor_card(app: &App, ui: &mut egui::Ui) {
    let sr = app.sample_rate().max(1.0);
    card(
        ui,
        fonts::icon::CLOCK,
        "Cursor",
        Topic::CursorCard,
        None,
        |ui| {
            kv_rows(ui, |rows| {
                let ph = app
                    .engine
                    .as_ref()
                    .map_or(0.0, |e| e.playhead() as f64 / sr);
                kv(rows, "Playhead", fmt_time(ph)).help(Topic::Playhead);
                kv(
                    rows,
                    "Pointer",
                    if app.hover_info.is_empty() {
                        "—".to_string()
                    } else {
                        app.hover_info.clone()
                    },
                )
                .help(Topic::PointerReadout);
                match app.selection {
                    Some((a, b)) if (a - b).abs() >= 1.0 => {
                        let (a, b) = (a.min(b) / sr, a.max(b) / sr);
                        kv(
                            rows,
                            "Selection",
                            format!("{} – {} ({:.3} s)", fmt_time(a), fmt_time(b), b - a),
                        )
                        .help(Topic::Selection);
                    }
                    _ => {
                        kv(rows, "Selection", "—").help(Topic::Selection);
                    }
                }
                match app.range {
                    Some((a, b)) => {
                        let (a, b) = (a / sr, b / sr);
                        kv(
                            rows,
                            "Range",
                            format!("{} – {} ({:.3} s)", fmt_time(a), fmt_time(b), b - a),
                        )
                        .help(Topic::RulerRange);
                        kv_colored(
                            rows,
                            "Loop",
                            if app.loop_enabled { "on" } else { "off" },
                            if app.loop_enabled { GOOD } else { VAL },
                        )
                        .help(Topic::LoopToggle);
                    }
                    None => {
                        kv(rows, "Range", "—").help(Topic::RulerRange);
                    }
                }
            });
        },
    );
}

/// A key drawn as a little keycap, for the hints that sit beside a control.
fn keycap(ui: &mut egui::Ui, key: &str) {
    egui::Frame::new()
        .fill(CARD_HEAD_BG)
        .stroke(Stroke::new(1.0, CARD_HEAD_RULE))
        .corner_radius(3.0)
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(key).monospace().size(fonts::SMALL).color(VAL));
        });
}

/// The settings dialog: what to show, and every control for the three views.
pub fn settings_window(app: &mut App, ctx: &egui::Context) {
    if !app.settings_open {
        app.settings_content_h = 0.0;
        return;
    }
    let modal = egui::Modal::new(egui::Id::new("settings"))
        .frame(
            egui::Frame::new()
                .fill(SIDE_BG)
                .corner_radius(8.0)
                .inner_margin(Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.set_width(SETTINGS_W);
            ui.spacing_mut().item_spacing.y = CARD_GAP;
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(fonts::icon::COGS)
                        .color(ACCENT)
                        .size(fonts::HEADING),
                );
                ui.label(
                    RichText::new("Settings")
                        .family(fonts::bold())
                        .size(fonts::HEADING)
                        .color(Color32::from_gray(235)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Close")
                        .on_hover_text("Close settings — Esc")
                        .clicked()
                    {
                        app.settings_open = false;
                    }
                    // Esc alone: spelled out as "<key> to close" so the hint
                    // cannot be mistaken for a view shortcut. Ctrl+, opens the
                    // dialog too, but it lives in the Keys card, not here.
                    ui.add_space(4.0);
                    ui.label(RichText::new("to close").small().color(KEY));
                    keycap(ui, "Esc");
                });
            });
            let max_h = (ctx.viewport_rect().height() * 0.85 - 80.0).max(300.0);
            let want_h = app.settings_content_h.min(max_h);
            let mut area = egui::ScrollArea::vertical()
                .max_height(max_h)
                .min_scrolled_height(want_h)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, true]);
            if app.settings_content_h == 0.0 {
                // Freshly opened: start at the top whatever egui remembers.
                // The screenshot hook asks for the bottom instead; egui clamps
                // the offset to the content once it has measured it.
                let offset = if app.settings_scroll_end { 1e5 } else { 0.0 };
                area = area.vertical_scroll_offset(offset);
            }
            let out = area.show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = CARD_GAP;
                panes_card(app, ui);
                spectrogram_card(app, ui);
                waveform_card(app, ui);
                spectrum_card(app, ui);
                files_card(app, ui);
                keys_card(ui);
                about_card(app, ui);
            });
            app.settings_content_h = out.content_size.y;
        });
    if modal.should_close() {
        app.settings_open = false;
    }
}

/// Width of the settings dialog, and the gap between its cards.
const SETTINGS_W: f32 = 560.0;
const CARD_GAP: f32 = 12.0;
/// Width of the label column in every settings grid, so controls line up
/// across cards.
const LABEL_W: f32 = 156.0;
const GRID_GAP: f32 = 18.0;
/// Vertical gap between the rows of a settings grid.
const ROW_GAP: f32 = 10.0;
/// Gap between the controls within one row, e.g. a run of checkboxes.
const CTRL_GAP: f32 = 12.0;
/// Width of a slider's value box plus its gap, reserved so the slider track
/// ends at the same x in every row.
const VALUE_W: f32 = 96.0;
const COMBO_W: f32 = 150.0;

/// One labelled control row inside a settings grid.
fn setting(
    ui: &mut egui::Ui,
    label: &str,
    topic: Option<Topic>,
    control: impl FnOnce(&mut egui::Ui),
) {
    let l = ui.add(egui::Label::new(RichText::new(label).color(KEY)).truncate());
    if let Some(topic) = topic {
        help::offer_response(ui, &l, topic);
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = CTRL_GAP;
        control(ui);
    });
    ui.end_row();
}

fn settings_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    let slider_w = (ui.available_width() - LABEL_W - GRID_GAP - VALUE_W).max(120.0);
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL_W)
        .spacing([GRID_GAP, ROW_GAP])
        .show(ui, |ui| {
            ui.spacing_mut().slider_width = slider_w;
            rows(ui);
        });
}

/// A thin strip showing a colour lookup table from quiet to loud.
fn palette_strip(ui: &mut egui::Ui, lut: &[Color32]) {
    let (rect, _) = ui.allocate_exact_size(vec2(96.0, 14.0), Sense::hover());
    let p = ui.painter();
    let w = rect.width() / lut.len() as f32;
    for (i, c) in lut.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + i as f32 * w, rect.top()),
            vec2(w + 0.6, rect.height()),
        );
        p.rect_filled(r, 0.0, *c);
    }
    p.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Outside,
    );
}

fn combo(id: &str) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id).width(COMBO_W)
}

fn panes_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::LIST,
        "Panes",
        Topic::PanesCard,
        None,
        |ui| {
            let both = app.settings.show_waveform && app.settings.show_spectrogram;
            let merged = both && app.settings.merge_views;
            settings_grid(ui, "panes-grid", |ui| {
                setting(ui, "Show", Some(Topic::ShowPanes), |ui| {
                    ui.checkbox(&mut app.settings.show_waveform, "Waveform");
                    ui.checkbox(&mut app.settings.show_spectrogram, "Spectrogram");
                    ui.checkbox(&mut app.settings.show_spectrum, "Spectrum");
                });
                setting(ui, "Merge", Some(Topic::MergeViews), |ui| {
                    ui.add_enabled_ui(both, |ui| {
                        ui.checkbox(&mut app.settings.merge_views, "Waveform over spectrogram");
                    });
                });
                // Enabled-state scopes must sit inside the control cell: a scope
                // around the whole row would swallow the grid's end_row.
                setting(
                    ui,
                    "Waveform opacity",
                    Some(Topic::MergeWaveOpacity),
                    |ui| {
                        ui.add_enabled_ui(merged, |ui| {
                            ui.add(
                                egui::Slider::new(&mut app.settings.merge_opacity, 0.05..=1.0)
                                    .fixed_decimals(2),
                            );
                        });
                    },
                );
                setting(
                    ui,
                    "Spectrogram opacity",
                    Some(Topic::MergeSpecOpacity),
                    |ui| {
                        ui.add_enabled_ui(merged, |ui| {
                            ui.add(
                                egui::Slider::new(&mut app.settings.merge_spec_opacity, 0.05..=1.0)
                                    .fixed_decimals(2),
                            );
                        });
                    },
                );
            });
            if !both && app.settings.merge_views {
                ui.label(
                    RichText::new("Merge needs both the waveform and the spectrogram.").color(KEY),
                );
            }
            if !app.settings.show_waveform && !app.settings.show_spectrogram {
                ui.label(
                    RichText::new(format!(
                        "{} Nothing left to draw; turn one back on.",
                        fonts::icon::WARN
                    ))
                    .color(WARN),
                );
            }
        },
    );
}

fn spectrogram_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::BARS,
        "Spectrogram",
        Topic::SpectrogramCard,
        None,
        |ui| {
            let mut stft = app.settings.stft;
            settings_grid(ui, "stft-grid", |ui| {
                setting(ui, "Window size", Some(Topic::WindowSize), |ui| {
                    combo("win-size")
                        .selected_text(stft.window_size.to_string())
                        .show_ui(ui, |ui| {
                            for s in StftParams::SIZES {
                                ui.selectable_value(&mut stft.window_size, s, s.to_string());
                            }
                        });
                });
                setting(ui, "Overlap", Some(Topic::Overlap), |ui| {
                    let mut overlap = (stft.overlap_num, stft.overlap_den);
                    combo("overlap")
                        .selected_text(stft.overlap_label())
                        .show_ui(ui, |ui| {
                            for (n, d) in [(0u8, 1u8), (1, 2), (3, 4), (7, 8)] {
                                let label = format!("{}%", 100 * n as u32 / d as u32);
                                ui.selectable_value(&mut overlap, (n, d), label);
                            }
                        });
                    (stft.overlap_num, stft.overlap_den) = overlap;
                });
                setting(ui, "Window", Some(Topic::WindowKind), |ui| {
                    combo("win-kind")
                        .selected_text(stft.window.name())
                        .show_ui(ui, |ui| {
                            for w in WindowKind::ALL {
                                ui.selectable_value(&mut stft.window, w, w.name());
                            }
                        });
                });
                let sr = app.sample_rate().max(1.0);
                setting(ui, "Resolution", Some(Topic::Resolution), |ui| {
                    ui.label(mono(format!(
                        "bin {} · frame {} · hop {}",
                        fmt_hz_unit(sr / stft.window_size as f64),
                        fmt_ms(stft.window_size as f64 / sr * 1000.0),
                        fmt_ms(stft.hop() as f64 / sr * 1000.0),
                    )));
                });
                setting(ui, "Reassignment", Some(Topic::Reassignment), |ui| {
                    ui.checkbox(&mut stft.reassign, "Sharper lines and clicks");
                });
            });
            if stft != app.settings.stft {
                app.settings.stft = stft;
                app.recompute_spectrograms();
            }
            ui.add_space(4.0);
            settings_grid(ui, "spec-grid", |ui| {
                setting(ui, "Colour map", Some(Topic::ColourMap), |ui| {
                    combo("cmap")
                        .selected_text(app.settings.colormap.name())
                        .show_ui(ui, |ui| {
                            for c in ColorMap::ALL {
                                ui.selectable_value(&mut app.settings.colormap, c, c.name());
                            }
                        });
                    let lut = app
                        .settings
                        .colormap
                        .lut_with(app.settings.spec_contrast, &app.settings.custom_stops);
                    palette_strip(ui, &lut);
                });
                if app.settings.colormap == ColorMap::Custom {
                    setting(ui, "Custom colours", Some(Topic::CustomColours), |ui| {
                        for (i, label) in ["quiet", "medium", "loud"].iter().enumerate() {
                            ui.color_edit_button_srgb(&mut app.settings.custom_stops[i])
                                .on_hover_text(*label);
                        }
                        if ui.button("Reset").clicked() {
                            app.settings.custom_stops = auriscope::analysis::DEFAULT_CUSTOM;
                        }
                    });
                }
                setting(ui, "Contrast", Some(Topic::Contrast), |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.spec_contrast, 0.4..=2.5)
                            .fixed_decimals(2)
                            .logarithmic(true),
                    );
                });
                setting(ui, "Frequency axis", Some(Topic::FrequencyAxis), |ui| {
                    ui.checkbox(&mut app.settings.log_frequency, "Logarithmic");
                });
                setting(ui, "Floor", Some(Topic::Floor), |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.db_min, -140.0..=-20.0)
                            .fixed_decimals(0)
                            .suffix(" dB"),
                    );
                });
                setting(ui, "Ceiling", Some(Topic::Ceiling), |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.db_max, -60.0..=0.0)
                            .fixed_decimals(0)
                            .suffix(" dB"),
                    );
                });
                if app.settings.db_max <= app.settings.db_min + 6.0 {
                    app.settings.db_max = app.settings.db_min + 6.0;
                }
                setting(ui, "Lowest frequency", Some(Topic::LowestFrequency), |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.min_hz, 10.0..=200.0)
                            .fixed_decimals(0)
                            .suffix(" Hz")
                            .logarithmic(true),
                    );
                });
            });
        },
    );
}

fn waveform_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::GAUGE,
        "Waveform",
        Topic::WaveformCard,
        None,
        |ui| {
            let merged = app.settings.merge_views
                && app.settings.show_waveform
                && app.settings.show_spectrogram;
            settings_grid(ui, "wave-grid", |ui| {
                setting(ui, "Overlays", Some(Topic::WaveOverlays), |ui| {
                    ui.checkbox(&mut app.settings.show_rms, "RMS");
                    ui.checkbox(&mut app.settings.show_db_scale, "dB scale");
                });
                setting(ui, "Colour", Some(Topic::WaveColour), |ui| {
                    ui.color_edit_button_srgb(&mut app.settings.wave_color);
                    const SWATCHES: [([u8; 3], &str); 5] = [
                        (super::DEFAULT_WAVE_COLOR, "Blue"),
                        ([235, 235, 235], "White"),
                        ([120, 220, 140], "Green"),
                        ([247, 198, 72], "Amber"),
                        ([230, 120, 200], "Pink"),
                    ];
                    for (rgb, name) in SWATCHES {
                        let c = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                        let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().rect_filled(rect, 3.0, c);
                        if app.settings.wave_color == rgb {
                            ui.painter().rect_stroke(
                                rect,
                                3.0,
                                Stroke::new(1.5, Color32::WHITE),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if resp.on_hover_text(name).clicked() {
                            app.settings.wave_color = rgb;
                        }
                    }
                });
                setting(ui, "Vertical zoom", Some(Topic::VerticalZoom), |ui| {
                    // Leave room for the reset button so the track still ends
                    // where the other sliders do.
                    ui.spacing_mut().slider_width -= 34.0;
                    ui.add(
                        egui::Slider::new(&mut app.settings.wave_v_zoom, V_ZOOM_MIN..=V_ZOOM_MAX)
                            .fixed_decimals(2)
                            .logarithmic(true)
                            .suffix("x"),
                    );
                    if ui.button("1x").on_hover_text("Reset").clicked() {
                        app.settings.wave_v_zoom = 1.0;
                    }
                });
                setting(ui, "Height", Some(Topic::WaveHeight), |ui| {
                    ui.add_enabled_ui(!merged, |ui| {
                        ui.add(
                            egui::Slider::new(&mut app.settings.waveform_fraction, 0.05..=0.95)
                                .fixed_decimals(2),
                        );
                    });
                });
            });
        },
    );
}

fn spectrum_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::BARS,
        "Spectrum",
        Topic::SpectrumCard,
        None,
        |ui| {
            let mut size = app.settings.spectrum_size;
            settings_grid(ui, "spectrum-grid", |ui| {
                setting(ui, "FFT size", Some(Topic::SpectrumSize), |ui| {
                    combo("spectrum-size")
                        .selected_text(size.to_string())
                        .show_ui(ui, |ui| {
                            for s in [1024usize, 2048, 4096, 8192, 16384] {
                                ui.selectable_value(&mut size, s, s.to_string());
                            }
                        });
                });
                setting(ui, "Averaging", Some(Topic::SpectrumAveraging), |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.spectrum_averaging, 0.0..=0.95)
                            .fixed_decimals(2),
                    );
                });
            });
            if size != app.settings.spectrum_size {
                app.settings.spectrum_size = size;
                if let Some(e) = &app.engine {
                    app.live = Some(auriscope::analysis::LiveSpectrum::new(size, e.device_rate));
                }
            }
        },
    );
}

/// Height the history list scrolls at: about six rows, so a full history is
/// reachable without the card growing past the cards around it.
const RECENT_LIST_H: f32 = 132.0;

fn files_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::CLOCK,
        "Files",
        Topic::FilesCard,
        None,
        |ui| {
            settings_grid(ui, "files-grid", |ui| {
                setting(ui, "History", Some(Topic::RememberFiles), |ui| {
                    let mut on = app.settings.remember_recent;
                    if ui.checkbox(&mut on, "Remember the files I open").changed() {
                        app.settings.remember_recent = on;
                        // Switching it off is a request to forget, not just to
                        // stop recording: leaving the old list behind would
                        // make the setting a half-truth.
                        if !on {
                            app.settings.clear_recent();
                        }
                    }
                });
            });
            ui.add_space(2.0);
            if !app.settings.remember_recent {
                ui.label(
                    RichText::new(
                        "Nothing is kept: no history, and the app starts empty rather than \
                         reopening the last file.",
                    )
                    .small()
                    .color(KEY),
                );
                return;
            }
            let recent = app.settings.recent_files.clone();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} of {RECENT_MAX} remembered, newest first",
                        recent.len()
                    ))
                    .small()
                    .color(KEY),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(!recent.is_empty(), egui::Button::new("Clear history"))
                        .clicked()
                    {
                        app.settings.clear_recent();
                    }
                });
            });
            if recent.is_empty() {
                return;
            }
            ui.add_space(4.0);
            let mut forget = None;
            let mut open = None;
            egui::ScrollArea::vertical()
                .max_height(RECENT_LIST_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for path in &recent {
                        let here = path.is_file();
                        ui.horizontal(|ui| {
                            if ui
                                .small_button("✕")
                                .on_hover_text("Forget this one")
                                .clicked()
                            {
                                forget = Some(path.clone());
                            }
                            let label = RichText::new(file_label(path))
                                .color(if here { VAL } else { KEY })
                                .size(fonts::BODY);
                            let resp = ui.add(
                                egui::Label::new(label)
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            let resp = if here {
                                resp.on_hover_text(path.display().to_string())
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                            } else {
                                resp.on_hover_text(format!(
                                    "{} — not there any more.",
                                    path.display()
                                ))
                            };
                            if here && resp.clicked() {
                                open = Some(path.clone());
                            }
                        });
                    }
                });
            if let Some(path) = forget {
                app.settings.forget_file(&path);
            }
            if let Some(path) = open {
                app.open(&path);
                app.settings_open = false;
            }
        },
    );
}

fn keys_card(ui: &mut egui::Ui) {
    wide_card(ui, fonts::icon::INFO, "Keys", Topic::KeysCard, None, |ui| {
        const KEYS: [(&str, &str); 18] = [
            ("Space", "play / pause"),
            ("Ctrl+O", "open a file"),
            ("Ctrl+,", "settings"),
            ("F1", "help mode: hover a label to read what it means"),
            ("Click", "seek, clear the highlight"),
            ("Drag", "select; the range stays on the ruler"),
            ("Ruler handles", "drag to adjust the range"),
            ("Right-click", "clear highlight and range"),
            ("L", "loop the range"),
            ("Esc", "clear highlight, then range"),
            ("F / Shift+F", "zoom to range / fit file"),
            ("+ / −", "zoom in / out"),
            ("← → / Shift", "±5 s / ±1 s · Home, End"),
            ("Wheel", "zoom at pointer · Shift: pan"),
            ("Ctrl+wheel, pinch", "zoom in / out"),
            ("Alt+Shift+wheel", "waveform vertical zoom"),
            ("Middle-drag", "scroll the clip"),
            ("Divider", "drag to resize the strips"),
        ];
        let desc_w = (ui.available_width() - LABEL_W - GRID_GAP).max(120.0);
        egui::Grid::new("keys-grid")
            .num_columns(2)
            .min_col_width(LABEL_W)
            .spacing([GRID_GAP, 7.0])
            .striped(true)
            .show(ui, |ui| {
                for (k, v) in KEYS {
                    ui.label(RichText::new(k).monospace().color(VAL));
                    ui.horizontal(|ui| {
                        ui.set_min_width(desc_w);
                        ui.label(RichText::new(v).color(KEY));
                    });
                    ui.end_row();
                }
            });
    });
}

fn about_card(app: &mut App, ui: &mut egui::Ui) {
    wide_card(
        ui,
        fonts::icon::TAG,
        "About",
        Topic::AboutCard,
        None,
        |ui| {
            kv_rows(ui, |rows| {
                kv(rows, "Version", update::CURRENT);
                if update::is_dev_build() {
                    kv_colored(rows, "Build", update::GIT_DESCRIBE, ACCENT);
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Source").small().color(KEY));
                ui.hyperlink_to(
                    RichText::new("github.com/frdcmp/auriscope").monospace(),
                    update::REPO_URL,
                );
            });
            ui.add_space(4.0);
            if !update::ENABLED {
                ui.label(
                    RichText::new("Updates come through your package manager.")
                        .small()
                        .color(KEY),
                );
                return;
            }
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut app.settings.check_updates,
                    "Check for updates on startup",
                )
                .on_hover_text(
                    "Asks api.github.com for the latest release, at most once a day. \
                     Nothing is downloaded and nothing about you is sent.",
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let checking = app.updater.status == update::Status::Checking;
                    if ui
                        .add_enabled(!checking, egui::Button::new("Check now"))
                        .clicked()
                    {
                        app.updater.start();
                    }
                });
            });
            ui.horizontal(|ui| match &app.updater.status {
                update::Status::Idle => {
                    ui.label(RichText::new("Not checked yet.").small().color(KEY));
                }
                update::Status::Checking => {
                    ui.spinner();
                    ui.label(RichText::new("Checking…").small().color(KEY));
                }
                update::Status::UpToDate => {
                    ui.label(RichText::new("Up to date.").small().color(GOOD));
                }
                update::Status::Newer(r) => {
                    let r = r.clone();
                    ui.label(
                        RichText::new(format!("{} available.", r.version))
                            .small()
                            .color(ACCENT),
                    );
                    ui.hyperlink_to(RichText::new("Release page").small(), &r.url);
                    let skipped =
                        app.settings.update_skipped.as_deref() == Some(r.version.as_str());
                    let label = if skipped { "Remind me" } else { "Skip" };
                    if ui.small_button(label).clicked() {
                        app.settings.update_skipped = (!skipped).then(|| r.version.clone());
                    }
                }
                update::Status::Failed(e) => {
                    ui.label(
                        RichText::new(format!("Check failed: {e}"))
                            .small()
                            .color(KEY),
                    )
                    .on_hover_text(
                        "Offline, or GitHub declined the request. Nothing else is affected.",
                    );
                }
            });
        },
    );
}
