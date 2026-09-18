//! Realtime spectrum panel.

use eframe::egui;
use egui::{Align2, Color32, FontId, Rangef, Sense, Stroke, pos2};

use super::App;
use super::fonts;
use super::util::fmt_hz;

const MIN_HZ: f32 = 20.0;
const DB_TOP: f32 = 0.0;
const DB_BOTTOM: f32 = -100.0;

/// Peak-hold line: the curve colour dimmed, so it reads as a ghost of the
/// live curve rather than a second signal in its own right. Opaque, so it
/// stays visible where it crosses a grid line.
fn peak_color(base: Color32) -> Color32 {
    let dim = |v: u8| (v as u16 * 3 / 5) as u8;
    Color32::from_rgb(dim(base.r()), dim(base.g()), dim(base.b()))
}

/// Draw the realtime spectrum. Returns the panel's rect, so a capture can
/// take it in along with the views above it.
pub fn bottom_panel(app: &mut App, root: &mut egui::Ui) -> egui::Rect {
    egui::Panel::bottom("spectrum")
        .resizable(true)
        .default_size(170.0)
        .min_size(80.0)
        .frame(egui::Frame::NONE.fill(Color32::from_rgb(14, 14, 18)))
        .show(root, |ui| {
            let size = ui.available_size();
            let (resp, p) = ui.allocate_painter(size, Sense::hover());
            let rect = resp.rect;
            p.rect_filled(rect, 0.0, Color32::from_rgb(14, 14, 18));

            let Some(live) = &app.live else {
                p.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "spectrum",
                    FontId::proportional(fonts::BODY),
                    Color32::from_gray(90),
                );
                return;
            };
            let [sr, sg, sb] = app.settings.spectrum_color;
            let curve_c = Color32::from_rgb(sr, sg, sb);
            let fill_c = curve_c.gamma_multiply(0.35);
            let peak_c = peak_color(curve_c);

            let nyq = live.sample_rate() as f32 / 2.0;
            let lmin = MIN_HZ.ln();
            let lmax = nyq.ln();
            let x_of = |hz: f32| rect.left() + (hz.ln() - lmin) / (lmax - lmin) * rect.width();
            let y_of = |db: f32| {
                rect.bottom()
                    - ((db - DB_BOTTOM) / (DB_TOP - DB_BOTTOM)).clamp(0.0, 1.0) * rect.height()
            };

            // Grid.
            let grid = Stroke::new(1.0, Color32::from_gray(38));
            for hz in [
                20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0,
            ] {
                if hz < nyq {
                    let x = x_of(hz);
                    p.vline(x, rect.y_range(), grid);
                    p.text(
                        pos2(x + 2.0, rect.bottom() - 2.0),
                        Align2::LEFT_BOTTOM,
                        fmt_hz(hz),
                        FontId::monospace(fonts::RULER),
                        Color32::from_gray(140),
                    );
                }
            }
            for db in [-20.0, -40.0, -60.0, -80.0] {
                let y = y_of(db);
                p.hline(rect.x_range(), y, grid);
                p.text(
                    pos2(rect.right() - 2.0, y),
                    Align2::RIGHT_BOTTOM,
                    format!("{db:.0}"),
                    FontId::monospace(fonts::RULER),
                    Color32::from_gray(140),
                );
            }

            // Curve: one point per pixel, max over the bins each pixel spans,
            // linear interpolation where bins are sparser than pixels.
            let w = rect.width().max(1.0) as usize;
            let bin_hz = live.bin_hz(1).max(1e-6);
            let nb = live.bins();
            let sample = |arr: &[f32], hz0: f32, hz1: f32| -> f32 {
                let b0 = hz0 / bin_hz;
                let b1 = hz1 / bin_hz;
                if b1 - b0 >= 1.0 {
                    let i0 = (b0.floor() as usize).min(nb - 1);
                    let i1 = (b1.ceil() as usize).clamp(i0 + 1, nb);
                    arr[i0..i1].iter().copied().fold(f32::MIN, f32::max)
                } else {
                    let i = (b0.floor() as usize).min(nb - 2);
                    let t = b0 - i as f32;
                    arr[i] * (1.0 - t) + arr[i + 1] * t
                }
            };
            let mut curve = Vec::with_capacity(w);
            let mut peaks = Vec::with_capacity(w);
            for px in 0..w {
                let t0 = px as f32 / w as f32;
                let t1 = (px + 1) as f32 / w as f32;
                let hz0 = (lmin + t0 * (lmax - lmin)).exp();
                let hz1 = (lmin + t1 * (lmax - lmin)).exp();
                let x = rect.left() + px as f32 + 0.5;
                curve.push(pos2(x, y_of(sample(&live.db, hz0, hz1))));
                peaks.push(pos2(x, y_of(sample(&live.peak, hz0, hz1))));
            }
            // Fill under the curve.
            for pt in &curve {
                p.vline(
                    pt.x,
                    Rangef::new(pt.y, rect.bottom()),
                    Stroke::new(1.0, fill_c),
                );
            }
            p.add(egui::Shape::line(peaks, Stroke::new(1.0, peak_c)));
            p.add(egui::Shape::line(curve, Stroke::new(1.5, curve_c)));
        })
        .response
        .rect
}
