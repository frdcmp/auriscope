//! The transport bar: the row across the top holding playback, the file
//! controls, the output meter and the mixer sliders.
//!
//! Everything here is drawn on one line, so the row's height is pinned once
//! and the widgets that paint themselves take it from `ROW_H` rather than
//! each guessing.

use eframe::egui;
use egui::{Align2, Color32, FontId, Rangef, Rect, RichText, Sense, Shape, Stroke, pos2, vec2};

use auriscope::analysis::db_to_amp;

use crate::ui::help::{self, Topic};
use crate::ui::util::{file_label, fmt_time, fmt_time_field};
use crate::ui::{App, fonts, update};

use super::theme::{ACCENT, BAD, GOOD, KEY, VAL, WARN};

/// Height of the transport bar's widget row. egui otherwise assumes a row is
/// `interact_size.y` tall and centres small widgets in that band, while taller
/// ones (the painted transport icons) hang below it — so the "Open…" button and
/// the labels sat a few pixels above the icons. Pinning the row to the tallest
/// widget puts everything on one centre line.
const ROW_H: f32 = 26.0;

pub fn top_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("top").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.set_height(ROW_H);
            if ui.button("Open…").on_hover_text("Ctrl+O").clicked() {
                app.pick_file();
            }
            recent_menu(app, ui);
            ui.separator();

            let has = app.engine.is_some();
            let playing = app.engine.as_ref().is_some_and(|e| e.is_playing());
            if transport_button(ui, has, Transport::ToStart, "Home").clicked() && has {
                app.seek_frames(0.0);
            }
            let icon = if playing {
                Transport::Pause
            } else {
                Transport::Play
            };
            if transport_button(ui, has, icon, "Space").clicked() && has {
                app.toggle_play();
            }
            if transport_button(ui, has, Transport::Stop, "Stop").clicked()
                && let Some(e) = &app.engine
            {
                e.pause();
                let start = app.loop_region().map_or(0, |(a, _)| a);
                e.seek(start);
            }

            let sr = app.sample_rate();
            let pos = app
                .engine
                .as_ref()
                .map_or(0.0, |e| e.playhead() as f64 / sr);
            let total = app.audio.as_ref().map_or(0.0, |a| a.info.duration_secs());
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    fmt_time_field(pos, total),
                    fmt_time(total)
                ))
                .monospace()
                .size(15.0),
            );

            ui.separator();
            let mut loop_on = app.loop_enabled;
            if ui
                .add_enabled(
                    app.range.is_some(),
                    egui::Checkbox::new(&mut loop_on, "Loop"),
                )
                .on_hover_text("L — loop the range on the ruler")
                .changed()
            {
                app.loop_enabled = loop_on;
                app.apply_loop();
            }
            // Mid-drag the ruler range is still the old one: the range only
            // lands when the button comes up. The live selection is what the
            // pointer is describing, so the readout follows that and counts up
            // as the drag is drawn. Ordered, because a drag can run backwards.
            if let Some((a, b)) = app.selection.or(app.range) {
                let (a, b) = (a.min(b) / sr, a.max(b) / sr);
                ui.label(
                    RichText::new(format!(
                        "{} – {}  ({})",
                        fmt_time_field(a, total),
                        fmt_time_field(b, total),
                        fmt_time_field(b - a, total)
                    ))
                    .monospace()
                    .color(Color32::from_gray(170)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The value boxes sit in a right-to-left layout, so anything
                // that changes their width shoves the rest of the bar sideways
                // as you drag. Monospace digits plus a padded, fixed-length
                // format keep each box the same size at every value.
                ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
                // Always signed: a bare "0.0" beside a slider that runs both
                // ways says nothing about which side of neutral you are on.
                let gain_tint = gain_tint(app.settings.gain_db);
                let gain = egui::Slider::new(&mut app.settings.gain_db, -60.0..=12.0)
                    .suffix(" dB")
                    .custom_formatter(|n, _| format!("{n:>+5.1}"))
                    .custom_parser(|s| s.trim().parse().ok());
                let mut gain_changed = tinted_slider(ui, gain_tint, gain).changed();
                if reset_label(ui, "Gain", "+0.0 dB").clicked() {
                    app.settings.gain_db = 0.0;
                    gain_changed = true;
                }
                if gain_changed && let Some(e) = &app.engine {
                    e.shared.set_gain(db_to_amp(app.settings.gain_db));
                }
                let pan_tint = pan_tint(app.settings.pan);
                let pan = egui::Slider::new(&mut app.settings.pan, -1.0..=1.0)
                    .custom_formatter(|n, _| format!("{n:>+5.2}"))
                    .custom_parser(|s| s.trim().parse().ok());
                let mut pan_changed = tinted_slider(ui, pan_tint, pan).changed();
                if reset_label(ui, "Pan", "centre").clicked() {
                    app.settings.pan = 0.0;
                    pan_changed = true;
                }
                if pan_changed && let Some(e) = &app.engine {
                    e.shared.set_pan(app.settings.pan);
                }
                ui.separator();
                out_meter(app, ui);
                ui.separator();
                if settings_button(ui, app.settings_open)
                    .on_hover_text("Settings (Ctrl+,)")
                    .clicked()
                {
                    app.settings_open = !app.settings_open;
                }
                camera_button(app, ui);
                if help::toggle_button(ui, app.help_mode, vec2(28.0, ROW_H))
                    .on_hover_text(if app.help_mode {
                        "Help mode on: hover anything labelled to read what it means (F1)"
                    } else {
                        "Help mode (F1)"
                    })
                    .clicked()
                {
                    app.help_mode = !app.help_mode;
                }
                ui.separator();
                ui.checkbox(&mut app.settings.follow_playhead, "Follow");
                update_notice(app, ui);
            });
        });
    });
}

