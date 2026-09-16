//! The settings dialog: which panes to show, and every control behind the
//! three views.
//!
//! Laid out as a column of cards with the sidebar's widgets at the settings
//! dialog's roomier padding. Controls sit on a two-column grid so labels and
//! values line up down the whole dialog rather than per card.

use eframe::egui;
use egui::{Align, Color32, Layout, Margin, Rect, RichText, Sense, Stroke, pos2, vec2};

use auriscope::analysis::{ColorMap, StftParams, WindowKind};

use crate::ui::help::{self, Topic};
use crate::ui::util::{file_label, fmt_hz_unit, fmt_ms};
use crate::ui::views::{V_ZOOM_MAX, V_ZOOM_MIN};
use crate::ui::{App, RECENT_MAX, fonts, update};

use super::theme::{ACCENT, GOOD, KEY, SIDE_BG, VAL, WARN};
use super::widgets::{CARD_HEAD_BG, CARD_HEAD_RULE, mono, wide_card};

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

/// The wash under an accent button, and what hovering and pressing it brighten
/// that wash to. A wash rather than a solid fill: the button carries the card's
/// one action without shouting over the rows it sits among.
const ACT_BG: Color32 = Color32::from_rgb(38, 52, 66);
const ACT_BG_HOVER: Color32 = Color32::from_rgb(49, 69, 89);
const ACT_BG_ACTIVE: Color32 = Color32::from_rgb(60, 86, 112);

/// A button for the one action a card is about: accent text on an accent wash.
///
/// The states are set on the style rather than with `Button::fill`, which
/// overrides every state at once and would leave the button dead under the
/// pointer. Disabled is left alone: egui fades it toward the card, which reads
/// as unavailable already.
fn action_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    ui.scope(|ui| {
        let w = &mut ui.style_mut().visuals.widgets;
        w.inactive.weak_bg_fill = ACT_BG;
        w.inactive.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.5));
        w.hovered.weak_bg_fill = ACT_BG_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        w.active.weak_bg_fill = ACT_BG_ACTIVE;
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
        ui.add_enabled(
            enabled,
            egui::Button::new(RichText::new(text).color(ACCENT)),
        )
    })
    .inner
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
                        (crate::ui::DEFAULT_WAVE_COLOR, "Blue"),
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
        const KEYS: [(&str, &str); 20] = [
            ("Space", "play / pause"),
            ("Ctrl+O", "open a file"),
            ("Ctrl+,", "settings"),
            ("F1", "help mode: hover a label to read what it means"),
            (
                "Ctrl+Shift+S",
                "save the views as a PNG, with a JSON beside it",
            ),
            ("Click", "seek, clear the highlight"),
            ("Drag", "select; the range stays on the ruler"),
            ("Ruler handles", "drag to adjust the range"),
            ("Right-click", "clear highlight and range"),
            ("L", "loop the range"),
            ("Esc", "clear highlight, then range"),
            ("F / Shift+F", "zoom to range / fit file"),
            ("+ / −", "zoom in / out"),
            ("Ctrl+= / Ctrl+−", "interface scale · Ctrl+0 resets to 100%"),
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
            // The same label grid as the cards above, so the versions, the
            // setting and the check all start at the one column the rest of the
            // dialog uses.
            settings_grid(ui, "about-grid", |ui| {
                setting(ui, "Version", None, |ui| {
                    ui.label(mono(update::CURRENT));
                });
                if update::is_dev_build() {
                    setting(ui, "Build", None, |ui| {
                        ui.label(mono(update::GIT_DESCRIBE).color(ACCENT));
                    });
                }
                setting(ui, "Source", None, |ui| {
                    ui.hyperlink_to(
                        RichText::new("github.com/frdcmp/auriscope").monospace(),
                        update::REPO_URL,
                    );
                });
                if !update::ENABLED {
                    setting(ui, "Updates", None, |ui| {
                        ui.label(RichText::new("Your package manager handles them.").color(KEY));
                    });
                    return;
                }
                setting(ui, "Updates", None, |ui| {
                    ui.checkbox(&mut app.settings.check_updates, "Check on startup")
                        .on_hover_text(
                            "Asks api.github.com for the latest release, at most once a day. \
                             Nothing is downloaded and nothing about you is sent.",
                        );
                });
                // The button takes the label column and what the check said
                // takes the value column, so the two read as one row like the
                // rows above rather than as a block of their own.
                let checking = app.updater.status == update::Status::Checking;
                if action_button(ui, !checking, "Check now")
                    .on_hover_text("Asks GitHub for the latest release, now.")
                    .clicked()
                {
                    app.updater.start();
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CTRL_GAP;
                    match &app.updater.status {
                        update::Status::Idle => {
                            ui.label(RichText::new("Not checked yet.").color(KEY));
                        }
                        update::Status::Checking => {
                            ui.spinner();
                            ui.label(RichText::new("Checking…").color(KEY));
                        }
                        update::Status::UpToDate => {
                            ui.label(RichText::new("Up to date.").color(GOOD));
                        }
                        update::Status::Newer(r) => {
                            let r = r.clone();
                            ui.label(
                                RichText::new(format!("{} available.", r.version)).color(ACCENT),
                            );
                            ui.hyperlink_to(RichText::new("Release page"), &r.url);
                            let skipped =
                                app.settings.update_skipped.as_deref() == Some(r.version.as_str());
                            let label = if skipped { "Remind me" } else { "Skip" };
                            if ui.small_button(label).clicked() {
                                app.settings.update_skipped = (!skipped).then(|| r.version.clone());
                            }
                        }
                        update::Status::Failed(e) => {
                            ui.label(RichText::new(format!("Check failed: {e}")).color(KEY))
                                .on_hover_text(
                                    "Offline, or GitHub declined the request. Nothing else is \
                                     affected.",
                                );
                        }
                    }
                });
                ui.end_row();
            });
        },
    );
}
