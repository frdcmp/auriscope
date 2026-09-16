//! Time ruler, waveform and spectrogram. They share one `View` and one set
//! of mouse gestures: click seeks, drag selects, wheel zooms at the pointer,
//! shift-wheel pans, alt-shift-wheel scales the waveform vertically, ctrl-wheel
//! or pinch also zoom. Zoomed in past ~4 samples per pixel the waveform is
//! drawn as a polyline through the samples; further out as a min/max envelope.

use eframe::egui;
use egui::{
    Align2, Color32, FontId, Pos2, Rangef, Rect, Sense, Shape, Stroke, TextureOptions, pos2, vec2,
};

use auriscope::analysis::stats::CLIP_THRESHOLD;
use auriscope::analysis::{ViewParams, render_view};

use super::fonts;
use super::util::{fmt_hz, fmt_hz_full, fmt_time_field, fmt_time_step, nice_time_step};
use super::{App, View};

/// A spectrogram image being rendered on a worker thread.
pub struct SpecRender {
    pub key: TexKey,
    pub rx: std::sync::mpsc::Receiver<egui::ColorImage>,
}

/// Everything that, when it changes, invalidates a spectrogram texture.
#[derive(Debug, Clone, PartialEq)]
pub struct TexKey {
    view: View,
    size: [usize; 2],
    params: ViewParams,
    colormap: auriscope::analysis::ColorMap,
    custom_stops: auriscope::analysis::CustomStops,
    contrast_bits: u32,
    spec_ptr: usize,
    /// Identity of the detail tile in use, so the texture is rebuilt the
    /// moment a finer one arrives.
    detail_ptr: usize,
}

const RULER_H: f32 = 22.0;
/// Drop from the top of the ruler to the text in it. Shared with the axis
/// caps, which sit in the ruler's corners and have to line up with it.
const RULER_PAD: f32 = 2.0;
/// Height of the draggable divider between the waveform and the spectrogram.
const SPLITTER_H: f32 = 7.0;
/// Neither strip may be dragged below this.
const MIN_STRIP: f32 = 48.0;
/// Waveform vertical-zoom limits. The top end is about 72 dB of boost, enough
/// to lift a noise floor to full height. The bottom end is 1x — full scale
/// fills the strip — because the only thing below it is empty air above
/// 0 dBFS, where nothing but the odd float sample over the top can live.
pub const V_ZOOM_MIN: f32 = 1.0;
pub const V_ZOOM_MAX: f32 = 4096.0;
const BG: Color32 = Color32::from_rgb(18, 18, 22);
/// RMS overlay colour for a given waveform colour: the same hue, lifted
/// halfway to white so it reads on top of the peaks.
fn rms_color(wave: Color32) -> Color32 {
    let lift = |v: u8| (v as u16 + (255 - v as u16) / 2) as u8;
    Color32::from_rgb(lift(wave.r()), lift(wave.g()), lift(wave.b()))
}
const PLAYHEAD: Color32 = Color32::from_rgb(255, 210, 80);
/// Highlight wash: a faint cool white at about 10 % alpha, so the waveform
/// and spectrogram stay fully legible underneath. Premultiplied because the
/// constructor has to be const: (170, 200, 240) at alpha 26.
const SELECTION: Color32 = Color32::from_rgba_premultiplied(17, 20, 24, 26);
/// Edge lines of the highlight: (200, 225, 255) at alpha 150.
const SELECTION_EDGE: Color32 = Color32::from_rgba_premultiplied(118, 132, 150, 150);
const LOOP_EDGE: Color32 = Color32::from_rgb(120, 220, 140);
const RANGE_EDGE: Color32 = Color32::from_rgb(150, 190, 235);
const CLIP: Color32 = Color32::from_rgb(255, 70, 70);
const CLIP_RMS: Color32 = Color32::from_rgb(255, 150, 150);
const DB_AXIS: Color32 = Color32::from_rgb(150, 190, 225);
/// The chrome the views sit in. The ruler across the top, the splitter
/// between the strips and the axis gutters down the sides all share it, so
/// the scales read as a frame around the signal rather than as more of it.
const CHROME: Color32 = Color32::from_rgb(28, 28, 34);
const SPLITTER_BG: Color32 = CHROME;
const SPLITTER_BG_HOT: Color32 = Color32::from_rgb(58, 58, 70);
const SPLITTER_GRIP: Color32 = Color32::from_gray(110);
const SPLITTER_GRIP_HOT: Color32 = Color32::from_gray(215);
/// How much of its brightness a muted channel's spectrogram keeps: enough to
/// see that the content is still there, little enough to read as switched off.
const MUTED_SPEC: f32 = 0.3;
/// Length of an axis tick, measured out from the edge of the signal.
const AXIS_TICK: f32 = 4.0;
/// Gap between a tick and the label that names it. Wide enough that the tick
/// reads as a tick and not as the minus sign the dB labels do without.
const AXIS_GAP: f32 = 4.0;
/// Drop from the top of a column to its cap. Two points below where the ruler
/// hangs its times: the cap is a heading for what is underneath it, not
/// another entry in the row it happens to share.
const AXIS_CAP_PAD: f32 = RULER_PAD + 2.0;
/// The band a column's cap occupies at the top of it.
const AXIS_CAP_H: f32 = 18.0;
/// Hairline between a gutter and the signal it labels.
const AXIS_EDGE: Color32 = Color32::from_gray(48);
/// Frequency ticks and their labels: the tick is a hint, the number is the
/// thing being read, so only the number is bright.
const FREQ_TICK: Color32 = Color32::from_gray(110);
const FREQ_TEXT: Color32 = Color32::from_gray(200);

