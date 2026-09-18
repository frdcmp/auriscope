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

/// The narrowest a mixer slider's track may be squeezed to when the row runs
/// short of room. Below this the grab has nowhere left to travel.
const SLIDER_W_MIN: f32 = 56.0;

/// And the narrowest the output meter's bars may be drawn. A meter's width is
/// its resolution, so it is the first thing asked to give: a coarser bar
/// still says what it has to, where a slider with no travel does not.
const METER_W_MIN: f32 = 104.0;

/// What the bar wears at the width it has been given.
///
/// One line cannot hold every control on a narrow window, so the row sheds in
/// order of what is least missed — and what it sheds goes into the ⋯ menu
/// rather than out of reach. The output meter is the exception: a live bar has
/// no form in a menu, and the sidebar draws one per channel anyway.
#[derive(Clone, Copy, PartialEq)]
struct Dress {
    /// The camera and help-mode buttons stand in the bar.
    chrome: bool,
    /// The Follow box stands in the bar.
    follow: bool,
    /// The output meter is drawn at all.
    meter: bool,
    /// The gain and pan sliders stand in the bar.
    mixer: bool,
}

/// What each step of shedding gives back, net — net because the first step
/// also has to pay for the ⋯ button it brings with it. Measured from the bar
/// itself: these are widget widths in points, so they hold at any interface
/// scale, and the app brings its own font rather than the desktop's.
const SHED_CHROME: f32 = 45.0;
const SHED_FOLLOW: f32 = 88.0;
const SHED_METER: f32 = 200.0;
const SHED_MIXER: f32 = 425.0;

/// Room a control has to gain back before it returns to the bar. A window
/// parked on the width where a step happens would otherwise sit between two
/// dresses, and the bar would flicker as the measurement rounded one way and
/// then the other.
const DRESS_HYST: f32 = 12.0;

impl Dress {
    const FULL: Self = Self {
        chrome: true,
        follow: true,
        meter: true,
        mixer: true,
    };

    /// The dress for a group that has `room` to stand in and wants `full`
    /// points at full dress. `elastic` is how much the meter and sliders can
    /// give up before anything has to go, and `was` is last frame's dress,
    /// which is what the hysteresis is measured against.
    fn choose(full: f32, room: f32, elastic: f32, was: Self) -> Self {
        let mut d = Self::FULL;
        let mut need = full;
        let mut give = elastic;
        // Each test asks whether the row fits without shedding the control on
        // the line below it. One already shed has to earn its way back.
        let fits = |need: f32, give: f32, shown: bool| {
            need - give + if shown { 0.0 } else { DRESS_HYST } <= room
        };
        if fits(need, give, was.chrome) {
            return d;
        }
        d.chrome = false;
        need -= SHED_CHROME;
        if fits(need, give, was.follow) {
            return d;
        }
        d.follow = false;
        need -= SHED_FOLLOW;
        if fits(need, give, was.meter) {
            return d;
        }
        d.meter = false;
        need -= SHED_METER;
        give -= METER_W - METER_W_MIN;
        if fits(need, give, was.mixer) {
            return d;
        }
        d.mixer = false;
        d
    }

    /// What this dress leaves out of the bar, in points: added back to the
    /// measured width so that what is remembered is always the full dress's,
    /// whatever was on show when it was measured.
    fn shed_width(self) -> f32 {
        (if self.chrome { 0.0 } else { SHED_CHROME })
            + (if self.follow { 0.0 } else { SHED_FOLLOW })
            + (if self.meter { 0.0 } else { SHED_METER })
            + (if self.mixer { 0.0 } else { SHED_MIXER })
    }

    /// Whether anything that has somewhere else to be has been shed, which is
    /// when the ⋯ menu appears.
    fn overflowing(self) -> bool {
        !(self.chrome && self.follow && self.mixer)
    }
}

