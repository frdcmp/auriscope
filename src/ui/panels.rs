//! Transport bar, side panel (file, loudness, view settings) and status bar.

use eframe::egui;
use egui::{
    Align, Align2, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Shape, Stroke, pos2,
    vec2,
};

use auriscope::analysis::{ColorMap, StftParams, WindowKind, db_to_amp};

use super::App;
use super::fonts;
use super::util::fmt_time;
use super::views::{V_ZOOM_MAX, V_ZOOM_MIN, channel_label};

const TITLEBAR_BG: Color32 = Color32::from_rgb(30, 30, 36);
const CLOSE_HOVER: Color32 = Color32::from_rgb(224, 27, 36);

enum WindowIcon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A GNOME-style window button: a grey circle on hover (red for close),
/// with a painted vector icon.
fn window_button(
    ui: &egui::Ui,
    rect: Rect,
    id: &str,
    icon: WindowIcon,
    tooltip: &str,
) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::new(id), Sense::click());
    let p = ui.painter_at(rect);
    let c = rect.center();
    let hover = resp.hovered();
    let fg = if hover {
        if matches!(icon, WindowIcon::Close) {
            p.circle_filled(c, 13.0, CLOSE_HOVER);
        } else {
            p.circle_filled(c, 13.0, Color32::from_gray(65));
        }
        Color32::WHITE
    } else {
        Color32::from_gray(185)
    };
    match icon {
        WindowIcon::Minimize => {
            p.rect_filled(Rect::from_center_size(c, vec2(10.0, 1.5)), 0.5, fg);
        }
        WindowIcon::Maximize => {
            p.rect_stroke(
                Rect::from_center_size(c, vec2(10.0, 10.0)),
                2.0,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
        }
        WindowIcon::Restore => {
            let back = Rect::from_center_size(c + vec2(2.5, 2.5), vec2(9.0, 9.0));
            let front = Rect::from_center_size(c - vec2(2.5, 2.5), vec2(9.0, 9.0));
            p.rect(
                back,
                2.0,
                Color32::TRANSPARENT,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
            p.rect(
                front,
                2.0,
                TITLEBAR_BG,
                egui::Stroke::new(1.5, fg),
                egui::StrokeKind::Middle,
            );
        }
        WindowIcon::Close => {
            let d = 4.5;
            p.line_segment([c - vec2(d, d), c + vec2(d, d)], egui::Stroke::new(1.6, fg));
            p.line_segment(
                [c - vec2(d, -d), c + vec2(d, -d)],
                egui::Stroke::new(1.6, fg),
            );
        }
    }
    resp.on_hover_text(tooltip)
}

/// Client-side title bar: window buttons, drag-to-move and
/// double-click-to-maximize, plus a north resize grip.
pub fn title_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("titlebar")
        .exact_size(34.0)
        .frame(egui::Frame::NONE.fill(TITLEBAR_BG))
        .show(root, |ui| {
            let ctx = ui.ctx().clone();
            let full = ui.available_rect_before_wrap();
            let bw = 34.0;
            let r_close =
                Rect::from_min_max(pos2(full.right() - bw, full.top()), full.right_bottom());
            let r_max = r_close.translate(vec2(-bw, 0.0));
            let r_min = r_max.translate(vec2(-bw, 0.0));

            let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            let close = window_button(ui, r_close, "win-close", WindowIcon::Close, "Close");
            if close.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let (max_icon, max_tip) = if maximized {
                (WindowIcon::Restore, "Restore")
            } else {
                (WindowIcon::Maximize, "Maximize")
            };
            let max = window_button(ui, r_max, "win-max", max_icon, max_tip);
            if max.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            let min = window_button(ui, r_min, "win-min", WindowIcon::Minimize, "Minimize");
            if min.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }

            // The rest of the bar moves the window; double-click maximizes.
            let drag_rect = Rect::from_min_max(full.min, pos2(r_min.left(), full.bottom()));
            let drag = ui.interact(
                drag_rect,
                egui::Id::new("titlebar-drag"),
                Sense::click_and_drag(),
            );
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            ui.painter().text(
                pos2(full.left() + 8.0, full.center().y),
                egui::Align2::LEFT_CENTER,
                &app.title,
                egui::FontId::proportional(fonts::BODY),
                Color32::from_gray(175),
            );
        });
}