/// Draw the ruler, waveform and spectrogram. Returns the rect they occupy,
/// which is what a window capture is cropped to.
pub fn central(app: &mut App, root: &mut egui::Ui) -> Rect {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(BG))
        .show(root, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let avail = ui.available_size();
            let show_w = app.settings.show_waveform;
            let show_s = app.settings.show_spectrogram;

            if !show_w && !show_s {
                let (_, p) = ui.allocate_painter(avail, Sense::hover());
                p.text(
                    ui.max_rect().center(),
                    Align2::CENTER_CENTER,
                    "Waveform and spectrogram are both hidden\nOpen Settings (Ctrl+,) to bring one back",
                    FontId::proportional(fonts::BODY),
                    Color32::from_gray(130),
                );
                return;
            }
            if avail.y < RULER_H + MIN_STRIP {
                return;
            }

            // Merged draws the waveform over the spectrogram as one pane, so
            // it needs neither a second strip nor a divider.
            let both = show_w && show_s;
            let merged = both && app.settings.merge_views;
            let split_needed = both && !merged;
            let split_h = if split_needed { SPLITTER_H } else { 0.0 };
            let body_h = (avail.y - RULER_H - split_h).max(1.0);
            let (wave_h, spec_h) = if split_needed {
                // Order the clamp bounds explicitly: a very short window would
                // otherwise give a min above the max.
                let lo = MIN_STRIP.min(body_h * 0.5);
                let hi = (body_h - MIN_STRIP).max(lo);
                let w = (body_h * app.settings.waveform_fraction).clamp(lo, hi);
                (w, body_h - w)
            } else if show_s {
                (0.0, body_h)
            } else {
                (body_h, 0.0)
            };

            // The axes get columns of their own rather than being written on
            // top of the signal. Merged, the two panes share one: frequency
            // down the left, amplitude down the right. Apart, each strip
            // carries its axis on the right, clear of the start of the clip.
            // Both strips reserve the same columns either way, so the time
            // axis stays aligned all the way down the window.
            let show_db = show_w && app.settings.show_db_scale;
            let gw = gutter_w(ui);
            let guts = if merged {
                Gutters {
                    left: gw,
                    right: if show_db { gw } else { 0.0 },
                }
            } else if show_s || show_db {
                Gutters {
                    left: 0.0,
                    right: gw,
                }
            } else {
                Gutters::default()
            };

            // Where the ruler meets an axis column, the corner belongs to the
            // column: it names the unit, and the time axis stops short of it.
            // The right-hand column under the ruler is the first strip's, so
            // it reads in dB whenever there is a waveform up there.
            let ruler_caps = [
                (guts.left > 0.0).then_some(Unit::Hz),
                if show_db {
                    Some(Unit::Db)
                } else if show_s && !show_w {
                    Some(Unit::Hz)
                } else {
                    None
                },
            ];

            let (ruler_resp, ruler_p) =
                ui.allocate_painter(vec2(avail.x, RULER_H), Sense::click_and_drag());
            let wave = (show_w && !merged)
                .then(|| ui.allocate_painter(vec2(avail.x, wave_h), Sense::click_and_drag()));
            let split_resp = split_needed
                .then(|| ui.allocate_response(vec2(avail.x, SPLITTER_H), Sense::click_and_drag()));
            let spec = show_s
                .then(|| ui.allocate_painter(vec2(avail.x, spec_h), Sense::click_and_drag()));

            // Drag the divider to rebalance the two strips; double-click resets.
            if let Some(sr) = &split_resp {
                if sr.double_clicked() {
                    app.settings.waveform_fraction = 0.3;
                } else if sr.dragged_by(egui::PointerButton::Primary) {
                    let dy = sr.drag_delta().y;
                    if dy != 0.0 && body_h > 0.0 {
                        let lo = MIN_STRIP.min(body_h * 0.5);
                        let hi = (body_h - MIN_STRIP).max(lo);
                        app.settings.waveform_fraction =
                            ((wave_h + dy).clamp(lo, hi) / body_h).clamp(0.02, 0.98);
                    }
                }
                if sr.hovered() || sr.dragged_by(egui::PointerButton::Primary) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
            }

            let total = app.total_frames();
            app.view.clamp_to(total);
            app.hover_info.clear();

            // Interaction is shared: any strip drives the view. Only the
            // waveform gets the vertical-zoom gesture.
            interact(app, ui, &ruler_resp, guts.plot(ruler_resp.rect), Surface::Ruler);
            if let Some((r, _)) = &wave {
                interact(app, ui, r, guts.plot(r.rect), Surface::Wave);
            }
            if let Some((r, _)) = &spec {
                let surface = if merged { Surface::Wave } else { Surface::Spec };
                interact(app, ui, r, guts.plot(r.rect), surface);
            }

            let bottom = spec
                .as_ref()
                .map(|(r, _)| r.rect.max)
                .or_else(|| wave.as_ref().map(|(r, _)| r.rect.max))
                .unwrap_or(ruler_resp.rect.max);
            let views = Rect::from_min_max(ruler_resp.rect.min, bottom);
            middle_pan(app, ui, views, guts.plot(views));

            draw_ruler(
                app,
                &ruler_p,
                ruler_resp.rect,
                guts.plot(ruler_resp.rect),
                ruler_caps,
            );
            if let Some((r, p)) = &wave {
                draw_waveform(app, p, r.rect, guts.plot(r.rect), None);
            }
            if let Some((r, p)) = &spec {
                draw_spectrogram(app, ui, p, r.rect, guts.plot(r.rect));
                if merged {
                    // After the spectrogram, so it lands on top of it.
                    let alpha = Some(app.settings.merge_opacity);
                    draw_waveform(app, p, r.rect, guts.plot(r.rect), alpha);
                }
            }
            if let Some(sr) = &split_resp {
                draw_splitter(
                    &ui.painter_at(sr.rect),
                    sr.rect,
                    sr.hovered() || sr.dragged_by(egui::PointerButton::Primary),
                );
            }

            // Overlays span whichever strips are present.
            let top = wave
                .as_ref()
                .map(|(r, _)| r.rect.min)
                .or_else(|| spec.as_ref().map(|(r, _)| r.rect.min))
                .unwrap_or(ruler_resp.rect.max);
            let full = Rect::from_min_max(top, bottom);
            let full_plot = guts.plot(full);
            draw_overlays(app, &ui.painter_at(full), full, full_plot);

            // Hover readout.
            let spec_hover = spec.as_ref().and_then(|(r, _)| r.hover_pos().map(|p| (p, r.rect)));
            if let Some((pos, rect)) = spec_hover {
                spectrogram_hover(app, pos, rect, guts.plot(rect));
            } else if let Some(pos) = wave
                .as_ref()
                .and_then(|(r, _)| r.hover_pos())
                .or(ruler_resp.hover_pos())
            {
                let f = x_to_frame(&app.view, full_plot, pos.x);
                let secs = f / app.sample_rate();
                app.hover_info = fmt_time_field(secs, app.duration_secs());
            }
        })
        .response
        .rect
}

/// What an axis column measures. Named once, at the top of the column, so the
/// numbers running down it can stay bare.
#[derive(Clone, Copy, PartialEq)]
enum Unit {
    Hz,
    Db,
}

impl Unit {
    fn label(self) -> &'static str {
        match self {
            Unit::Hz => "Hz",
            Unit::Db => "dB",
        }
    }

    /// The colour of the scale it caps, so cap and numbers read as one column.
    fn color(self) -> Color32 {
        match self {
            Unit::Hz => FREQ_TEXT,
            Unit::Db => DB_AXIS,
        }
    }
}

