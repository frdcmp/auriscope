//! Transport bar, side panel (file, loudness, view settings) and status bar.

use eframe::egui;
use egui::{Color32, Rect, RichText, Sense, Shape, pos2, vec2};

use auriscope::analysis::{ColorMap, StftParams, WindowKind, db_to_amp};

use super::App;
use super::util::fmt_time;
use super::views::channel_label;

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
                egui::FontId::proportional(12.5),
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
                    app.selection.is_some(),
                    egui::Checkbox::new(&mut loop_on, "Loop"),
                )
                .on_hover_text("L — loop the selection")
                .changed()
            {
                app.loop_enabled = loop_on;
                app.apply_loop();
            }
            if let Some((a, b)) = app.selection {
                let (a, b) = (a.min(b) / sr, a.max(b) / sr);
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
                ui.checkbox(&mut app.settings.follow_playhead, "Follow");
            });
        });
    });
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
    let max_w = (root.ctx().viewport_rect().width() * 0.45).max(220.0);
    egui::Panel::right("side")
        .resizable(true)
        .default_size(270.0)
        .min_size(200.0)
        .max_size(max_w)
        .show(root, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                file_section(app, ui);
                ui.separator();
                channels_section(app, ui);
                ui.separator();
                loudness_section(app, ui);
                ui.separator();
                view_section(app, ui);
            });
        });
}

fn file_section(app: &App, ui: &mut egui::Ui) {
    ui.heading("File");
    let Some(audio) = &app.audio else {
        ui.label(RichText::new("No file open").weak());
        return;
    };
    let info = &audio.info;
    ui.label(RichText::new(info.file_name()).strong());
    egui::Grid::new("fileinfo").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, k: &str, v: String| {
            ui.label(RichText::new(k).weak());
            ui.label(RichText::new(v).monospace());
            ui.end_row();
        };
        row(ui, "Container", info.container.clone());
        row(ui, "Codec", info.codec.clone());
        row(ui, "Rate", format!("{} Hz", info.sample_rate));
        row(ui, "Channels", info.channels.to_string());
        row(
            ui,
            "Depth",
            info.bits_per_sample
                .map_or("—".into(), |b| format!("{b} bit")),
        );
        row(ui, "Duration", fmt_time(info.duration_secs()));
        row(ui, "Frames", info.frames.to_string());
    });
    if !info.tags.is_empty() {
        ui.collapsing(format!("Tags ({})", info.tags.len()), |ui| {
            egui::Grid::new("tags").num_columns(2).show(ui, |ui| {
                for (k, v) in &info.tags {
                    ui.label(RichText::new(k).weak());
                    ui.add(egui::Label::new(RichText::new(v).monospace()).truncate());
                    ui.end_row();
                }
            });
        });
    }
}

fn channels_section(app: &mut App, ui: &mut egui::Ui) {
    let Some(audio) = app.audio.clone() else {
        return;
    };
    let nch = audio.channels.len();
    if nch < 2 {
        return;
    }
    ui.heading("Channels");
    let mut changed = false;
    for ch in 0..nch {
        ui.horizontal(|ui| {
            ui.label(RichText::new(channel_label(ch, nch)).monospace());
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
            if let Some(stats) = &app.stats
                && let Some(cs) = stats.channels.get(ch)
            {
                ui.label(
                    RichText::new(format!(
                        "pk {:.1}  tp {:.1}",
                        cs.sample_peak_db, cs.true_peak_dbtp
                    ))
                    .monospace()
                    .small()
                    .color(Color32::from_gray(160)),
                );
            }
        });
    }
    if changed {
        app.apply_mutes();
    }
}

fn loudness_section(app: &App, ui: &mut egui::Ui) {
    ui.heading("Measurements");
    let Some(stats) = &app.stats else {
        if app.audio.is_some() {
            ui.label(RichText::new("measuring…").weak());
        }
        return;
    };
    let lufs = |v: f32| {
        if v.is_finite() {
            format!("{v:.1} LUFS")
        } else {
            "—".into()
        }
    };
    egui::Grid::new("loud")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(RichText::new(k).weak());
                ui.label(RichText::new(v).monospace());
                ui.end_row();
            };
            row(ui, "Integrated", lufs(stats.integrated_lufs));
            row(ui, "Range", format!("{:.1} LU", stats.loudness_range_lu));
            row(ui, "Max momentary", lufs(stats.max_momentary_lufs));
            row(ui, "Max short-term", lufs(stats.max_short_term_lufs));
            if let Some(c) = stats.correlation {
                row(ui, "Correlation", format!("{c:+.2}"));
            }
        });
    let nch = stats.channels.len();
    egui::Grid::new("chstats")
        .num_columns(nch + 1)
        .striped(true)
        .show(ui, |ui| {
            ui.label("");
            for ch in 0..nch {
                ui.label(RichText::new(channel_label(ch, nch)).strong());
            }
            ui.end_row();
            type Fmt = fn(&auriscope::analysis::ChannelStats) -> String;
            let rows: [(&str, Fmt); 5] = [
                ("Peak", |c| format!("{:.1}", c.sample_peak_db)),
                ("True pk", |c| format!("{:.1}", c.true_peak_dbtp)),
                ("RMS", |c| format!("{:.1}", c.rms_db)),
                ("DC", |c| format!("{:+.4}", c.dc_offset)),
                ("Clipped", |c| {
                    if c.clipped_samples == 0 {
                        "0".into()
                    } else {
                        format!("{} ({} runs)", c.clipped_samples, c.clipped_runs)
                    }
                }),
            ];
            for (name, f) in &rows {
                ui.label(RichText::new(*name).weak());
                for c in &stats.channels {
                    let txt = f(c);
                    let clip = *name == "Clipped" && c.clipped_samples > 0;
                    let mut rt = RichText::new(txt).monospace();
                    if clip {
                        rt = rt.color(Color32::from_rgb(255, 110, 110));
                    }
                    ui.label(rt);
                }
                ui.end_row();
            }
        });
}