/// Client-side window resize borders, drawn as a foreground overlay.
///
/// These deliberately do **not** live inside any panel. `allocate_*` inside a
/// panel takes space out of that panel's own layout, which is what collapses
/// it; `Ui::interact` on an overlay allocates nothing and disturbs nothing.
pub fn resize_borders(ctx: &egui::Context) {
    use egui::CursorIcon as Cur;
    use egui::viewport::ResizeDirection as Dir;

    // A maximized window is not resizable by its edges.
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }

    const EDGE: f32 = 6.0;
    const CORNER: f32 = 16.0;
    let r = ctx.viewport_rect();
    if r.width() < 4.0 * CORNER || r.height() < 4.0 * CORNER {
        return;
    }
    let (l, rt, t, b) = (r.left(), r.right(), r.top(), r.bottom());

    // Edges first, corners last: within a layer, the widget added later wins
    // the pointer, and a corner must beat the two edges it overlaps.
    let regions: [(&str, Rect, Dir, Cur); 8] = [
        (
            "rz-n",
            Rect::from_min_max(pos2(l, t), pos2(rt, t + EDGE)),
            Dir::North,
            Cur::ResizeNorth,
        ),
        (
            "rz-s",
            Rect::from_min_max(pos2(l, b - EDGE), pos2(rt, b)),
            Dir::South,
            Cur::ResizeSouth,
        ),
        (
            "rz-w",
            Rect::from_min_max(pos2(l, t), pos2(l + EDGE, b)),
            Dir::West,
            Cur::ResizeWest,
        ),
        (
            "rz-e",
            Rect::from_min_max(pos2(rt - EDGE, t), pos2(rt, b)),
            Dir::East,
            Cur::ResizeEast,
        ),
        (
            "rz-nw",
            Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)),
            Dir::NorthWest,
            Cur::ResizeNorthWest,
        ),
        (
            "rz-ne",
            Rect::from_min_max(pos2(rt - CORNER, t), pos2(rt, t + CORNER)),
            Dir::NorthEast,
            Cur::ResizeNorthEast,
        ),
        (
            "rz-sw",
            Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)),
            Dir::SouthWest,
            Cur::ResizeSouthWest,
        ),
        (
            "rz-se",
            Rect::from_min_max(pos2(rt - CORNER, b - CORNER), pos2(rt, b)),
            Dir::SouthEast,
            Cur::ResizeSouthEast,
        ),
    ];

    egui::Area::new(egui::Id::new("resize-borders"))
        .order(egui::Order::Foreground)
        .fixed_pos(r.min)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_clip_rect(r);
            for (id, rect, dir, cursor) in regions {
                let resp = ui.interact(rect, egui::Id::new(id), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(cursor);
                }
                if resp.drag_started() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                }
            }
        });
}

pub fn top_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("top").show(root, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Open…").on_hover_text("Ctrl+O").clicked() {
                app.pick_file();
            }
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
                RichText::new(format!("{} / {}", fmt_time(pos), fmt_time(total)))
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
            if let Some((a, b)) = app.range {
                let (a, b) = (a / sr, b / sr);
                ui.label(
                    RichText::new(format!(
                        "{} – {}  ({})",
                        fmt_time(a),
                        fmt_time(b),
                        fmt_time(b - a)
                    ))
                    .monospace()
                    .color(Color32::from_gray(170)),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let gain = egui::Slider::new(&mut app.settings.gain_db, -60.0..=12.0)
                    .suffix(" dB")
                    .text("Gain");
                if ui.add(gain).changed()
                    && let Some(e) = &app.engine
                {
                    e.shared.set_gain(db_to_amp(app.settings.gain_db));
                }
                let pan = egui::Slider::new(&mut app.settings.pan, -1.0..=1.0)
                    .text("Pan")
                    .fixed_decimals(2);
                if ui.add(pan).changed()
                    && let Some(e) = &app.engine
                {
                    e.shared.set_pan(app.settings.pan);
                }
                if settings_button(ui, app.settings_open)
                    .on_hover_text("Settings (Ctrl+,)")
                    .clicked()
                {
                    app.settings_open = !app.settings_open;
                }
                ui.separator();
                ui.checkbox(&mut app.settings.follow_playhead, "Follow");
            });
        });
    });
}

