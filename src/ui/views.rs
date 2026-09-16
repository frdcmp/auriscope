//! Time ruler, waveform and spectrogram. They share one `View` and one set
//! of mouse gestures: click seeks, drag selects, wheel zooms at the pointer,
//! shift-wheel pans, alt-shift-wheel scales the waveform vertically, ctrl-wheel
//! or pinch also zoom. Zoomed in past ~4 samples per pixel the waveform is
//! drawn as a polyline through the samples; further out as a min/max envelope.

use eframe::egui;
use egui::{
    Align2, Color32, FontId, Pos2, Rangef, Rect, Sense, Stroke, TextureOptions, pos2, vec2,
};

use auriscope::analysis::stats::CLIP_THRESHOLD;
use auriscope::analysis::{ViewParams, render_view};

use super::util::{fmt_hz, fmt_time, fmt_time_step, nice_time_step};
use super::{App, View};

/// Everything that, when it changes, invalidates a spectrogram texture.
#[derive(Debug, Clone, PartialEq)]
pub struct TexKey {
    view: View,
    size: [usize; 2],
    params: ViewParams,
    colormap: auriscope::analysis::ColorMap,
    spec_ptr: usize,
}

const RULER_H: f32 = 22.0;
const BG: Color32 = Color32::from_rgb(18, 18, 22);
const WAVE_FILL: Color32 = Color32::from_rgb(86, 156, 214);
const WAVE_RMS: Color32 = Color32::from_rgb(160, 210, 255);
const PLAYHEAD: Color32 = Color32::from_rgb(255, 210, 80);
const SELECTION: Color32 = Color32::from_rgba_premultiplied(60, 90, 140, 70);
const LOOP_EDGE: Color32 = Color32::from_rgb(120, 220, 140);
const CLIP: Color32 = Color32::from_rgb(255, 70, 70);
const CLIP_RMS: Color32 = Color32::from_rgb(255, 150, 150);

pub fn central(app: &mut App, root: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(BG))
        .show(root, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let avail = ui.available_size();
            if avail.y < RULER_H + 40.0 {
                return;
            }
            let body_h = avail.y - RULER_H;
            let wave_h = (body_h * app.settings.waveform_fraction).clamp(60.0, body_h - 60.0);
            let spec_h = body_h - wave_h;

            let (ruler_resp, ruler_p) =
                ui.allocate_painter(vec2(avail.x, RULER_H), Sense::click_and_drag());
            let (wave_resp, wave_p) =
                ui.allocate_painter(vec2(avail.x, wave_h), Sense::click_and_drag());
            let (spec_resp, spec_p) =
                ui.allocate_painter(vec2(avail.x, spec_h), Sense::click_and_drag());

            let total = app.total_frames();
            app.view.clamp_to(total);
            app.hover_info.clear();

            // Interaction is shared: any of the three strips drives the view.
            // Only the waveform strip gets the vertical-zoom gesture.
            for (resp, is_wave) in [
                (&ruler_resp, false),
                (&wave_resp, true),
                (&spec_resp, false),
            ] {
                interact(app, ui, resp, is_wave);
            }

            draw_ruler(app, &ruler_p, ruler_resp.rect);
            draw_waveform(app, &wave_p, wave_resp.rect);
            draw_spectrogram(app, ui, &spec_p, spec_resp.rect);

            // Overlays across both.
            let full = Rect::from_min_max(wave_resp.rect.min, spec_resp.rect.max);
            draw_overlays(app, &ui.painter_at(full), full);

            // Hover readout.
            if let Some(pos) = spec_resp.hover_pos() {
                spectrogram_hover(app, pos, spec_resp.rect);
            } else if let Some(pos) = wave_resp.hover_pos().or(ruler_resp.hover_pos()) {
                let f = x_to_frame(&app.view, full, pos.x);
                app.hover_info = fmt_time(f / app.sample_rate());
            }
        });
}

// ---- geometry --------------------------------------------------------------

pub fn frame_to_x(view: &View, rect: Rect, frame: f64) -> f32 {
    rect.left() + ((frame - view.start) / view.len()) as f32 * rect.width()
}

pub fn x_to_frame(view: &View, rect: Rect, x: f32) -> f64 {
    view.start + ((x - rect.left()) / rect.width().max(1.0)) as f64 * view.len()
}

// ---- input -----------------------------------------------------------------