/// What the bar carries from one frame to the next: the width the right-hand
/// group wants at full dress, and the dress it settled on. Both are known
/// only once the group has been drawn, so they are read back a frame later —
/// and the only thing that moves them is a resize, which is not a frame you
/// can catch the bar out on.
#[derive(Clone, Copy)]
struct BarFit {
    full: f32,
    dress: Dress,
}

/// Where [`BarFit`] is kept between frames.
fn bar_fit_id() -> egui::Id {
    egui::Id::new("transport-bar-fit")
}

pub fn top_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("top").show(root, |ui| {
        ui.horizontal(|ui| {
            ui.set_height(ROW_H);
            // The mixer sliders are the row's only elastic parts: everything
            // else is a button, a checkbox or a fixed-width readout. When the
            // window is too narrow for all of it they give up their width
            // first, and the selection readout gives up its detail, rather
            // than the right-hand group overrunning the controls to its left.
            let full_slider = ui.spacing().slider_width;
            let (last_full, last_dress) =
                match ui.ctx().data(|d| d.get_temp::<BarFit>(bar_fit_id())) {
                    Some(f) => (f.full, f.dress),
                    None => (0.0, Dress::FULL),
                };
            let slider_give = 2.0 * (full_slider - SLIDER_W_MIN);
            let elastic = (METER_W - METER_W_MIN) + slider_give;
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
            if transport_button(ui, has, Transport::Stop, "Stop").clicked() && has {
                app.stop();
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
            // The dress is settled here, against the room left by the
            // controls that are always drawn and before the readout that is
            // not — which is why a long file's wider clock sheds a control
            // rather than overrunning one.
            let room = ui.available_width();
            let dress = Dress::choose(last_full, room, elastic, last_dress);
            let dressed = (last_full - dress.shed_width()).max(0.0);
            // Mid-drag the ruler range is still the old one: the range only
            // lands when the button comes up. The live selection is what the
            // pointer is describing, so the readout follows that and counts up
            // as the drag is drawn. Ordered, because a drag can run backwards.
            if let Some((a, b)) = app.selection.or(app.range) {
                let (a, b) = (a.min(b) / sr, a.max(b) / sr);
                // The controls have first claim on the room: they are
                // controls, this is a readout. It may put the meter and the
                // sliders under the squeeze but never off the bar, so what it
                // may take is what is left once they are at their narrowest.
                let squeezable = if dress.meter {
                    METER_W - METER_W_MIN
                } else {
                    0.0
                } + if dress.mixer { slider_give } else { 0.0 };
                let spare = room - (dressed - squeezable) - ui.spacing().item_spacing.x;
                selection_readout(ui, a, b, total, spare);
            }

            // Spend what give there is on the deficit, the meter before the
            // sliders, and stop as soon as the row fits.
            let mut short_by = (dressed - ui.available_width()).max(0.0);
            let meter_squeeze = if dress.meter {
                short_by.min(METER_W - METER_W_MIN)
            } else {
                0.0
            };
            let meter_w = METER_W - meter_squeeze;
            short_by -= meter_squeeze;
            let slider_w = (full_slider - short_by / 2.0).clamp(SLIDER_W_MIN, full_slider);
            let group = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().slider_width = slider_w;
                // The value boxes sit in a right-to-left layout, so anything
                // that changes their width shoves the rest of the bar sideways
                // as you drag. Monospace digits plus a padded, fixed-length
                // format keep each box the same size at every value.
                ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
                sidebar_toggle(app, ui);
                ui.separator();
                if dress.mixer {
                    gain_control(app, ui);
                    pan_control(app, ui);
                    ui.separator();
                }
                if dress.meter {
                    out_meter(app, ui, meter_w);
                    ui.separator();
                }
                if settings_button(ui, app.settings_open)
                    .on_hover_text("Settings (Ctrl+,)")
                    .clicked()
                {
                    app.settings_open = !app.settings_open;
                }
                if dress.chrome {
                    camera_button(app, ui);
                    help_button(app, ui);
                }
                if dress.overflowing() {
                    overflow_menu(app, ui, dress);
                }
                if dress.follow {
                    ui.separator();
                    follow_box(app, ui);
                }
                update_notice(app, ui);
            });
            // What it would have wanted at full size — not what it took —
            // so that a squeezed slider cannot read as room next frame and
            // set the two widths oscillating.
            // What the group would have wanted at full dress: what it took,
            // plus the squeeze it was put under, plus what it was not
            // wearing. Dress-independent by construction, so the dress it
            // decides next frame cannot feed on the one it was drawn in.
            let full = group.response.rect.width()
                + (METER_W - meter_w)
                + if dress.mixer {
                    2.0 * (full_slider - slider_w)
                } else {
                    0.0
                }
                + dress.shed_width();
            ui.ctx()
                .data_mut(|d| d.insert_temp(bar_fit_id(), BarFit { full, dress }));
        });
    });
}

