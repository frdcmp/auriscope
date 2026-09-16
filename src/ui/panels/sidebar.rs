//! The side panel and the status bar: the two frames the sidebar's cards are
//! drawn into, and the placeholder shown before a file is open.

use eframe::egui;
use egui::{Color32, Margin, RichText};

use crate::ui::help::{self, Topic};
use crate::ui::{App, fonts};

use super::file_cards::{bwf_card, file_card, header_card, markers_card, tags_card};
use super::meter_cards::{analysis_card, cursor_card, levels_card, loudness_card};
use super::theme::{ACCENT, KEY, SIDE_BG};
use super::widgets::card;

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
            // A capture landing, and anything else worth one line. Clears
            // itself after a few seconds, so the bar goes back to the device.
            if let Some(note) = app.notice() {
                ui.colored_label(ACCENT, note);
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
                ui.separator();
                interface_scale(ui);
            });
        });
    });
}

/// The interface-scale readout: what Ctrl+= and Ctrl+- have done to the size
/// of the whole UI, as a percentage.
///
/// egui owns the zoom itself and already persists it with the rest of its
/// memory, so this reads `zoom_factor` every frame rather than keeping a copy
/// that could drift out of step with the keyboard. Clicking resets to 100%,
/// the same as Ctrl+0.
fn interface_scale(ui: &mut egui::Ui) {
    let zoom = ui.ctx().zoom_factor();
    let default = (zoom - 1.0).abs() < 0.001;
    // Quiet at 100%, so the usual case costs the eye nothing; accented once
    // the scale is somewhere the user put it and might want to undo.
    let colour = if default { KEY } else { ACCENT };
    let resp = ui
        .add(
            egui::Label::new(
                RichText::new(format!("{}  {:.0}%", fonts::icon::MAGNIFIER, zoom * 100.0))
                    .small()
                    .color(colour),
            )
            .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    // Help mode has its own popover for this; two tooltips over one label
    // would fight for the same corner.
    let resp = if help::enabled(ui.ctx()) {
        resp
    } else {
        resp.on_hover_text("Interface scale — Ctrl+= / Ctrl+- to change, click to reset to 100%")
    };
    if resp.clicked() {
        ui.ctx().set_zoom_factor(1.0);
    }
    help::offer_response(ui, &resp, Topic::InterfaceScale);
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
                // No scrollbar: it ate width from an already narrow column and
                // the wheel scrolls the cards just the same.
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
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

fn empty_card(ui: &mut egui::Ui) {
    card(
        ui,
        fonts::icon::INFO,
        "No file",
        Topic::FileCard,
        None,
        |ui| {
            ui.label(
                RichText::new("Drop an audio file on the window, or press Ctrl+O.")
                    .small()
                    .color(KEY),
            );
        },
    );
}