/// A sliders glyph for the settings button: three tracks with a knob each,
/// drawn as vectors so no fallback font decides how it looks.
fn settings_button(ui: &mut egui::Ui, active: bool) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(vec2(28.0, 26.0), Sense::click());
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
    let (resp, painter) = ui.allocate_painter(vec2(30.0, 26.0), Sense::click());
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

// ---- Sidebar styling -------------------------------------------------------

const SIDE_BG: Color32 = Color32::from_rgb(22, 23, 26);
const CARD_BG: Color32 = Color32::from_rgb(31, 32, 36);
const ACCENT: Color32 = Color32::from_rgb(86, 156, 214);
const KEY: Color32 = Color32::from_gray(140);
const VAL: Color32 = Color32::from_gray(228);
const GOOD: Color32 = Color32::from_rgb(120, 200, 130);
const WARN: Color32 = Color32::from_rgb(247, 198, 72);
const BAD: Color32 = Color32::from_rgb(255, 110, 110);

/// A titled card: icon, title, optional right-aligned note, then the body.
fn card(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    trailing: Option<String>,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(CARD_BG)
        .corner_radius(6.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).color(ACCENT).size(fonts::BODY));
                ui.label(
                    RichText::new(title)
                        .family(fonts::bold())
                        .size(fonts::BODY)
                        .color(Color32::from_gray(235)),
                );
                if let Some(t) = trailing {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(t).small().color(KEY));
                    });
                }
            });
            ui.add_space(2.0);
            body(ui);
        });
}

/// Key/value grid. Keys weak, values monospace, so numbers line up.
fn kv_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, rows);
}

fn kv(ui: &mut egui::Ui, key: &str, value: impl Into<String>) {
    kv_colored(ui, key, value, VAL);
}

fn kv_colored(ui: &mut egui::Ui, key: &str, value: impl Into<String>, color: Color32) {
    ui.label(RichText::new(key).small().color(KEY));
    ui.add(egui::Label::new(RichText::new(value.into()).monospace().color(color)).truncate());
    ui.end_row();
}

fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().color(VAL)
}

fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// UTC date and time without pulling in a calendar crate.
fn fmt_system_time(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Symphonia's long codec names, trimmed to fit a sidebar row.
fn short_codec(name: &str) -> String {
    name.replace("Little-Endian", "LE")
        .replace("Big-Endian", "BE")
        .replace(" Interleaved", "")
        .replace(" Planar", "")
        .replace("Signed ", "s")
        .replace("Unsigned ", "u")
        .replace("Floating Point ", "f")
        .replace("-bit", "")
}

fn db_str(v: f32) -> String {
    if v.is_finite() {
        format!("{v:.1} dB")
    } else {
        "—".into()
    }
}

fn lufs_str(v: f32) -> String {
    if v.is_finite() {
        format!("{v:.1} LUFS")
    } else {
        "—".into()
    }
}

/// Horizontal level bar on a −60…0 dB scale with a value readout.
fn level_bar(ui: &mut egui::Ui, label: &str, db: f32, color: Color32) {
    let h = 15.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let label_w = 34.0;
    let val_w = 52.0;
    let bar = Rect::from_min_max(
        pos2(rect.left() + label_w, rect.top() + 3.0),
        pos2(rect.right() - val_w, rect.bottom() - 3.0),
    );
    p.rect_filled(bar, 2.0, Color32::from_gray(44));
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
    p.text(
        pos2(rect.left(), rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::new(fonts::SMALL, egui::FontFamily::Proportional),
        KEY,
    );
    p.text(
        pos2(rect.right(), rect.center().y),
        Align2::RIGHT_CENTER,
        if db.is_finite() {
            format!("{db:.1}")
        } else {
            "—".into()
        },
        FontId::monospace(fonts::SMALL),
        VAL,
    );
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
    card(ui, fonts::icon::INFO, "No file", None, |ui| {
        ui.label(
            RichText::new("Drop an audio file on the window, or press Ctrl+O.")
                .small()
                .color(KEY),
        );
    });
}

fn file_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let info = &audio.info;
    let ext = info
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase());
    card(ui, fonts::icon::FILE_AUDIO, "File", ext, |ui| {
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
        ui.add(egui::Label::new(RichText::new(info.directory()).small().color(KEY)).truncate());
        ui.add_space(2.0);
        let layout = match info.channels {
            1 => "mono".to_string(),
            2 => "stereo".into(),
            n => format!("{n} ch"),
        };
        kv_grid(ui, "file-kv", |ui| {
            kv(ui, "Container", &info.container);
            ui.label(RichText::new("Codec").small().color(KEY));
            ui.add(egui::Label::new(mono(short_codec(&info.codec))).truncate())
                .on_hover_text(&info.codec);
            ui.end_row();
            kv(
                ui,
                "Sample rate",
                format!("{} Hz", fmt_int(info.sample_rate as u64)),
            );
            kv(ui, "Channels", format!("{} ({layout})", info.channels));
            kv(
                ui,
                "Bit depth",
                info.bits_per_sample
                    .map_or("—".into(), |b| format!("{b} bit")),
            );
            kv(ui, "Duration", fmt_time(info.duration_secs()));
            kv(ui, "Frames", fmt_int(info.frames as u64));
            kv(ui, "Size", fmt_bytes(info.file_size));
            if let Some(kbps) = info.bitrate_kbps() {
                kv(ui, "Bit rate", format!("{kbps:.0} kb/s"));
            }
            kv(
                ui,
                "In memory",
                fmt_bytes((info.frames * info.channels * 4) as u64),
            );
            if let Some(m) = info.modified {
                kv(ui, "Modified", fmt_system_time(m));
            }
        });
    });
}

