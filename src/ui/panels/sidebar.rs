//! The side panel and the status bar: the two frames the sidebar's cards are
//! drawn into, and the placeholder shown before a file is open.

use eframe::egui;
use egui::{Color32, Margin, RichText};

use crate::ui::help::Topic;
use crate::ui::{App, fonts};

use super::file_cards::{bwf_card, file_card, header_card, markers_card, tags_card};
use super::meter_cards::{analysis_card, cursor_card, levels_card, loudness_card};
use super::theme::{KEY, SIDE_BG};
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
