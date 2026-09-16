//! Transport bar, side panel (file, loudness, view settings) and status bar.

use eframe::egui;
use egui::{Color32, RichText};

use auriscope::analysis::{ColorMap, StftParams, WindowKind, db_to_amp};

use super::App;
use super::util::fmt_time;
use super::views::channel_label;

pub fn top_bar(app: &mut App, root: &mut egui::Ui) {
    egui::Panel::top("top").show(root, |ui| {
        ui.horizontal(|ui| {
            if ui.button("Open…").on_hover_text("Ctrl+O").clicked() {
                app.pick_file();
            }
            ui.separator();

            let has = app.engine.is_some();
            let playing = app.engine.as_ref().is_some_and(|e| e.is_playing());
            let label = if playing { "⏸" } else { "▶" };
            if ui
                .add_enabled(has, egui::Button::new(RichText::new(label).size(18.0)))
                .on_hover_text("Space")
                .clicked()
            {
                app.toggle_play();
            }
            if ui
                .add_enabled(has, egui::Button::new(RichText::new("⏹").size(18.0)))
                .clicked()
                && let Some(e) = &app.engine
            {
                e.pause();
                let start = app.loop_region().map_or(0, |(a, _)| a);
                e.seek(start);
            }
            if ui
                .add_enabled(has, egui::Button::new("⏮"))
                .on_hover_text("Home")
                .clicked()
            {
                app.seek_frames(0.0);
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
    egui::Panel::right("side")
        .resizable(true)
        .default_size(270.0)
        .min_size(200.0)
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
             wheel pan · Ctrl+wheel / pinch zoom · +/- zoom",
            )
            .small(),
        );
    });
}