/// The gain slider and its label, which resets it. Drawn in the bar, or in
/// the ⋯ menu when the bar has no room for it.
///
/// Always signed: a bare "0.0" beside a slider that runs both ways says
/// nothing about which side of neutral you are on.
fn gain_control(app: &mut App, ui: &mut egui::Ui) {
    let tint = gain_tint(app.settings.gain_db);
    let gain = egui::Slider::new(&mut app.settings.gain_db, -60.0..=12.0)
        .suffix(" dB")
        .custom_formatter(|n, _| format!("{n:>+5.1}"))
        .custom_parser(|s| s.trim().parse().ok());
    let mut changed = tinted_slider(ui, tint, gain).changed();
    if reset_label(ui, "Gain", "+0.0 dB").clicked() {
        app.settings.gain_db = 0.0;
        changed = true;
    }
    if changed && let Some(e) = &app.engine {
        e.shared.set_gain(db_to_amp(app.settings.gain_db));
    }
}

/// The pan slider and its label, which centres it.
fn pan_control(app: &mut App, ui: &mut egui::Ui) {
    let tint = pan_tint(app.settings.pan);
    let pan = egui::Slider::new(&mut app.settings.pan, -1.0..=1.0)
        .custom_formatter(|n, _| format!("{n:>+5.2}"))
        .custom_parser(|s| s.trim().parse().ok());
    let mut changed = tinted_slider(ui, tint, pan).changed();
    if reset_label(ui, "Pan", "centre").clicked() {
        app.settings.pan = 0.0;
        changed = true;
    }
    if changed && let Some(e) = &app.engine {
        e.shared.set_pan(app.settings.pan);
    }
}

/// The Follow box: the same setting as the Playback card's, reached here
/// mid-listen.
fn follow_box(app: &mut App, ui: &mut egui::Ui) {
    let follow = ui
        .checkbox(&mut app.settings.follow_playhead, "Follow")
        .on_hover_text("Keep the playhead in view while playing");
    help::offer_response(ui, &follow, Topic::FollowPlayhead);
}

/// The side-panel button and what clicking it does. Disabled, with a word
/// about why, on a window too narrow to hold the panel at all — the panel is
/// collapsed there whatever the setting says.
fn sidebar_toggle(app: &mut App, ui: &mut egui::Ui) {
    let room = app.sidebar_has_room(ui.ctx());
    let open = app.settings.sidebar_open && room;
    let resp = sidebar_button(ui, open, room);
    let resp = if !room {
        resp.on_hover_text("Too narrow a window for the readouts")
    } else if open {
        resp.on_hover_text("Hide the readouts")
    } else {
        resp.on_hover_text("Show the readouts")
    };
    help::offer_response(ui, &resp, Topic::SidePanel);
    if resp.clicked() && room {
        app.settings.sidebar_open = !app.settings.sidebar_open;
    }
}

/// The help-mode button.
fn help_button(app: &mut App, ui: &mut egui::Ui) {
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
}

/// Everything the row had to shed, behind one button.
fn overflow_menu(app: &mut App, ui: &mut egui::Ui, dress: Dress) {
    ui.menu_button(
        RichText::new(fonts::icon::MORE).size(fonts::HEADING),
        |ui| {
            ui.set_min_width(OVERFLOW_MENU_W);
            overflow_items(app, ui, dress);
        },
    )
    .response
    .on_hover_text("What the window is too narrow to show");
}