fn view_section(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Spectrogram");
    let mut stft = app.settings.stft;
    egui::ComboBox::from_label("Window size")
        .selected_text(stft.window_size.to_string())
        .show_ui(ui, |ui| {
            for s in StftParams::SIZES {
                ui.selectable_value(&mut stft.window_size, s, s.to_string());
            }
        });
    egui::ComboBox::from_label("Overlap")
        .selected_text(stft.overlap_label())
        .show_ui(ui, |ui| {
            for (n, d) in [(0u8, 1u8), (1, 2), (3, 4), (7, 8)] {
                let label = format!("{}%", 100 * n as u32 / d as u32);
                ui.selectable_value(&mut (stft.overlap_num, stft.overlap_den), (n, d), label);
            }
        });
    egui::ComboBox::from_label("Window")
        .selected_text(stft.window.name())
        .show_ui(ui, |ui| {
            for w in WindowKind::ALL {
                ui.selectable_value(&mut stft.window, w, w.name());
            }
        });
    if stft != app.settings.stft {
        app.settings.stft = stft;
        app.recompute_spectrograms();
    }

    egui::ComboBox::from_label("Colour map")
        .selected_text(app.settings.colormap.name())
        .show_ui(ui, |ui| {
            for c in ColorMap::ALL {
                ui.selectable_value(&mut app.settings.colormap, c, c.name());
            }
        });
    ui.checkbox(&mut app.settings.log_frequency, "Logarithmic frequency");
    ui.add(
        egui::Slider::new(&mut app.settings.db_min, -140.0..=-20.0)
            .text("Floor")
            .suffix(" dB"),
    );
    ui.add(
        egui::Slider::new(&mut app.settings.db_max, -60.0..=0.0)
            .text("Ceiling")
            .suffix(" dB"),
    );
    if app.settings.db_max <= app.settings.db_min + 6.0 {
        app.settings.db_max = app.settings.db_min + 6.0;
    }
    ui.add(
        egui::Slider::new(&mut app.settings.min_hz, 10.0..=200.0)
            .text("Min freq")
            .suffix(" Hz")
            .logarithmic(true),
    );

    ui.separator();
    ui.heading("Waveform");
    ui.checkbox(&mut app.settings.show_rms, "Show RMS");
    ui.add(egui::Slider::new(&mut app.settings.waveform_fraction, 0.15..=0.7).text("Height"));

    ui.separator();
    ui.heading("Spectrum");
    let mut size = app.settings.spectrum_size;
    egui::ComboBox::from_label("FFT size")
        .selected_text(size.to_string())
        .show_ui(ui, |ui| {
            for s in [1024usize, 2048, 4096, 8192, 16384] {
                ui.selectable_value(&mut size, s, s.to_string());
            }
        });
    if size != app.settings.spectrum_size {
        app.settings.spectrum_size = size;
        if let Some(e) = &app.engine {
            app.live = Some(auriscope::analysis::LiveSpectrum::new(size, e.device_rate));
        }
    }
    ui.add(egui::Slider::new(&mut app.settings.spectrum_averaging, 0.0..=0.95).text("Averaging"));

    ui.separator();
    ui.collapsing("Keys", |ui| {
        ui.label(
            RichText::new(
                "Space play/pause · click seek · drag select\n\
             ←/→ ±5 s (Shift: 1 s) · Home/End\n\
             L loop selection · Esc clear · F zoom selection (Shift+F: fit)\n\
             wheel zoom at pointer · shift+wheel pan\n\
             Alt+Shift+wheel waveform vertical zoom\n\
             Ctrl+wheel / pinch zoom · +/- zoom",
            )
            .small(),
        );
    });
}