fn header_card(app: &App, ui: &mut egui::Ui) {
    let Some(audio) = &app.audio else { return };
    let Some(wav) = &audio.info.wav else { return };
    let kind = if wav.rf64 { "RF64" } else { "RIFF" };
    card(
        ui,
        fonts::icon::LIST,
        "WAVE header",
        Some(format!("{kind} · {} chunks", wav.chunks.len())),
        |ui| {
            if let Some(f) = &wav.fmt {
                kv_grid(ui, "fmt-kv", |ui| {
                    kv(
                        ui,
                        "Format",
                        format!("{} (0x{:04X})", f.format_name(), f.format_tag),
                    );
                    kv(ui, "Block align", format!("{} B", f.block_align));
                    kv(
                        ui,
                        "Byte rate",
                        format!("{} kB/s", f.byte_rate as f64 / 1000.0),
                    );
                    if let Some(v) = f.valid_bits {
                        kv(ui, "Valid bits", format!("{v} of {}", f.bits_per_sample));
                    }
                    if let Some(m) = f.channel_mask {
                        kv(ui, "Channel mask", format!("0x{m:08X}"));
                    }
                    kv(ui, "Body", fmt_bytes(wav.riff_size));
                });
                ui.add_space(4.0);
            }
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
        Some(format!("bext v{}", b.version)),
        |ui| {
            if !b.description.is_empty() {
                ui.add(egui::Label::new(RichText::new(&b.description).color(VAL)).wrap());
                ui.add_space(2.0);
            }
            kv_grid(ui, "bext-kv", |ui| {
                if !b.originator.is_empty() {
                    kv(ui, "Originator", &b.originator);
                }
                if !b.originator_reference.is_empty() {
                    kv(ui, "Reference", &b.originator_reference);
                }
                if !b.origination_date.is_empty() || !b.origination_time.is_empty() {
                    kv(
                        ui,
                        "Originated",
                        format!("{} {}", b.origination_date, b.origination_time),
                    );
                }
                let tr = b.time_reference as f64 / sr;
                kv(
                    ui,
                    "Time ref",
                    format!("{} ({} smp)", fmt_time(tr), fmt_int(b.time_reference)),
                );
                if let Some(u) = &b.umid {
                    kv(ui, "UMID", format!("{}…", &u[..u.len().min(16)]));
                }
                if let Some(l) = b.loudness {
                    kv(ui, "Integrated", lufs_str(l.integrated_lufs));
                    kv(ui, "Range", format!("{:.1} LU", l.range_lu));
                    kv(
                        ui,
                        "Max true pk",
                        format!("{:.1} dBTP", l.max_true_peak_dbtp),
                    );
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
    card(ui, fonts::icon::TAG, "Tags", Some(n.to_string()), |ui| {
        kv_grid(ui, "tags-kv", |ui| {
            for (k, v) in riff_info.iter().chain(&info.tags) {
                kv(ui, k, v);
            }
        });
    });
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
    card(ui, fonts::icon::GAUGE, "Loudness", trailing, |ui| {
        let Some(stats) = &app.stats else {
            return;
        };
        kv_grid(ui, "loud-kv", |ui| {
            kv(ui, "Integrated", lufs_str(stats.integrated_lufs));
            kv(ui, "Range", format!("{:.1} LU", stats.loudness_range_lu));
            kv(ui, "Max momentary", lufs_str(stats.max_momentary_lufs));
            kv(ui, "Max short-term", lufs_str(stats.max_short_term_lufs));
            if let Some(c) = stats.correlation {
                kv(ui, "Correlation", format!("{c:+.2}"));
            }
        });
        if !stats.integrated_lufs.is_finite() {
            return;
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Against targets").small().color(KEY));
        kv_grid(ui, "targets-kv", |ui| {
            for (name, target) in [
                ("EBU R128 · −23", -23.0f32),
                ("Podcast · −16", -16.0),
                ("Streaming · −14", -14.0),
            ] {
                let d = stats.integrated_lufs - target;
                let color = if d.abs() <= 1.0 {
                    GOOD
                } else if d.abs() <= 3.0 {
                    WARN
                } else {
                    VAL
                };
                kv_colored(ui, name, format!("{d:+.1} LU"), color);
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
            kv_colored(ui, "Headroom", db_str(-tp), peak_color(tp));
            kv(ui, "Crest factor", db_str(pk - rms));
        });
    });
}

fn levels_card(app: &mut App, ui: &mut egui::Ui) {
    let Some(audio) = app.audio.clone() else {
        return;
    };
    let nch = audio.channels.len();
    let mut changed = false;
    card(ui, fonts::icon::BARS, "Levels", None, |ui| {
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
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(channel_label(ch, nch))
                        .family(fonts::bold())
                        .size(fonts::SMALL)
                        .color(VAL),
                );
                if nch > 1 {
                    let mut m = app.mutes[ch];
                    if ui.toggle_value(&mut m, "M").on_hover_text("Mute").changed() {
                        app.mutes[ch] = m;
                        changed = true;
                    }
                    let mut s = app.solo == Some(ch);
                    if ui.toggle_value(&mut s, "S").on_hover_text("Solo").changed() {
                        app.solo = if s { Some(ch) } else { None };
                        changed = true;
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if cs.clipped_samples > 0 {
                        ui.label(
                            RichText::new(format!(
                                "{} {} clipped · {} runs",
                                fonts::icon::WARN,
                                fmt_int(cs.clipped_samples as u64),
                                cs.clipped_runs
                            ))
                            .small()
                            .color(BAD),
                        );
                    } else {
                        ui.label(RichText::new("no clipping").small().color(KEY));
                    }
                });
            });
            level_bar(ui, "Peak", cs.sample_peak_db, peak_color(cs.true_peak_dbtp));
            level_bar(ui, "TP", cs.true_peak_dbtp, peak_color(cs.true_peak_dbtp));
            level_bar(ui, "RMS", cs.rms_db, Color32::from_rgb(96, 118, 172));
            ui.label(
                RichText::new(format!("DC offset {:+.4}", cs.dc_offset))
                    .small()
                    .color(if cs.dc_offset.abs() > 0.01 { WARN } else { KEY }),
            );
        }
    });
    if changed {
        app.apply_mutes();
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
        Some(format!("{:?}", app.settings.colormap)),
        |ui| {
            kv_grid(ui, "an-kv", |ui| {
                kv(
                    ui,
                    "Window",
                    format!(
                        "{} {:?} · {}",
                        st.window_size,
                        st.window,
                        st.overlap_label()
                    ),
                );
                kv(
                    ui,
                    "Hop",
                    format!("{hop} smp · {:.1} ms", hop as f64 / sr * 1000.0),
                );
                kv(
                    ui,
                    "Resolution",
                    format!(
                        "{:.1} Hz · {:.1} ms",
                        sr / st.window_size as f64,
                        st.window_size as f64 / sr * 1000.0
                    ),
                );
                kv(ui, "Reassignment", if st.reassign { "on" } else { "off" });
                let tile = app.detail.iter().flatten().next();
                kv(
                    ui,
                    "Detail tile",
                    tile.map_or("—".into(), |t| {
                        format!("{} cols · {:.2} smp", fmt_int(t.columns as u64), t.stride)
                    }),
                );
                let span = app.view.len() / sr;
                kv(
                    ui,
                    "View",
                    format!(
                        "{} – {}",
                        fmt_time(app.view.start / sr),
                        fmt_time(app.view.end / sr)
                    ),
                );
                kv(ui, "Span", format!("{span:.3} s"));
                kv(
                    ui,
                    "Floor / ceiling",
                    format!("{:.0} / {:.0} dB", app.settings.db_min, app.settings.db_max),
                );
            });
        },
    );
}

fn cursor_card(app: &App, ui: &mut egui::Ui) {
    let sr = app.sample_rate().max(1.0);
    card(ui, fonts::icon::CLOCK, "Cursor", None, |ui| {
        kv_grid(ui, "cur-kv", |ui| {
            let ph = app
                .engine
                .as_ref()
                .map_or(0.0, |e| e.playhead() as f64 / sr);
            kv(ui, "Playhead", fmt_time(ph));
            kv(
                ui,
                "Pointer",
                if app.hover_info.is_empty() {
                    "—".to_string()
                } else {
                    app.hover_info.clone()
                },
            );
            match app.selection {
                Some((a, b)) if (a - b).abs() >= 1.0 => {
                    let (a, b) = (a.min(b) / sr, a.max(b) / sr);
                    kv(
                        ui,
                        "Selection",
                        format!("{} – {} ({:.3} s)", fmt_time(a), fmt_time(b), b - a),
                    );
                }
                _ => kv(ui, "Selection", "—"),
            }
            match app.range {
                Some((a, b)) => {
                    let (a, b) = (a / sr, b / sr);
                    kv(
                        ui,
                        "Range",
                        format!("{} – {} ({:.3} s)", fmt_time(a), fmt_time(b), b - a),
                    );
                    kv_colored(
                        ui,
                        "Loop",
                        if app.loop_enabled { "on" } else { "off" },
                        if app.loop_enabled { GOOD } else { VAL },
                    );
                }
                None => kv(ui, "Range", "—"),
            }
        });
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
                .inner_margin(Margin::same(10)),
        )
        .show(ctx, |ui| {
            ui.set_width(480.0);
            ui.spacing_mut().item_spacing.y = 8.0;
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
                    if ui.button("Close").clicked() {
                        app.settings_open = false;
                    }
                    ui.label(RichText::new("Ctrl+,  ·  Esc").small().color(KEY));
                });
            });
            let max_h = (ctx.viewport_rect().height() * 0.85 - 80.0).max(300.0);
            let want_h = app.settings_content_h.min(max_h);
            let mut area = egui::ScrollArea::vertical()
                .max_height(max_h)
                .min_scrolled_height(want_h)
                .auto_shrink([false, true]);
            if app.settings_content_h == 0.0 {
                // Freshly opened: start at the top whatever egui remembers.
                area = area.vertical_scroll_offset(0.0);
            }
            let out = area.show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                panes_card(app, ui);
                spectrogram_card(app, ui);
                waveform_card(app, ui);
                spectrum_card(app, ui);
                keys_card(ui);
            });
            app.settings_content_h = out.content_size.y;
        });
    if modal.should_close() {
        app.settings_open = false;
    }
}