/// Where a gain sits on its colour ramp: the hue its far end wears, and how far
/// along it this value is. Cuts fade toward the quiet key grey; boosts run
/// through amber into the meter's red, which is where they start costing heads.
fn gain_tint(db: f32) -> (Color32, f32) {
    if db >= 0.0 {
        let t = (db / 12.0).clamp(0.0, 1.0);
        (WARN.lerp_to_gamma(BAD, (t * 2.0 - 1.0).max(0.0)), t)
    } else {
        (KEY, (db / -60.0).clamp(0.0, 1.0))
    }
}

/// Pan's ramp: the accent, by how far off centre rather than which way round.
/// The handle and the sign already say which side, and tinting the two sides
/// differently would imply one of them is the wrong side to be on.
fn pan_tint(pan: f32) -> (Color32, f32) {
    (ACCENT, pan.abs().clamp(0.0, 1.0))
}

/// Draws a slider carrying its value as colour as well as position: the rail
/// takes a hint of the hue, the grab and the number take all of it. At the
/// default nothing is tinted, so a moved control stands out from a bar full of
/// untouched ones.
fn tinted_slider(
    ui: &mut egui::Ui,
    (hue, t): (Color32, f32),
    slider: egui::Slider<'_>,
) -> egui::Response {
    ui.scope(|ui| {
        let v = &mut ui.style_mut().visuals;
        let fg = VAL.lerp_to_gamma(hue, t);
        // The number is a `DragValue`, and its text follows this override
        // rather than the stroke the grab is drawn with.
        v.override_text_color = Some(fg);
        for w in [
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
        ] {
            // `bg_fill` is the rail as well as the grab's fill, so it only gets
            // a wash of the hue: a fully coloured rail would shout.
            w.bg_fill = w.bg_fill.lerp_to_gamma(hue, 0.45 * t);
            w.fg_stroke.color = fg;
        }
        ui.add(slider)
    })
    .inner
}