/// Name an axis column, centred across the top of it. Hung from the top edge
/// rather than centred in the band, so a cap in a ruler corner keeps its
/// distance from the times beside it however tall the ruler is.
fn draw_cap(p: &egui::Painter, gut: Rect, unit: Unit) {
    p.text(
        pos2(gut.center().x, gut.top() + AXIS_CAP_PAD),
        Align2::CENTER_TOP,
        unit.label(),
        FontId::monospace(fonts::RULER),
        unit.color().gamma_multiply(0.85),
    );
}

// ---- geometry --------------------------------------------------------------

/// The columns reserved either side of a strip for its axes. Every strip in
/// the window gets the same pair, so the time axis lines up between them.
#[derive(Clone, Copy, Default)]
struct Gutters {
    left: f32,
    right: f32,
}

impl Gutters {
    /// The part of `rect` the signal itself is drawn in.
    fn plot(self, rect: Rect) -> Rect {
        let l = rect.left() + self.left;
        Rect::from_min_max(
            pos2(l, rect.top()),
            pos2((rect.right() - self.right).max(l + 1.0), rect.bottom()),
        )
    }
}

/// How wide one gutter has to be: a tick, a gap, the widest label either
/// scale writes and a little air after it. Neither goes past three characters
/// — "20k", "100", "inf", "48" — so the column stays narrow at full size.
/// Measured rather than guessed, so it stays snug whatever the display scale
/// does to the type.
fn gutter_w(ui: &egui::Ui) -> f32 {
    let cw = ui
        .ctx()
        .fonts_mut(|f| f.glyph_width(&FontId::monospace(fonts::RULER), '0'));
    (AXIS_TICK + AXIS_GAP + cw * 3.0 + 2.0).ceil()
}

/// Fill the axis gutters and mark them off from the signal. Called before
/// anything is drawn in them, and only by whichever pass owns the strip's
/// background.
fn paint_gutters(p: &egui::Painter, rect: Rect, plot: Rect) {
    for gut in [
        Rect::from_min_max(rect.min, pos2(plot.left(), rect.bottom())),
        Rect::from_min_max(pos2(plot.right(), rect.top()), rect.max),
    ] {
        if gut.width() > 0.0 {
            p.rect_filled(gut, 0.0, CHROME);
        }
    }
    for x in [plot.left(), plot.right()] {
        if x > rect.left() && x < rect.right() {
            p.vline(x, rect.y_range(), Stroke::new(1.0, AXIS_EDGE));
        }
    }
}

pub fn frame_to_x(view: &View, rect: Rect, frame: f64) -> f32 {
    rect.left() + ((frame - view.start) / view.len()) as f32 * rect.width()
}

pub fn x_to_frame(view: &View, rect: Rect, x: f32) -> f64 {
    view.start + ((x - rect.left()) / rect.width().max(1.0)) as f64 * view.len()
}

// ---- input -----------------------------------------------------------------

/// Which strip a pointer interaction came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Ruler,
    Wave,
    Spec,
}

/// Grab distance for the range handles on the ruler, in points.
const HANDLE_GRAB: f32 = 8.0;

