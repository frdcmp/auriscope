//! Auriscope — audio player and analyser.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use std::path::PathBuf;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let arg = std::env::args_os().nth(1);
    // `--version` is what the installer reads to decide whether an update is
    // due, so it prints the bare number and nothing else. On Windows the
    // release build has no console attached; these still work when run from
    // one, and are harmless when double-clicked.
    match arg.as_deref().and_then(|a| a.to_str()) {
        Some("--version" | "-V") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!(
                "Auriscope {} — audio player and analyser\n\n\
                 Usage: auriscope [FILE]\n\n\
                 Options:\n  \
                 -V, --version  print the version and exit\n  \
                 -h, --help     print this help and exit\n\n\
                 Opening a file is also possible from the launcher, by drag and\n\
                 drop, or with Ctrl+O once the window is up.",
                env!("CARGO_PKG_VERSION")
            );
            return Ok(());
        }
        _ => {}
    }
    let initial: Option<PathBuf> = arg.map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Auriscope")
            .with_app_id("io.github.frdcmp.Auriscope")
            .with_icon(ui::icon::window_icon())
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0])
            // We draw our own title bar (GNOME-style window buttons), so no
            // server-side decorations.
            .with_decorations(false)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Auriscope",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, initial)))),
    )
}