/// A slider's caption, drawn here instead of through `Slider::text` so that a
/// click on it can put the slider back to its neutral value. `Slider` keeps its
/// label's response to itself, so the only way to hear the click is to draw the
/// label separately — in a right-to-left row it still lands left of the slider.
fn reset_label(ui: &mut egui::Ui, text: &str, default: &str) -> egui::Response {
    ui.add(
        egui::Label::new(text)
            .wrap_mode(egui::TextWrapMode::Extend)
            .sense(Sense::click()),
    )
    .on_hover_text(format!("Click to reset to {default}"))
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Output meter geometry: wide enough for 60 dB to stay readable, with room
/// for the channel letter on the left and its level on the right.
const METER_W: f32 = 178.0;
const METER_MIN_DB: f32 = -60.0;
/// Above this the bar turns amber, and at full scale it turns red.
const METER_WARN_DB: f32 = -6.0;
const METER_CLIP_DB: f32 = -0.2;

/// The stereo output meter: one bar per device channel, peak ballistics with
/// a hold marker, and the same number in dBFS beside it.
fn out_meter(app: &App, ui: &mut egui::Ui) {
    let (rect, resp) = ui.allocate_exact_size(vec2(METER_W, ROW_H), Sense::hover());
    let p = ui.painter_at(rect);
    const LABEL_COL: f32 = 10.0;
    const VALUE_COL: f32 = 40.0;
    const BAR_H: f32 = 7.0;
    const BAR_GAP: f32 = 3.0;
    let font = FontId::monospace(fonts::RULER);
    let x0 = rect.left() + LABEL_COL;
    let x1 = rect.right() - VALUE_COL;
    let top = rect.center().y - (BAR_H * 2.0 + BAR_GAP) / 2.0;
    // Linear in dB: the top 6 dB, where a mix lives or dies, gets a tenth of
    // the width, the same as any other 6 dB.
    let frac = |amp: f32| {
        let db = 20.0 * amp.max(1e-7).log10();
        ((db - METER_MIN_DB) / -METER_MIN_DB).clamp(0.0, 1.0)
    };
    for (ch, name) in ["L", "R"].iter().enumerate() {
        let m = app.meter[ch];
        let y = top + ch as f32 * (BAR_H + BAR_GAP);
        let track = Rect::from_min_max(pos2(x0, y), pos2(x1, y + BAR_H));
        p.rect_filled(track, 2.0, Color32::from_gray(38));
        // -6 dB, where the bar changes colour: a hairline so the eye can find
        // the threshold without reading the number.
        let warn_x = x0 + (track.width()) * ((METER_WARN_DB - METER_MIN_DB) / -METER_MIN_DB);
        p.vline(
            warn_x,
            Rangef::new(track.top(), track.bottom()),
            Stroke::new(1.0, Color32::from_gray(58)),
        );
        let db = 20.0 * m.level.max(1e-7).log10();
        let lit = if db >= METER_CLIP_DB {
            BAD
        } else if db >= METER_WARN_DB {
            WARN
        } else {
            GOOD
        };
        let w = track.width() * frac(m.level);
        if w > 0.5 {
            p.rect_filled(Rect::from_min_size(track.min, vec2(w, BAR_H)), 2.0, lit);
        }
        if m.hold > 0.0 {
            let hx = (x0 + track.width() * frac(m.hold)).min(x1 - 1.0);
            p.vline(
                hx,
                Rangef::new(track.top(), track.bottom()),
                Stroke::new(1.5, lit.gamma_multiply(0.85)),
            );
        }
        p.text(
            pos2(rect.left(), track.center().y),
            Align2::LEFT_CENTER,
            name,
            font.clone(),
            Color32::from_gray(120),
        );
        p.text(
            pos2(rect.right(), track.center().y),
            Align2::RIGHT_CENTER,
            if m.level <= 1e-6 {
                "  -inf".into()
            } else {
                format!("{db:>6.1}")
            },
            font.clone(),
            if db >= METER_CLIP_DB { BAD } else { VAL },
        );
    }
    help::offer_response(ui, &resp, Topic::OutputMeter);
}

/// "0.2.0 available" in the transport bar, with a skip button. Shown only
/// while a newer release is known and the user has not dismissed it.
/// Width the history menu opens at, so a run of file names reads as a column
/// rather than a ragged edge.
const RECENT_MENU_W: f32 = 260.0;

/// The history menu beside "Open…": every file opened before, newest first.
///
/// Disabled rather than hidden when there is nothing in it, so the bar keeps
/// its shape from the first launch onwards.
fn recent_menu(app: &mut App, ui: &mut egui::Ui) {
    let empty = app.settings.recent_files.is_empty();
    ui.add_enabled_ui(!empty, |ui| {
        let resp = ui
            .menu_button("Recent", |ui| {
                ui.set_min_width(RECENT_MENU_W);
                // Cloned because opening a file borrows the whole app, and the
                // list it would be iterating lives inside it.
                let recent = app.settings.recent_files.clone();
                let mut open = None;
                for path in &recent {
                    // A stat per entry, but only while the menu is open: a file
                    // moved or deleted since should not look openable.
                    let here = path.is_file();
                    let name = file_label(path);
                    let text = RichText::new(name).color(if here { VAL } else { KEY });
                    let resp = ui
                        .add_enabled(here, egui::Button::new(text).truncate())
                        .on_hover_text(path.display().to_string())
                        .on_disabled_hover_text(format!(
                            "{} — not there any more.",
                            path.display()
                        ));
                    if resp.clicked() {
                        open = Some(path.clone());
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Clear history").clicked() {
                    app.settings.clear_recent();
                    ui.close();
                }
                if let Some(path) = open {
                    app.open(&path);
                }
            })
            .response;
        if empty {
            resp.on_disabled_hover_text("No files opened yet.");
        } else {
            resp.on_hover_text("Files opened before, newest first");
        }
    });
}

fn update_notice(app: &mut App, ui: &mut egui::Ui) {
    let Some(release) = app
        .updater
        .notice(app.settings.update_skipped.as_deref())
        .cloned()
    else {
        return;
    };
    ui.separator();
    let skip = ui
        .add(egui::Button::new(RichText::new("×").color(KEY)).frame(false))
        .on_hover_text("Skip this version");
    let open = ui
        .add(egui::Button::new(
            RichText::new(format!(
                "{} {} available",
                fonts::icon::DOWNLOAD,
                release.version
            ))
            .color(ACCENT),
        ))
        .on_hover_text(format!(
            "Auriscope {} is out; you have {}.\nOpens the release page.",
            release.version,
            update::CURRENT
        ));
    if open.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(&release.url));
    }
    if skip.clicked() {
        app.settings.update_skipped = Some(release.version);
    }
}