/// Width of the label column in every settings grid, so controls line up
/// across cards.
const LABEL_W: f32 = 156.0;
const GRID_GAP: f32 = 12.0;
/// Width of a slider's value box plus its gap, reserved so the slider track
/// ends at the same x in every row.
const VALUE_W: f32 = 92.0;
const COMBO_W: f32 = 150.0;

/// One labelled control row inside a settings grid.
fn setting(ui: &mut egui::Ui, label: &str, hint: &str, control: impl FnOnce(&mut egui::Ui)) {
    let l = ui.add(egui::Label::new(RichText::new(label).color(KEY)).truncate());
    if !hint.is_empty() {
        l.on_hover_text(hint);
    }
    ui.horizontal(|ui| control(ui));
    ui.end_row();
}

fn settings_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    let slider_w = (ui.available_width() - LABEL_W - GRID_GAP - VALUE_W).max(120.0);
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(LABEL_W)
        .spacing([GRID_GAP, 6.0])
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
    card(ui, fonts::icon::LIST, "Panes", None, |ui| {
        let both = app.settings.show_waveform && app.settings.show_spectrogram;
        let merged = both && app.settings.merge_views;
        settings_grid(ui, "panes-grid", |ui| {
            setting(ui, "Show", "", |ui| {
                ui.checkbox(&mut app.settings.show_waveform, "Waveform");
                ui.checkbox(&mut app.settings.show_spectrogram, "Spectrogram");
                ui.checkbox(&mut app.settings.show_spectrum, "Spectrum");
            });
            setting(
                ui,
                "Merge",
                "Draw the waveform over the spectrogram in one pane.",
                |ui| {
                    ui.add_enabled_ui(both, |ui| {
                        ui.checkbox(&mut app.settings.merge_views, "Waveform over spectrogram");
                    });
                },
            );
            // Enabled-state scopes must sit inside the control cell: a scope
            // around the whole row would swallow the grid's end_row.
            setting(ui, "Waveform opacity", "", |ui| {
                ui.add_enabled_ui(merged, |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.merge_opacity, 0.05..=1.0)
                            .fixed_decimals(2),
                    );
                });
            });
            setting(ui, "Spectrogram opacity", "", |ui| {
                ui.add_enabled_ui(merged, |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.merge_spec_opacity, 0.05..=1.0)
                            .fixed_decimals(2),
                    );
                });
            });
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
    });
}