/// `plot` is the strip's signal area: the response covers the axis gutters
/// too, but every position is read against the part that carries the clip.
fn interact(app: &mut App, ui: &egui::Ui, resp: &egui::Response, plot: Rect, surface: Surface) {
    if app.audio.is_none() {
        return;
    }
    let rect = plot;
    let total = app.total_frames();
    let is_wave = surface == Surface::Wave;

    // Where the button went down, which is not where the pointer is by the
    // time egui calls the gesture a drag: it only decides that once the
    // pointer has travelled past the click threshold, and a brisk grab can be
    // tens of points clear of the handle by then. Every press-relative test
    // below uses this, so grabbing a handle works at any mouse speed.
    let press_x = ui.input(|i| i.pointer.press_origin()).map(|o| o.x);

    // Range handles live on the ruler: dragging one moves that edge.
    if surface == Surface::Ruler
        && let Some((a, b)) = app.range
    {
        let (xa, xb) = (
            frame_to_x(&app.view, rect, a),
            frame_to_x(&app.view, rect, b),
        );
        let grabbed = |x: f32| (x - xa).abs().min((x - xb).abs()) <= HANDLE_GRAB;
        let hovering = resp.hover_pos().is_some_and(|p| grabbed(p.x));
        if hovering || app.range_drag.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if app.range_drag.is_none()
            && resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(x) = press_x
            && grabbed(x)
        {
            app.range_drag = Some(usize::from((x - xa).abs() > (x - xb).abs()));
            // The highlight follows the edge being dragged, so the body shows
            // the range as it is reshaped rather than where it used to be.
            app.selection = app.range;
        }
    }
    if let Some(which) = app.range_drag {
        if resp.dragged_by(egui::PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((a, b)) = app.range
        {
            let f = x_to_frame(&app.view, rect, p.x).clamp(0.0, total);
            app.range = Some(if which == 0 { (f, b) } else { (a, f) });
            app.selection = app.range;
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary) {
            app.range_drag = None;
            app.range = app
                .range
                .map(|(a, b)| (a.min(b), a.max(b)))
                .filter(|(a, b)| b - a >= 1.0);
            app.selection = app.range;
            app.apply_loop();
        }
        return;
    }

    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(x) = press_x.or_else(|| resp.interact_pointer_pos().map(|p| p.x))
    {
        let f = x_to_frame(&app.view, rect, x).clamp(0.0, total);
        app.selection = Some((f, f));
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(p) = resp.interact_pointer_pos()
        && let Some((a, _)) = app.selection
    {
        let f = x_to_frame(&app.view, rect, p.x).clamp(0.0, total);
        app.selection = Some((a, f));
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary)
        && let Some((a, b)) = app.selection
    {
        let px = ((b - a) / app.view.len()) as f32 * rect.width();
        if px.abs() < 3.0 {
            // A drag that went nowhere is a click: seek there.
            app.selection = None;
            app.seek_frames(a);
        } else {
            // A real drag becomes the ruler range too, which is what the
            // loop plays and what survives the next click.
            app.range = Some((a.min(b), a.max(b)));
        }
        app.apply_loop();
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        // A click seeks and drops the highlight; the ruler range stays.
        app.selection = None;
        let f = x_to_frame(&app.view, rect, p.x);
        app.seek_frames(f);
    }
    if resp.secondary_clicked() {
        app.selection = None;
        app.range = None;
        app.loop_enabled = false;
        app.apply_loop();
    }

    if resp.hovered() {
        let (scroll, zoom, shift, alt) = ui.input(|i| {
            (
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        if let Some(p) = resp.hover_pos() {
            if (zoom - 1.0).abs() > 1e-4 {
                let anchor = x_to_frame(&app.view, rect, p.x);
                app.view.zoom(zoom as f64, anchor, total, 64.0);
            } else if shift && alt {
                // Alt+Shift+wheel scales the waveform vertically (around
                // the zero line) without touching the time axis.
                if is_wave && scroll.y.abs() > 0.0 {
                    // Geometric, so each notch is the same proportional step
                    // wherever you are in a range spanning four decades.
                    let factor = (scroll.y / 120.0).exp();
                    if (factor - 1.0).abs() > 1e-4 {
                        app.settings.wave_v_zoom =
                            (app.settings.wave_v_zoom * factor).clamp(V_ZOOM_MIN, V_ZOOM_MAX);
                    }
                }
            } else if shift {
                // Shift+wheel pans. Some input stacks remap shift+wheel to a
                // horizontal scroll, so pan by whichever axis carries it.
                let amount = if scroll.x.abs() > scroll.y.abs() {
                    scroll.x
                } else {
                    scroll.y
                };
                pan(app, rect, total, amount);
            } else if scroll.y.abs() >= scroll.x.abs() && scroll.y.abs() > 0.0 {
                // Plain wheel zooms at the pointer, at the same speed as
                // Ctrl+wheel (egui's scroll_zoom_speed).
                let factor = (scroll.y / 200.0).exp();
                if (factor - 1.0).abs() > 1e-4 {
                    let anchor = x_to_frame(&app.view, rect, p.x);
                    app.view.zoom(factor as f64, anchor, total, 64.0);
                }
            } else if scroll.x.abs() > 0.0 {
                pan(app, rect, total, scroll.x);
            }
        }
    }
}

/// Middle-button drag is a hand tool: grab the clip and scroll it sideways.
///
/// This reads raw pointer state rather than a `Response`, so the pan keeps
/// following the mouse once it leaves the strip the drag started on, and so a
/// single gesture cannot be applied once per strip.
fn middle_pan(app: &mut App, ui: &egui::Ui, views: Rect, plot: Rect) {
    let (down, pressed, pos, delta) = ui.input(|i| {
        (
            i.pointer.button_down(egui::PointerButton::Middle),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.interact_pos(),
            i.pointer.delta(),
        )
    });
    if !down {
        app.middle_panning = false;
        return;
    }
    if pressed && app.audio.is_some() && pos.is_some_and(|p| views.contains(p)) {
        app.middle_panning = true;
    }
    if app.middle_panning {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if delta.x != 0.0 {
            let total = app.total_frames();
            pan(app, plot, total, delta.x);
        }
    }
}

fn pan(app: &mut App, rect: Rect, total: f64, amount: f32) {
    if amount.abs() > 0.0 {
        let frames = -(amount / rect.width().max(1.0)) as f64 * app.view.len();
        app.view.start += frames;
        app.view.end += frames;
        app.view.clamp_to(total);
    }
}

// ---- ruler -----------------------------------------------------------------

/// `caps` names the axis column under each corner of the ruler, left then
/// right; a column with nothing in it under the ruler gets `None`.
fn draw_ruler(app: &App, p: &egui::Painter, rect: Rect, plot: Rect, caps: [Option<Unit>; 2]) {
    p.rect_filled(rect, 0.0, CHROME);
    if app.audio.is_none() {
        frame_ruler(p, rect, plot, caps);
        return;
    }
    let sr = app.sample_rate();
    let visible_secs = app.view.len() / sr;
    let step = nice_time_step(visible_secs, plot.width(), 90.0);
    let start_secs = app.view.start / sr;
    let first = (start_secs / step).floor() * step;
    let mut t = first;
    let font = FontId::monospace(fonts::RULER);
    let minor = step / 5.0;
    while t <= start_secs + visible_secs + step {
        let x = frame_to_x(&app.view, plot, t * sr);
        if x >= plot.left() - 1.0 && x <= plot.right() + 1.0 {
            p.vline(
                x,
                Rangef::new(rect.bottom() - 8.0, rect.bottom()),
                Stroke::new(1.0, Color32::from_gray(150)),
            );
            // A label that would run into the corner is dropped rather than
            // written over it: the tick is still there to read the time off.
            let c = Color32::from_gray(200);
            let g = p.layout_no_wrap(fmt_time_step(t, step), font.clone(), c);
            if x + 3.0 + g.size().x <= plot.right() {
                p.galley(pos2(x + 3.0, rect.top() + RULER_PAD), g, c);
            }
        }
        for k in 1..5 {
            let xm = frame_to_x(&app.view, plot, (t + minor * k as f64) * sr);
            if xm >= plot.left() && xm <= plot.right() {
                p.vline(
                    xm,
                    Rangef::new(rect.bottom() - 4.0, rect.bottom()),
                    Stroke::new(1.0, Color32::from_gray(90)),
                );
            }
        }
        t += step;
    }
    draw_range_band(app, p, rect, plot);
    frame_ruler(p, rect, plot, caps);
}

/// Rule the ruler off from the axis columns beside it and the signal below it,
/// and name each column in the corner it starts from. Drawn last, so nothing
/// the time axis paints can run through it.
fn frame_ruler(p: &egui::Painter, rect: Rect, plot: Rect, caps: [Option<Unit>; 2]) {
    let corners = [
        Rect::from_min_max(rect.min, pos2(plot.left(), rect.bottom())),
        Rect::from_min_max(pos2(plot.right(), rect.top()), rect.max),
    ];
    for (corner, cap) in corners.into_iter().zip(caps) {
        if corner.width() <= 0.0 {
            continue;
        }
        p.rect_filled(corner, 0.0, CHROME);
        if let Some(unit) = cap {
            draw_cap(p, corner, unit);
        }
    }
    for x in [plot.left(), plot.right()] {
        if x > rect.left() && x < rect.right() {
            p.vline(x, rect.y_range(), Stroke::new(1.0, AXIS_EDGE));
        }
    }
    p.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, AXIS_EDGE),
    );
}

/// The kept range on the ruler: a band with a handle at each end, green
/// when it is what the loop plays.
fn draw_range_band(app: &App, p: &egui::Painter, rect: Rect, plot: Rect) {
    let Some((a, b)) = app.range else { return };
    let (x0, x1) = (
        frame_to_x(&app.view, plot, a),
        frame_to_x(&app.view, plot, b),
    );
    if x1 < plot.left() || x0 > plot.right() {
        return;
    }
    let (fill, edge) = if app.loop_enabled {
        (
            Color32::from_rgba_unmultiplied(120, 220, 140, 60),
            LOOP_EDGE,
        )
    } else {
        (
            Color32::from_rgba_unmultiplied(86, 156, 214, 55),
            RANGE_EDGE,
        )
    };
    let band = Rect::from_min_max(
        pos2(x0.max(plot.left()), rect.top()),
        pos2(x1.min(plot.right()), rect.bottom()),
    );
    p.rect_filled(band, 0.0, fill);
    // Handles: small triangles pointing into the range.
    let h = rect.height() * 0.55;
    let w = h * 0.8;
    let top = rect.top() + 1.0;
    for (x, dir) in [(x0, 1.0f32), (x1, -1.0)] {
        if x < plot.left() || x > plot.right() {
            continue;
        }
        p.vline(x, rect.y_range(), Stroke::new(1.0, edge));
        p.add(Shape::convex_polygon(
            vec![
                pos2(x, top),
                pos2(x + dir * w, top + h * 0.5),
                pos2(x, top + h),
            ],
            edge,
            Stroke::NONE,
        ));
    }
}

// ---- waveform --------------------------------------------------------------

/// Draw the waveform. `rect` is the whole strip and `plot` the part of it
/// left over once the axis gutters are taken off; the signal never leaves
/// `plot`. `overlay` is `Some(alpha)` when drawing on top of the spectrogram
/// as a single merged pane, in which case the background, the separators, the
/// gutter edges and the channel labels are left to the spectrogram underneath.
fn draw_waveform(app: &App, p: &egui::Painter, rect: Rect, plot: Rect, overlay: Option<f32>) {
    let over = overlay.is_some();
    let alpha = (overlay.unwrap_or(1.0).clamp(0.05, 1.0) * 255.0) as u8;
    let tint = |c: Color32| -> Color32 {
        if over {
            Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
        } else {
            c
        }
    };
    if !over {
        p.rect_filled(rect, 0.0, BG);
        paint_gutters(p, rect, plot);
    }
    let (Some(audio), Some(pyr)) = (&app.audio, &app.pyramid) else {
        if app.audio.is_some() && !over {
            p.text(
                plot.center(),
                Align2::CENTER_CENTER,
                "building waveform…",
                FontId::proportional(fonts::BODY),
                Color32::from_gray(120),
            );
        }
        return;
    };
    let nch = audio.channels.len();
    // An isolated channel is the only row, so it gets the whole height.
    let chans = app.visible_channels(nch);
    let ch_h = rect.height() / chans.len() as f32;
    let width = plot.width().max(1.0) as usize;
    let show_rms = app.settings.show_rms;
    let vz = app.settings.wave_v_zoom;
    // Zoomed in far enough that a pixel spans only a few samples: draw the
    // actual signal, a polyline through every sample (Sound Forge/Praat
    // style), instead of the min/max envelope.
    let spp = app.view.len() / width.max(1) as f64;
    let per_sample = spp <= 4.0;

    for (row, ch) in chans.clone().enumerate() {
        let top = rect.top() + ch_h * row as f32;
        let mid = top + ch_h / 2.0;
        let half = ch_h / 2.0 - 2.0;
        // The zero line (-inf dBFS). Under the envelope, where it only has
        // to show through the gaps; brighter and dashed over the per-sample
        // trace, which is a hairline of the same weight and would otherwise
        // be indistinguishable from it.
        let zero_c = if over {
            Color32::from_white_alpha(if per_sample { 64 } else { 28 })
        } else {
            Color32::from_gray(if per_sample { 78 } else { 50 })
        };
        if !per_sample {
            p.hline(plot.x_range(), mid, Stroke::new(1.0, zero_c));
        }
        if !over && row > 0 {
            p.hline(
                rect.x_range(),
                top,
                Stroke::new(1.0, Color32::from_gray(40)),
            );
        }
        let muted = app.channel_muted(ch);
        let [wr, wg, wb] = app.settings.wave_color;
        let wave_c = Color32::from_rgb(wr, wg, wb);
        let fill = tint(if muted {
            Color32::from_gray(70)
        } else {
            wave_c
        });
        let rms_c = tint(if muted {
            Color32::from_gray(110)
        } else {
            rms_color(wave_c)
        });
        let clip_c = tint(CLIP);
        let clip_rms_c = tint(CLIP_RMS);
        if per_sample {
            let samples = &audio.channels[ch];
            let a = app.view.start.floor().max(0.0) as usize;
            let b = (app.view.end.ceil().max(0.0) as usize).min(samples.len());
            let pts: Vec<Pos2> = samples[a..b]
                .iter()
                .enumerate()
                .map(|(i, &s)| {
                    pos2(
                        frame_to_x(&app.view, plot, (a + i) as f64),
                        mid - (s * vz).clamp(-1.0, 1.0) * half,
                    )
                })
                .collect();
            if pts.len() >= 2 {
                p.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, fill)));
                // Clipped stretches again on top, in the clip colour. The
                // envelope path colours whole bins; here the run has to be
                // found sample by sample, or the red vanishes the moment the
                // view zooms past the envelope threshold.
                let vals = &samples[a..b];
                let mut i = 0;
                while i < vals.len() {
                    if vals[i].abs() < CLIP_THRESHOLD {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < vals.len() && vals[i].abs() >= CLIP_THRESHOLD {
                        i += 1;
                    }
                    // A sample either side, so the run joins the trace it sits
                    // in rather than floating above it.
                    let lo = start.saturating_sub(1);
                    let hi = (i + 1).min(pts.len());
                    if hi - lo >= 2 {
                        p.add(egui::Shape::line(
                            pts[lo..hi].to_vec(),
                            Stroke::new(1.0, clip_c),
                        ));
                    }
                }
            } else if let Some(&pt) = pts.first() {
                let c = if pts.len() == 1 && samples[a].abs() >= CLIP_THRESHOLD {
                    clip_c
                } else {
                    fill
                };
                p.circle_filled(pt, 1.5, c);
            }
        } else {
            let bins = pyr.query(ch, &audio.channels[ch], app.view.start, app.view.end, width);
            for (i, b) in bins.iter().enumerate() {
                let x = plot.left() + i as f32 + 0.5;
                let y0 = mid - (b.max * vz).clamp(-1.0, 1.0) * half;
                let y1 = mid - (b.min * vz).clamp(-1.0, 1.0) * half;
                let clipped = b.max >= CLIP_THRESHOLD || b.min <= -CLIP_THRESHOLD;
                p.vline(
                    x,
                    Rangef::new(y0.min(y1), y1.max(y0 + 1.0)),
                    Stroke::new(1.0, if clipped { clip_c } else { fill }),
                );
                if show_rms && b.rms > 0.0 {
                    let r = (b.rms * vz).min(1.0) * half;
                    let c = if clipped { clip_rms_c } else { rms_c };
                    p.vline(x, Rangef::new(mid - r, mid + r), Stroke::new(1.0, c));
                }
            }
        }
        if per_sample {
            p.extend(egui::Shape::dashed_line(
                &[pos2(plot.left(), mid), pos2(plot.right(), mid)],
                Stroke::new(1.0, zero_c),
                5.0,
                4.0,
            ));
        }
        // The dB scale lives in the right-hand gutter, whichever way the
        // views are arranged: on the left it would sit where the spectrogram
        // keeps its frequencies when the two are merged.
        let gut = Rect::from_min_max(pos2(plot.right(), top), pos2(rect.right(), top + ch_h));
        if app.settings.show_db_scale && gut.width() >= AXIS_TICK {
            draw_db_axis(p, gut, mid, half, vz);
        }
        // Mono needs no label: there is nothing for it to tell apart.
        if !over && nch > 1 {
            p.text(
                pos2(plot.left() + 4.0, top + 2.0),
                Align2::LEFT_TOP,
                channel_label(ch, nch),
                FontId::monospace(fonts::RULER),
                Color32::from_gray(170),
            );
        }
    }
}

/// Amplitude scale for one waveform channel, in dBFS, mirrored above and
/// below the zero line and following the vertical zoom. `gut` is the channel's
/// slice of the strip's right-hand gutter, so the scale is never drawn over
/// the signal; the ticks sit against the signal and the labels read outwards.
///
/// The labels carry no minus sign. Nothing on a dBFS scale is above zero, so
/// the sign is a given, and dropping it buys a character off the width of the
/// gutter — which is width taken from the clip on every strip.
fn draw_db_axis(p: &egui::Painter, gut: Rect, mid: f32, half: f32, vz: f32) {
    const STEPS: [f32; 14] = [
        0.0, -3.0, -6.0, -12.0, -18.0, -24.0, -30.0, -36.0, -42.0, -48.0, -60.0, -72.0, -84.0,
        -96.0,
    ];
    let font = FontId::monospace(fonts::RULER);
    let (x_tick0, x_tick1) = (gut.left(), gut.left() + AXIS_TICK);
    let x_text = x_tick1 + AXIS_GAP;
    let (top, bottom) = (gut.top(), gut.bottom());
    // 0 dBFS sits on the very edge of the strip at 1x, so the range runs to
    // within a hair of both ends.
    let y_min = top + 1.0;
    let y_max = bottom - 1.0;
    let mut placed: Vec<f32> = Vec::new();
    for db in STEPS {
        let amp = 10f32.powf(db / 20.0) * vz;
        if amp > 1.0 {
            continue;
        }
        let dy = amp * half;
        for y in [mid - dy, mid + dy] {
            if !(y_min..=y_max).contains(&y) || placed.iter().any(|&q| (q - y).abs() < 11.0) {
                continue;
            }
            placed.push(y);
            p.hline(
                Rangef::new(x_tick0, x_tick1),
                y,
                Stroke::new(1.0, DB_AXIS.gamma_multiply(0.65)),
            );
            // A tick on the very edge would have half its label outside the
            // strip, so the text alone is nudged back inside.
            let ty = y.clamp(top + 7.0, bottom - 7.0);
            p.text(
                pos2(x_text, ty),
                Align2::LEFT_CENTER,
                format!("{:.0}", db.abs()),
                font.clone(),
                DB_AXIS,
            );
            if dy < 0.5 {
                break;
            }
        }
    }
    if (y_min..=y_max).contains(&mid) && !placed.iter().any(|&q| (q - mid).abs() < 11.0) {
        p.text(
            pos2(x_text, mid),
            Align2::LEFT_CENTER,
            "inf",
            font.clone(),
            DB_AXIS.gamma_multiply(0.7),
        );
    }
}

/// The divider between the waveform and the spectrogram: a hairline at each
/// edge and a short row of grip dots, brightened while hovered or dragged.
fn draw_splitter(p: &egui::Painter, rect: Rect, active: bool) {
    p.rect_filled(
        rect,
        0.0,
        if active { SPLITTER_BG_HOT } else { SPLITTER_BG },
    );
    let edge = Stroke::new(1.0, Color32::from_gray(if active { 120 } else { 55 }));
    p.hline(rect.x_range(), rect.top() + 0.5, edge);
    p.hline(rect.x_range(), rect.bottom() - 0.5, edge);

    let c = rect.center();
    let col = if active {
        SPLITTER_GRIP_HOT
    } else {
        SPLITTER_GRIP
    };
    for dx in [-16.0, -8.0, 0.0, 8.0, 16.0] {
        p.circle_filled(pos2(c.x + dx, c.y), 1.1, col);
    }
}

/// The channel's name in full, for the places with room for it. A bare "L"
/// beside a Mute/Solo pair reads as one more letter in a row of letters,
/// especially with an RMS row underneath.
pub fn channel_name(ch: usize, nch: usize) -> String {
    match (nch, ch) {
        (1, _) => "Mono".into(),
        (2, 0) => "Left".into(),
        (2, 1) => "Right".into(),
        _ => format!("Channel {}", ch + 1),
    }
}

pub fn channel_label(ch: usize, nch: usize) -> String {
    match (nch, ch) {
        (1, _) => "M".into(),
        (2, 0) => "L".into(),
        (2, 1) => "R".into(),
        _ => format!("{}", ch + 1),
    }
}

// ---- spectrogram -----------------------------------------------------------

fn spec_view_params(app: &App, nyquist: f32) -> ViewParams {
    ViewParams {
        start_frame: app.view.start,
        end_frame: app.view.end,
        min_hz: app.settings.min_hz,
        max_hz: nyquist,
        log_frequency: app.settings.log_frequency,
        db_min: app.settings.db_min,
        db_max: app.settings.db_max,
    }
}

/// Draw a texture rendered for the view in `have` so that it lines up with
/// the view in `want`: shifted and stretched along time, which is exact for
/// the spectrogram's time axis. A frequency-axis change is drawn stretched
/// whole; it lasts one frame.
fn draw_remapped(
    p: &egui::Painter,
    tex: &egui::TextureHandle,
    have: &TexKey,
    want: &TexKey,
    rect: Rect,
    tint: Color32,
) {
    let full = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let same_freq = have.params.min_hz == want.params.min_hz
        && have.params.max_hz == want.params.max_hz
        && have.params.log_frequency == want.params.log_frequency;
    if !same_freq || have.view == want.view {
        p.image(tex.id(), rect, full, tint);
        return;
    }
    let len = want.view.len();
    let x_of = |f: f64| rect.left() + ((f - want.view.start) / len) as f32 * rect.width();
    let (x0, x1) = (x_of(have.view.start), x_of(have.view.end));
    if x1 - x0 < 1.0 {
        return;
    }
    let vis = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom())).intersect(rect);
    if vis.width() <= 0.0 {
        return;
    }
    let u0 = (vis.left() - x0) / (x1 - x0);
    let u1 = (vis.right() - x0) / (x1 - x0);
    p.image(
        tex.id(),
        vis,
        Rect::from_min_max(pos2(u0, 0.0), pos2(u1, 1.0)),
        tint,
    );
}