/// The camera: saves the waveform, spectrogram and spectrum as a PNG, with a
/// JSON of everything the sidebar says about the file beside it.
///
/// Its tooltip is left off while a capture is on its way, because the frame
/// being saved is the one drawn a beat after the click, and the pointer is
/// still sitting on the button then. See [`crate::ui::capture`].
fn camera_button(app: &mut App, ui: &mut egui::Ui) {
    let taking = app.capture.is_some();
    let resp = camera_icon(ui, taking);
    let resp = if taking {
        resp
    } else {
        resp.on_hover_text(
            "Save the views as a PNG, with a JSON of the file's info beside it \
             (Ctrl+Shift+S)",
        )
    };
    if resp.clicked() && !taking {
        app.save_screenshot();
    }
}

/// The camera glyph: a body, the lens, and the bump over it, drawn as vectors
/// like the buttons either side of it.
fn camera_icon(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(vec2(28.0, ROW_H), Sense::click());
    let visuals = ui.style().visuals.clone();
    let fg = if active {
        visuals.widgets.active.fg_stroke.color
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        Color32::from_gray(215)
    };
    if active || resp.hovered() {
        let bg = if active {
            visuals.widgets.active.weak_bg_fill
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(resp.rect, 4.0, bg);
    }
    let c = resp.rect.center();
    let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(16.0, 11.0));
    // The viewfinder bump, drawn before the body so the body's stroke closes
    // the top edge over it.
    painter.rect_filled(
        Rect::from_min_max(
            pos2(c.x - 4.5, body.top() - 3.0),
            pos2(c.x - 0.5, body.top() + 1.0),
        ),
        1.0,
        fg,
    );
    painter.rect_stroke(body, 2.0, Stroke::new(1.4, fg), egui::StrokeKind::Middle);
    painter.circle_stroke(body.center(), 3.2, Stroke::new(1.4, fg));
    resp
}