fn spectrogram_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, fonts::icon::BARS, "Spectrogram", None, |ui| {
        let mut stft = app.settings.stft;
        settings_grid(ui, "stft-grid", |ui| {
            setting(
                ui,
                "Window size",
                "Samples per FFT. Larger: finer frequency, coarser time.",
                |ui| {
                    combo("win-size")
                        .selected_text(stft.window_size.to_string())
                        .show_ui(ui, |ui| {
                            for s in StftParams::SIZES {
                                ui.selectable_value(&mut stft.window_size, s, s.to_string());
                            }
                        });
                    let sr = app.sample_rate().max(1.0);
                    ui.label(
                        RichText::new(format!(
                            "{:.1} Hz · {:.1} ms",
                            sr / stft.window_size as f64,
                            stft.window_size as f64 / sr * 1000.0
                        ))
                        .color(KEY),
                    );
                },
            );
            setting(ui, "Overlap", "", |ui| {
                combo("overlap")
                    .selected_text(stft.overlap_label())
                    .show_ui(ui, |ui| {
                        for (n, d) in [(0u8, 1u8), (1, 2), (3, 4), (7, 8)] {
                            let label = format!("{}%", 100 * n as u32 / d as u32);
                            ui.selectable_value(
                                &mut (stft.overlap_num, stft.overlap_den),
                                (n, d),
                                label,
                            );
                        }
                    });
            });
            setting(ui, "Window", "", |ui| {
                combo("win-kind")
                    .selected_text(stft.window.name())
                    .show_ui(ui, |ui| {
                        for w in WindowKind::ALL {
                            ui.selectable_value(&mut stft.window, w, w.name());
                        }
                    });
            });
            setting(
                ui,
                "Reassignment",
                "Moves each bin's energy to its true frequency and time instead of \
                 smearing it across the window. Three FFTs per frame instead of one.",
                |ui| {
                    ui.checkbox(&mut stft.reassign, "Sharper lines and clicks");
                },
            );
        });
        if stft != app.settings.stft {
            app.settings.stft = stft;
            app.recompute_spectrograms();
        }
        ui.add_space(4.0);
        settings_grid(ui, "spec-grid", |ui| {
            setting(ui, "Colour map", "", |ui| {
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
                setting(
                    ui,
                    "Custom colours",
                    "Quiet, medium and loud. Black below the quiet colour.",
                    |ui| {
                        for (i, label) in ["quiet", "medium", "loud"].iter().enumerate() {
                            ui.color_edit_button_srgb(&mut app.settings.custom_stops[i])
                                .on_hover_text(*label);
                        }
                        if ui.button("Reset").clicked() {
                            app.settings.custom_stops = auriscope::analysis::DEFAULT_CUSTOM;
                        }
                    },
                );
            }
            setting(
                ui,
                "Contrast",
                "Gamma on the colour map. Above 1 darkens quiet material, below 1 lifts it.",
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.spec_contrast, 0.4..=2.5)
                            .fixed_decimals(2)
                            .logarithmic(true),
                    );
                },
            );
            setting(ui, "Frequency axis", "", |ui| {
                ui.checkbox(&mut app.settings.log_frequency, "Logarithmic");
            });
            setting(ui, "Floor", "Level drawn as the darkest colour.", |ui| {
                ui.add(
                    egui::Slider::new(&mut app.settings.db_min, -140.0..=-20.0)
                        .fixed_decimals(0)
                        .suffix(" dB"),
                );
            });
            setting(
                ui,
                "Ceiling",
                "Level drawn as the brightest colour.",
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.db_max, -60.0..=0.0)
                            .fixed_decimals(0)
                            .suffix(" dB"),
                    );
                },
            );
            if app.settings.db_max <= app.settings.db_min + 6.0 {
                app.settings.db_max = app.settings.db_min + 6.0;
            }
            setting(
                ui,
                "Lowest frequency",
                "Bottom of the logarithmic axis.",
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.min_hz, 10.0..=200.0)
                            .fixed_decimals(0)
                            .suffix(" Hz")
                            .logarithmic(true),
                    );
                },
            );
        });
    });
}