/// `rect` is the whole strip; `plot` is what is left of it once the axis
/// gutters are taken off, and is where the image and its labels go.
fn draw_spectrogram(app: &mut App, ui: &egui::Ui, p: &egui::Painter, rect: Rect, plot: Rect) {
    p.rect_filled(rect, 0.0, Color32::BLACK);
    paint_gutters(p, rect, plot);
    let Some(audio) = app.audio.clone() else {
        p.text(
            plot.center(),
            Align2::CENTER_CENTER,
            "Drop an audio file here, or press Ctrl+O",
            FontId::proportional(fonts::HEADING),
            Color32::from_gray(120),
        );
        return;
    };
    let nch = audio.channels.len();
    let chans = app.visible_channels(nch);
    let ch_h = rect.height() / chans.len() as f32;
    let lut = app
        .settings
        .colormap
        .lut_with(app.settings.spec_contrast, &app.settings.custom_stops);
    let merged =
        app.settings.merge_views && app.settings.show_waveform && app.settings.show_spectrogram;
    let base_opacity = if merged {
        app.settings.merge_spec_opacity.clamp(0.05, 1.0)
    } else {
        1.0
    };
    let ppp = ui.ctx().pixels_per_point();
    // Ask for high-resolution tiles if the zoom has outrun the base hop.
    app.request_detail((plot.width() * ppp).round() as usize);
    // Merged, the waveform's dB scale has the right-hand gutter, so the
    // frequencies go down the left. On its own the spectrogram keeps them on
    // the right, out of the way of the first moments of the clip.
    let freq_left = merged;
    // Split, the waveform sits between the ruler and this strip, so the
    // ruler's corner names that column and not this one: the frequency scale
    // introduces itself instead.
    let cap = app.settings.show_waveform && !merged;

    for (row, ch) in chans.clone().enumerate() {
        let crect = Rect::from_min_size(
            pos2(plot.left(), plot.top() + ch_h * row as f32),
            vec2(plot.width(), ch_h),
        );
        let gut = if freq_left {
            Rect::from_min_max(
                pos2(rect.left(), crect.top()),
                pos2(plot.left(), crect.bottom()),
            )
        } else {
            Rect::from_min_max(
                pos2(plot.right(), crect.top()),
                pos2(rect.right(), crect.bottom()),
            )
        };
        // A muted channel is dimmed the way its waveform is: the image fades
        // back towards the black behind it, so a soloed channel reads as the
        // only one still speaking.
        let muted = app.channel_muted(ch);
        let spec_tint = {
            let o = base_opacity * if muted { MUTED_SPEC } else { 1.0 };
            Color32::from_rgba_unmultiplied(255, 255, 255, (o * 255.0) as u8)
        };
        let Some(spec) = app.spectrograms.get(ch).cloned().flatten() else {
            let msg = match &app.spec_job_progress {
                _ if app.job.is_some() => "computing spectrogram…",
                _ => "no spectrogram",
            };
            p.text(
                crect.center(),
                Align2::CENTER_CENTER,
                msg,
                FontId::proportional(fonts::BODY),
                Color32::from_gray(120),
            );
            continue;
        };
        let size = [
            ((crect.width() * ppp).round() as usize).clamp(1, 8192),
            ((crect.height() * ppp).round() as usize).clamp(1, 8192),
        ];
        let params = spec_view_params(app, spec.nyquist());
        let detail = app.detail.get(ch).cloned().flatten();
        let key = TexKey {
            view: app.view,
            size,
            params: params.clone(),
            colormap: app.settings.colormap,
            custom_stops: app.settings.custom_stops,
            contrast_bits: app.settings.spec_contrast.to_bits(),
            spec_ptr: std::sync::Arc::as_ptr(&spec) as usize,
            detail_ptr: detail
                .as_ref()
                .map_or(0, |d| std::sync::Arc::as_ptr(d) as usize),
        };
        // Collect a finished render, if any.
        if let Some(job) = &app.spec_render[ch] {
            match job.rx.try_recv() {
                Ok(img) => {
                    let key = job.key.clone();
                    match &mut app.spec_textures[ch] {
                        Some((tex, k)) => {
                            tex.set(img, TextureOptions::LINEAR);
                            *k = key;
                        }
                        slot @ None => {
                            let tex = ui.ctx().load_texture(
                                format!("spec{ch}"),
                                img,
                                TextureOptions::LINEAR,
                            );
                            *slot = Some((tex, key));
                        }
                    }
                    app.spec_render[ch] = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => app.spec_render[ch] = None,
            }
        }
        // Rendering happens off the UI thread, one image in flight per
        // channel. Meanwhile the last image is drawn stretched to the
        // current view, so zooming and panning never wait on a render.
        let current = app.spec_textures[ch]
            .as_ref()
            .is_some_and(|(_, k)| *k == key);
        if !current && app.spec_render[ch].is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ui.ctx().clone();
            let (spec, detail, params, lut) =
                (spec.clone(), detail.clone(), params.clone(), lut.clone());
            std::thread::Builder::new()
                .name("auriscope-render".into())
                .spawn(move || {
                    let img =
                        render_view(&spec, detail.as_deref(), &params, size[0], size[1], &lut);
                    if tx.send(img).is_ok() {
                        ctx.request_repaint();
                    }
                })
                .ok();
            app.spec_render[ch] = Some(SpecRender {
                key: key.clone(),
                rx,
            });
        }
        if let Some((tex, k)) = &app.spec_textures[ch] {
            draw_remapped(p, tex, k, &key, crect, spec_tint);
        }
        if gut.width() >= AXIS_TICK {
            draw_freq_axis(
                app,
                p,
                crect,
                gut,
                freq_left,
                spec.nyquist(),
                cap && row == 0,
            );
        }
        if row > 0 {
            p.hline(
                rect.x_range(),
                crect.top(),
                Stroke::new(1.0, Color32::from_gray(60)),
            );
        }
        // Top left, the same corner the standalone waveform uses.
        if nch > 1 {
            p.text(
                pos2(crect.left() + 4.0, crect.top() + 2.0),
                Align2::LEFT_TOP,
                channel_label(ch, nch),
                FontId::monospace(fonts::RULER),
                Color32::from_gray(200),
            );
        }
    }
}