/// What sits in that menu: whatever this dress left out of the bar. The
/// sliders keep their own right-to-left rows, so they read here the way they
/// read in the bar.
fn overflow_items(app: &mut App, ui: &mut egui::Ui, dress: Dress) {
    if !dress.mixer {
        // Each on a row of its own, laid right to left so the label sits to
        // the left of its slider as it does in the bar. The row's height is
        // given rather than taken: a bare `with_layout` would claim all the
        // height a menu can open to and centre one slider in it.
        let row = vec2(ui.available_width(), ui.spacing().interact_size.y);
        let right = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(row, right, |ui| gain_control(app, ui));
        ui.allocate_ui_with_layout(row, right, |ui| pan_control(app, ui));
        ui.separator();
    }
    if !dress.follow {
        follow_box(app, ui);
    }
    if !dress.chrome {
        ui.checkbox(&mut app.help_mode, "Help mode (F1)");
        let taking = app.capture.is_some();
        if ui
            .add_enabled(!taking, egui::Button::new("Save a picture…"))
            .on_hover_text("The views as a PNG, with a JSON beside it (Ctrl+Shift+S)")
            .clicked()
        {
            app.save_screenshot();
            ui.close();
        }
    }
}

/// Width the overflow menu opens at: enough for a slider and its label to sit
/// on one line, as they do in the bar.
const OVERFLOW_MENU_W: f32 = 230.0;

/// The selected range, in the longest form that fits `spare`: both ends and
/// the length, the length on its own, or — when there is not even room for
/// that — nothing, the ruler's own band being the readout of last resort.
fn selection_readout(ui: &mut egui::Ui, a: f64, b: f64, total: f64, spare: f32) {
    let long = format!(
        "{} – {}  ({})",
        fmt_time_field(a, total),
        fmt_time_field(b, total),
        fmt_time_field(b - a, total)
    );
    let short = format!("({})", fmt_time_field(b - a, total));
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let width = |t: &str| {
        ui.painter()
            .layout_no_wrap(t.to_owned(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    let (long_w, short_w) = (width(&long), width(&short));
    let text = if long_w <= spare {
        long
    } else if short_w <= spare {
        short
    } else {
        return;
    };
    ui.label(
        RichText::new(text)
            .monospace()
            .color(Color32::from_gray(170)),
    );
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
fn out_meter(app: &App, ui: &mut egui::Ui, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
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
/// The side-panel button: a window with its right-hand column filled in while
/// the panel is open and hollow while it is not. Painted like the buttons
/// beside it rather than set as a glyph, so it carries the same weight.
///
/// It sits at the right end of the bar, against the panel it opens, and it is
/// never shed: it is how the window's space is won back.
fn sidebar_button(ui: &mut egui::Ui, open: bool, enabled: bool) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(vec2(28.0, ROW_H), Sense::click());
    let visuals = ui.style().visuals.clone();
    let fg = if !enabled {
        Color32::from_gray(110)
    } else if open {
        visuals.widgets.active.fg_stroke.color
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        Color32::from_gray(215)
    };
    if enabled && (open || resp.hovered()) {
        let bg = if open {
            visuals.widgets.active.weak_bg_fill
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(resp.rect, 4.0, bg);
    }
    let c = resp.rect.center();
    let body = Rect::from_center_size(c, vec2(17.0, 13.0));
    painter.rect_stroke(body, 2.0, Stroke::new(1.3, fg), egui::StrokeKind::Inside);
    // The column that is the panel: filled when it is there to be seen.
    let split = body.right() - 6.0;
    if open {
        painter.rect_filled(
            Rect::from_min_max(pos2(split, body.top()), body.max),
            0.0,
            fg,
        );
    } else {
        painter.line_segment(
            [pos2(split, body.top()), pos2(split, body.bottom())],
            Stroke::new(1.0, fg.gamma_multiply(0.6)),
        );
    }
    resp
}

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