fn waveform_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, fonts::icon::GAUGE, "Waveform", None, |ui| {
        let merged =
            app.settings.merge_views && app.settings.show_waveform && app.settings.show_spectrogram;
        settings_grid(ui, "wave-grid", |ui| {
            setting(ui, "Overlays", "", |ui| {
                ui.checkbox(&mut app.settings.show_rms, "RMS");
                ui.checkbox(&mut app.settings.show_db_scale, "dB scale");
            });
            setting(
                ui,
                "Colour",
                "Waveform colour. The RMS overlay is a lighter tint of it.",
                |ui| {
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
                },
            );
            setting(
                ui,
                "Vertical zoom",
                "Alt+Shift+wheel over the waveform does the same.",
                |ui| {
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
                },
            );
            setting(
                ui,
                "Height",
                "Share of the pane given to the waveform. Not used while merged.",
                |ui| {
                    ui.add_enabled_ui(!merged, |ui| {
                        ui.add(
                            egui::Slider::new(&mut app.settings.waveform_fraction, 0.05..=0.95)
                                .fixed_decimals(2),
                        );
                    });
                },
            );
        });
    });
}

fn spectrum_card(app: &mut App, ui: &mut egui::Ui) {
    card(ui, fonts::icon::BARS, "Spectrum", None, |ui| {
        let mut size = app.settings.spectrum_size;
        settings_grid(ui, "spectrum-grid", |ui| {
            setting(ui, "FFT size", "", |ui| {
                combo("spectrum-size")
                    .selected_text(size.to_string())
                    .show_ui(ui, |ui| {
                        for s in [1024usize, 2048, 4096, 8192, 16384] {
                            ui.selectable_value(&mut size, s, s.to_string());
                        }
                    });
            });
            setting(
                ui,
                "Averaging",
                "How much of the previous frame is kept.",
                |ui| {
                    ui.add(
                        egui::Slider::new(&mut app.settings.spectrum_averaging, 0.0..=0.95)
                            .fixed_decimals(2),
                    );
                },
            );
        });
        if size != app.settings.spectrum_size {
            app.settings.spectrum_size = size;
            if let Some(e) = &app.engine {
                app.live = Some(auriscope::analysis::LiveSpectrum::new(size, e.device_rate));
            }
        }
    });
}

