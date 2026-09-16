//! The sidebar cards reporting what the audio measures: programme loudness,
//! per-channel levels with the mixer strip, the analysis summary and the
//! cursor readout. These follow playback and the pointer, so they redraw
//! constantly.

use eframe::egui;
use egui::{
    Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Shape, Stroke, pos2, vec2,
};

use crate::ui::help::{self, Topic};
use crate::ui::util::{db_str, fmt_int, fmt_time, lufs_str};
use crate::ui::views::channel_name;
use crate::ui::{App, fonts};

use super::theme::{ACCENT, BAD, CARD_BG, GOOD, KEY, VAL, WARN};
use super::widgets::{CARD_HEAD_BG, CARD_HEAD_RULE, card, kv, kv_colored, kv_rows, subhead};

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

pub(super) fn loudness_card(app: &App, ui: &mut egui::Ui) {
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

pub(super) fn levels_card(app: &mut App, ui: &mut egui::Ui) {
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

pub(super) fn analysis_card(app: &App, ui: &mut egui::Ui) {
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

pub(super) fn cursor_card(app: &App, ui: &mut egui::Ui) {
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