fn interact(app: &mut App, ui: &egui::Ui, resp: &egui::Response, is_wave: bool) {
    if app.audio.is_none() {
        return;
    }
    let rect = resp.rect;
    let total = app.total_frames();

    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let f = x_to_frame(&app.view, rect, p.x).clamp(0.0, total);
        app.selection = Some((f, f));
    }
    if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some((a, _)) = app.selection
    {
        let f = x_to_frame(&app.view, rect, p.x).clamp(0.0, total);
        app.selection = Some((a, f));
    }
    if resp.drag_stopped()
        && let Some((a, b)) = app.selection
    {
        let px = ((b - a) / app.view.len()) as f32 * rect.width();
        if px.abs() < 3.0 {
            // A drag that went nowhere is a click: seek there.
            app.selection = None;
            app.seek_frames(a);
        }
        app.apply_loop();
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let f = x_to_frame(&app.view, rect, p.x);
        app.seek_frames(f);
    }
    if resp.secondary_clicked() {
        app.selection = None;
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
                // Sound Forge style: Alt+Shift+wheel scales the waveform
                // vertically (around the zero line) without touching the
                // time axis.
                if is_wave && scroll.y.abs() > 0.0 {
                    let factor = (scroll.y / 200.0).exp();
                    if (factor - 1.0).abs() > 1e-4 {
                        app.settings.wave_v_zoom =
                            (app.settings.wave_v_zoom * factor).clamp(0.25, 64.0);
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

fn pan(app: &mut App, rect: Rect, total: f64, amount: f32) {
    if amount.abs() > 0.0 {
        let frames = -(amount / rect.width().max(1.0)) as f64 * app.view.len();
        app.view.start += frames;
        app.view.end += frames;
        app.view.clamp_to(total);
    }
}

// ---- ruler -----------------------------------------------------------------

fn draw_ruler(app: &App, p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, Color32::from_rgb(28, 28, 34));
    if app.audio.is_none() {
        return;
    }
    let sr = app.sample_rate();
    let visible_secs = app.view.len() / sr;
    let step = nice_time_step(visible_secs, rect.width(), 90.0);
    let start_secs = app.view.start / sr;
    let first = (start_secs / step).floor() * step;
    let mut t = first;
    let font = FontId::monospace(10.0);
    let minor = step / 5.0;
    while t <= start_secs + visible_secs + step {
        let x = frame_to_x(&app.view, rect, t * sr);
        if x >= rect.left() - 1.0 && x <= rect.right() + 1.0 {
            p.vline(
                x,
                Rangef::new(rect.bottom() - 8.0, rect.bottom()),
                Stroke::new(1.0, Color32::from_gray(150)),
            );
            p.text(
                pos2(x + 3.0, rect.top() + 2.0),
                Align2::LEFT_TOP,
                fmt_time_step(t, step),
                font.clone(),
                Color32::from_gray(200),
            );
        }
        for k in 1..5 {
            let xm = frame_to_x(&app.view, rect, (t + minor * k as f64) * sr);
            if xm >= rect.left() && xm <= rect.right() {
                p.vline(
                    xm,
                    Rangef::new(rect.bottom() - 4.0, rect.bottom()),
                    Stroke::new(1.0, Color32::from_gray(90)),
                );
            }
        }
        t += step;
    }
}

// ---- waveform --------------------------------------------------------------

fn draw_waveform(app: &App, p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, BG);
    let (Some(audio), Some(pyr)) = (&app.audio, &app.pyramid) else {
        if app.audio.is_some() {
            p.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "building waveform…",
                FontId::proportional(13.0),
                Color32::from_gray(120),
            );
        }
        return;
    };
    let nch = audio.channels.len();
    let ch_h = rect.height() / nch as f32;
    let width = rect.width().max(1.0) as usize;
    let show_rms = app.settings.show_rms;
    let vz = app.settings.wave_v_zoom;

    for ch in 0..nch {
        let top = rect.top() + ch_h * ch as f32;
        let mid = top + ch_h / 2.0;
        let half = ch_h / 2.0 - 2.0;
        p.hline(
            rect.x_range(),
            mid,
            Stroke::new(1.0, Color32::from_gray(50)),
        );
        if ch > 0 {
            p.hline(
                rect.x_range(),
                top,
                Stroke::new(1.0, Color32::from_gray(40)),
            );
        }
        let muted = match app.solo {
            Some(s) => s != ch,
            None => app.mutes.get(ch).copied().unwrap_or(false),
        };
        let fill = if muted {
            Color32::from_gray(70)
        } else {
            WAVE_FILL
        };
        let rms_c = if muted {
            Color32::from_gray(110)
        } else {
            WAVE_RMS
        };
        // Zoomed in far enough that a pixel spans only a few samples: draw
        // the actual signal, a polyline through every sample (Sound
        // Forge/Praat style), instead of the min/max envelope.
        let spp = app.view.len() / width.max(1) as f64;
        if spp <= 4.0 {
            let samples = &audio.channels[ch];
            let a = app.view.start.floor().max(0.0) as usize;
            let b = (app.view.end.ceil().max(0.0) as usize).min(samples.len());
            let pts: Vec<Pos2> = samples[a..b]
                .iter()
                .enumerate()
                .map(|(i, &s)| {
                    pos2(
                        frame_to_x(&app.view, rect, (a + i) as f64),
                        mid - (s * vz).clamp(-1.0, 1.0) * half,
                    )
                })
                .collect();
            if pts.len() >= 2 {
                p.add(egui::Shape::line(pts, Stroke::new(1.0, fill)));
            } else if let Some(&pt) = pts.first() {
                p.circle_filled(pt, 1.5, fill);
            }
        } else {
            let bins = pyr.query(ch, &audio.channels[ch], app.view.start, app.view.end, width);
            for (i, b) in bins.iter().enumerate() {
                let x = rect.left() + i as f32 + 0.5;
                let y0 = mid - (b.max * vz).clamp(-1.0, 1.0) * half;
                let y1 = mid - (b.min * vz).clamp(-1.0, 1.0) * half;
                let clipped = b.max >= CLIP_THRESHOLD || b.min <= -CLIP_THRESHOLD;
                p.vline(
                    x,
                    Rangef::new(y0.min(y1), y1.max(y0 + 1.0)),
                    Stroke::new(1.0, if clipped { CLIP } else { fill }),
                );
                if show_rms && b.rms > 0.0 {
                    let r = (b.rms * vz).min(1.0) * half;
                    let c = if clipped { CLIP_RMS } else { rms_c };
                    p.vline(x, Rangef::new(mid - r, mid + r), Stroke::new(1.0, c));
                }
            }
        }
        p.text(
            pos2(rect.left() + 4.0, top + 2.0),
            Align2::LEFT_TOP,
            channel_label(ch, nch),
            FontId::monospace(10.0),
            Color32::from_gray(170),
        );
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

fn draw_spectrogram(app: &mut App, ui: &egui::Ui, p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 0.0, Color32::BLACK);
    let Some(audio) = app.audio.clone() else {
        p.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Drop an audio file here, or press Ctrl+O",
            FontId::proportional(15.0),
            Color32::from_gray(120),
        );
        return;
    };
    let nch = audio.channels.len();
    let ch_h = rect.height() / nch as f32;
    let lut = app.settings.colormap.lut();
    let ppp = ui.ctx().pixels_per_point();

    for ch in 0..nch {
        let crect = Rect::from_min_size(
            pos2(rect.left(), rect.top() + ch_h * ch as f32),
            vec2(rect.width(), ch_h),
        );
        let Some(spec) = app.spectrograms.get(ch).cloned().flatten() else {
            let msg = match &app.spec_job_progress {
                _ if app.job.is_some() => "computing spectrogram…",
                _ => "no spectrogram",
            };
            p.text(
                crect.center(),
                Align2::CENTER_CENTER,
                msg,
                FontId::proportional(13.0),
                Color32::from_gray(120),
            );
            continue;
        };
        let size = [
            ((crect.width() * ppp).round() as usize).clamp(1, 8192),
            ((crect.height() * ppp).round() as usize).clamp(1, 8192),
        ];
        let params = spec_view_params(app, spec.nyquist());
        let key = TexKey {
            view: app.view,
            size,
            params: params.clone(),
            colormap: app.settings.colormap,
            spec_ptr: std::sync::Arc::as_ptr(&spec) as usize,
        };
        let slot = &mut app.spec_textures[ch];
        let needs = match slot {
            Some((_, k)) => *k != key,
            None => true,
        };
        if needs {
            let img = render_view(&spec, &params, size[0], size[1], &lut);
            match slot {
                Some((tex, k)) => {
                    tex.set(img, TextureOptions::LINEAR);
                    *k = key;
                }
                None => {
                    let tex =
                        ui.ctx()
                            .load_texture(format!("spec{ch}"), img, TextureOptions::LINEAR);
                    *slot = Some((tex, key));
                }
            }
        }
        if let Some((tex, _)) = slot {
            p.image(
                tex.id(),
                crect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        draw_freq_axis(app, p, crect, spec.nyquist());
        if ch > 0 {
            p.hline(
                rect.x_range(),
                crect.top(),
                Stroke::new(1.0, Color32::from_gray(60)),
            );
        }
        p.text(
            pos2(crect.right() - 4.0, crect.top() + 2.0),
            Align2::RIGHT_TOP,
            channel_label(ch, nch),
            FontId::monospace(10.0),
            Color32::from_gray(200),
        );
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

fn draw_freq_axis(app: &App, p: &egui::Painter, rect: Rect, nyquist: f32) {
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
    let font = FontId::monospace(9.0);
    let mut last_y = f32::MAX;
    for &hz in ticks {
        if hz >= nyquist || hz < app.settings.min_hz {
            continue;
        }
        let y = hz_to_y(app, rect, hz, nyquist);
        if (last_y - y).abs() < 12.0 || y < rect.top() + 6.0 || y > rect.bottom() - 2.0 {
            continue;
        }
        last_y = y;
        p.hline(
            Rangef::new(rect.left(), rect.left() + 6.0),
            y,
            Stroke::new(1.0, Color32::from_gray(180)),
        );
        p.text(
            pos2(rect.left() + 8.0, y),
            Align2::LEFT_CENTER,
            fmt_hz(hz),
            font.clone(),
            Color32::from_gray(210),
        );
    }
}

fn spectrogram_hover(app: &mut App, pos: Pos2, rect: Rect) {
    let Some(audio) = &app.audio else {
        return;
    };
    let nch = audio.channels.len();
    let ch_h = rect.height() / nch as f32;
    let ch = (((pos.y - rect.top()) / ch_h) as usize).min(nch - 1);
    let crect = Rect::from_min_size(
        pos2(rect.left(), rect.top() + ch_h * ch as f32),
        vec2(rect.width(), ch_h),
    );
    let frame = x_to_frame(&app.view, rect, pos.x);
    let secs = frame / app.sample_rate();
    let Some(spec) = app.spectrograms.get(ch).cloned().flatten() else {
        app.hover_info = fmt_time(secs);
        return;
    };
    let hz = y_to_hz(app, crect, pos.y, spec.nyquist());
    let col = (frame / spec.params.hop() as f64).round().max(0.0) as usize;
    let bin = (hz / spec.bin_hz(1)).round() as usize;
    let db = spec.db_at(
        col.min(spec.columns().saturating_sub(1)),
        bin.min(spec.bins - 1),
    );
    app.hover_info = format!(
        "{}   {} Hz   {:.1} dB   ({})",
        fmt_time(secs),
        fmt_hz_full(hz),
        db,
        channel_label(ch, nch)
    );
}

fn fmt_hz_full(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.2} k", hz / 1000.0)
    } else {
        format!("{hz:.1}")
    }
}

// ---- overlays --------------------------------------------------------------

fn draw_overlays(app: &App, p: &egui::Painter, rect: Rect) {
    if app.audio.is_none() {
        return;
    }
    if let Some((a, b)) = app.selection {
        let x0 = frame_to_x(&app.view, rect, a.min(b));
        let x1 = frame_to_x(&app.view, rect, a.max(b));
        let sel = Rect::from_min_max(
            pos2(x0.max(rect.left()), rect.top()),
            pos2(x1.min(rect.right()), rect.bottom()),
        );
        if sel.width() > 0.0 {
            p.rect_filled(sel, 0.0, SELECTION);
        }
        let edge = if app.loop_enabled {
            LOOP_EDGE
        } else {
            Color32::from_gray(200)
        };
        for x in [x0, x1] {
            if x >= rect.left() && x <= rect.right() {
                p.vline(x, rect.y_range(), Stroke::new(1.0, edge));
            }
        }
        if app.loop_enabled {
            p.text(
                pos2(x0.max(rect.left()) + 4.0, rect.top() + 4.0),
                Align2::LEFT_TOP,
                "LOOP",
                FontId::monospace(10.0),
                LOOP_EDGE,
            );
        }
    }
    if let Some(engine) = &app.engine {
        let ph = engine.playhead() as f64;
        let x = frame_to_x(&app.view, rect, ph);
        if x >= rect.left() && x <= rect.right() {
            p.vline(x, rect.y_range(), Stroke::new(1.5, PLAYHEAD));
        }
    }
}