fn keys_card(ui: &mut egui::Ui) {
    card(ui, fonts::icon::INFO, "Keys", None, |ui| {
        const KEYS: [(&str, &str); 16] = [
            ("Space", "play / pause"),
            ("Ctrl+O", "open a file"),
            ("Ctrl+,", "settings"),
            ("Click", "seek, clear the highlight"),
            ("Drag", "select; the range stays on the ruler"),
            ("Ruler handles", "drag to adjust the range"),
            ("Right-click", "clear highlight and range"),
            ("L", "loop the range"),
            ("Esc", "clear highlight, then range"),
            ("F / Shift+F", "zoom to range / fit file"),
            ("← → / Shift", "±5 s / ±1 s · Home, End"),
            ("Wheel", "zoom at pointer · Shift: pan"),
            ("Ctrl+wheel, pinch", "zoom · + / −"),
            ("Alt+Shift+wheel", "waveform vertical zoom"),
            ("Middle-drag", "scroll the clip"),
            ("Divider", "drag to resize the strips"),
        ];
        let desc_w = (ui.available_width() - LABEL_W - GRID_GAP).max(120.0);
        egui::Grid::new("keys-grid")
            .num_columns(2)
            .min_col_width(LABEL_W)
            .spacing([GRID_GAP, 4.0])
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