fn hz_to_y(app: &App, rect: Rect, hz: f32, nyquist: f32) -> f32 {
    let min = if app.settings.log_frequency {
        app.settings.min_hz.max(10.0)
    } else {
        app.settings.min_hz.max(0.0)
    };
    let t = if app.settings.log_frequency {
        (hz.max(min).ln() - min.ln()) / (nyquist.ln() - min.ln())
    } else {
        (hz - min) / (nyquist - min)
    };
    rect.bottom() - t.clamp(0.0, 1.0) * rect.height()
}

fn y_to_hz(app: &App, rect: Rect, y: f32, nyquist: f32) -> f32 {
    let min = if app.settings.log_frequency {
        app.settings.min_hz.max(10.0)
    } else {
        app.settings.min_hz.max(0.0)
    };
    let t = ((rect.bottom() - y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    if app.settings.log_frequency {
        (min.ln() + t * (nyquist.ln() - min.ln())).exp()
    } else {
        min + t * (nyquist - min)
    }
}

/// The frequency scale for one channel. `rect` is the channel's image, which
/// sets where each frequency falls; `gut` is its column of the gutter, on
/// whichever side `left` says, and is the only place anything is drawn. `cap`
/// names the column at the top of it, for when no ruler corner does.
#[allow(clippy::too_many_arguments)]
fn draw_freq_axis(
    app: &App,
    p: &egui::Painter,
    rect: Rect,
    gut: Rect,
    left: bool,
    nyquist: f32,
    cap: bool,
) {
    let ticks: &[f32] = if app.settings.log_frequency {
        &[
            20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0, 40000.0,
        ]
    } else {
        &[
            1000.0, 2000.0, 4000.0, 6000.0, 8000.0, 10000.0, 12000.0, 16000.0, 20000.0, 24000.0,
            32000.0, 40000.0, 48000.0,
        ]
    };
    let font = FontId::monospace(fonts::RULER);
    // Ticks against the image, labels reading outwards from them.
    let (x_tick0, x_tick1, x_text, align) = if left {
        (
            gut.right() - AXIS_TICK,
            gut.right(),
            gut.right() - AXIS_TICK - AXIS_GAP,
            Align2::RIGHT_CENTER,
        )
    } else {
        (
            gut.left(),
            gut.left() + AXIS_TICK,
            gut.left() + AXIS_TICK + AXIS_GAP,
            Align2::LEFT_CENTER,
        )
    };
    let y_min = if cap {
        draw_cap(p, gut, Unit::Hz);
        rect.top() + AXIS_CAP_H + 2.0
    } else {
        rect.top() + 6.0
    };
    let mut last_y = f32::MAX;
    for &hz in ticks {
        if hz >= nyquist || hz < app.settings.min_hz {
            continue;
        }
        let y = hz_to_y(app, rect, hz, nyquist);
        if (last_y - y).abs() < 12.0 || y < y_min || y > rect.bottom() - 2.0 {
            continue;
        }
        last_y = y;
        p.hline(
            Rangef::new(x_tick0, x_tick1),
            y,
            Stroke::new(1.0, FREQ_TICK),
        );
        p.text(pos2(x_text, y), align, fmt_hz(hz), font.clone(), FREQ_TEXT);
    }
}

fn spectrogram_hover(app: &mut App, pos: Pos2, rect: Rect, plot: Rect) {
    let Some(audio) = &app.audio else {
        return;
    };
    let nch = audio.channels.len();
    let chans = app.visible_channels(nch);
    let ch_h = rect.height() / chans.len() as f32;
    let row = (((pos.y - rect.top()) / ch_h) as usize).min(chans.len() - 1);
    let ch = chans.start + row;
    let crect = Rect::from_min_size(
        pos2(plot.left(), plot.top() + ch_h * row as f32),
        vec2(plot.width(), ch_h),
    );
    let frame = x_to_frame(&app.view, plot, pos.x);
    let secs = frame / app.sample_rate();
    let Some(spec) = app.spectrograms.get(ch).cloned().flatten() else {
        app.hover_info = fmt_time_field(secs, app.duration_secs());
        return;
    };
    let hz = y_to_hz(app, crect, pos.y, spec.nyquist());
    let col = (frame / spec.params.hop() as f64).round().max(0.0) as usize;
    let bin = (hz / spec.bin_hz(1)).round() as usize;
    let db = spec.db_at(
        col.min(spec.columns().saturating_sub(1)),
        bin.min(spec.bins - 1),
    );
    // Every field is right-aligned in a field wide enough for its widest
    // value: the readout is right-aligned in the status bar, so a field that
    // grows by a digit would otherwise drag everything left of it along.
    let hz_w = fmt_hz_full(spec.nyquist()).len();
    app.hover_info = format!(
        "{}   {:>hz_w$}   {db:>6.1} dB   ({})",
        fmt_time_field(secs, app.duration_secs()),
        fmt_hz_full(hz),
        channel_label(ch, nch)
    );
}

// ---- overlays --------------------------------------------------------------

/// The playhead, the highlight and the loop edges: drawn across every strip,
/// but only ever inside `plot`, so the axis gutters stay clean.
fn draw_overlays(app: &App, p: &egui::Painter, rect: Rect, plot: Rect) {
    if app.audio.is_none() {
        return;
    }
    if let Some((a, b)) = app.selection {
        let x0 = frame_to_x(&app.view, plot, a.min(b));
        let x1 = frame_to_x(&app.view, plot, a.max(b));
        let sel = Rect::from_min_max(
            pos2(x0.max(plot.left()), rect.top()),
            pos2(x1.min(plot.right()), rect.bottom()),
        );
        if sel.width() > 0.0 {
            p.rect_filled(sel, 0.0, SELECTION);
        }
        for x in [x0, x1] {
            if x >= plot.left() && x <= plot.right() {
                p.vline(x, rect.y_range(), Stroke::new(1.0, SELECTION_EDGE));
            }
        }
    }
    // The looped range shows as thin edge lines in the body, so the loop
    // points are visible without the highlight.
    if app.loop_enabled
        && let Some((a, b)) = app.range
    {
        for f in [a, b] {
            let x = frame_to_x(&app.view, plot, f);
            if x >= plot.left() && x <= plot.right() {
                p.vline(
                    x,
                    rect.y_range(),
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 220, 140, 140)),
                );
            }
        }
    }
    if let Some(engine) = &app.engine {
        let ph = engine.playhead() as f64;
        let x = frame_to_x(&app.view, plot, ph);
        if x >= plot.left() && x <= plot.right() {
            p.vline(x, rect.y_range(), Stroke::new(1.5, PLAYHEAD));
        }
    }
}