/// A sliders glyph for the settings button: three tracks with a knob each,
/// drawn as vectors so no fallback font decides how it looks.
fn settings_button(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(vec2(28.0, ROW_H), Sense::click());
    let visuals = ui.style().visuals.clone();
    let fg = if active {
        visuals.widgets.active.fg_stroke.color
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        Color32::from_gray(215)
    };
    if active || resp.hovered() {
        let bg = if active {
            visuals.widgets.active.weak_bg_fill
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(resp.rect, 4.0, bg);
    }
    let c = resp.rect.center();
    let half = 7.0;
    // Track y offsets and the knob position along each.
    for (dy, knob) in [(-5.0f32, -3.0f32), (0.0, 3.5), (5.0, -1.0)] {
        painter.line_segment(
            [pos2(c.x - half, c.y + dy), pos2(c.x + half, c.y + dy)],
            egui::Stroke::new(1.4, fg),
        );
        painter.circle_filled(pos2(c.x + knob, c.y + dy), 2.1, fg);
    }
    resp
}

enum Transport {
    Play,
    Pause,
    Stop,
    ToStart,
}

/// A transport button drawn as a vector icon: crisp, uniform, and not at the
/// mercy of whichever fallback font owns the ⏸/⏹/⏮ glyphs.
fn transport_button(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: Transport,
    tooltip: &str,
) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(vec2(30.0, ROW_H), Sense::click());
    let visuals = ui.style().visuals.clone();
    let (bg, fg) = if !enabled {
        (Color32::TRANSPARENT, Color32::from_gray(100))
    } else if resp.hovered() && resp.is_pointer_button_down_on() {
        (
            visuals.widgets.active.weak_bg_fill,
            visuals.widgets.active.fg_stroke.color,
        )
    } else if resp.hovered() {
        (
            visuals.widgets.hovered.weak_bg_fill,
            visuals.widgets.hovered.fg_stroke.color,
        )
    } else {
        (Color32::TRANSPARENT, Color32::from_gray(215))
    };
    if bg != Color32::TRANSPARENT {
        painter.rect_filled(resp.rect, 4.0, bg);
    }
    let c = resp.rect.center();
    match icon {
        Transport::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - 5.0, c.y - 8.0),
                    pos2(c.x - 5.0, c.y + 8.0),
                    pos2(c.x + 7.0, c.y),
                ],
                fg,
                egui::Stroke::NONE,
            ));
        }
        Transport::Pause => {
            let (w, h) = (2.5, 8.0);
            painter.rect_filled(
                Rect::from_min_max(pos2(c.x - 3.0 - w, c.y - h), pos2(c.x - 3.0 + w, c.y + h)),
                1.0,
                fg,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(c.x + 3.0 - w, c.y - h), pos2(c.x + 3.0 + w, c.y + h)),
                1.0,
                fg,
            );
        }
        Transport::Stop => {
            painter.rect_filled(Rect::from_center_size(c, vec2(13.0, 13.0)), 2.0, fg);
        }
        Transport::ToStart => {
            let h = 8.0;
            painter.rect_filled(
                Rect::from_min_max(pos2(c.x - 7.5, c.y - h), pos2(c.x - 5.0, c.y + h)),
                1.0,
                fg,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x + 7.5, c.y - h),
                    pos2(c.x + 7.5, c.y + h),
                    pos2(c.x - 3.5, c.y),
                ],
                fg,
                egui::Stroke::NONE,
            ));
        }
    }
    resp.on_hover_text(tooltip)
}
