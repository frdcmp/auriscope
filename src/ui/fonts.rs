//! Bundled typeface and the app's type scale.
//!
//! JetBrains Mono Nerd Font, three faces: the proportional ("Propo") cut for
//! UI text, its bold for headings, and the monospace cut for numbers, rulers
//! and anything that has to line up. The Nerd Font patch adds the icon
//! glyphs used for section headers. SIL OFL 1.1; see `assets/fonts/OFL.txt`.
//!
//! Every text size in the app comes from the text styles set here, so
//! there is one scale rather than a size per call site.

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const PROPO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFontPropo-Regular.ttf");
const PROPO_BOLD: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFontPropo-Bold.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMonoNerdFontMono-Regular.ttf");

/// Family name for the bold face, for headings and emphasised values.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Nerd Font icon glyphs (Font Awesome range, stable across Nerd Fonts v3).
pub mod icon {
    pub const FILE_AUDIO: &str = "\u{f1c7}";
    pub const LIST: &str = "\u{f0ae}";
    pub const TAG: &str = "\u{f02b}";
    pub const GAUGE: &str = "\u{f0e4}";
    pub const BARS: &str = "\u{f080}";
    pub const COGS: &str = "\u{f085}";
    pub const CLOCK: &str = "\u{f017}";
    pub const PLAY: &str = "\u{f04b}";
    pub const MARKER: &str = "\u{f0c5}";
    pub const WARN: &str = "\u{f071}";
    pub const INFO: &str = "\u{f05a}";
    pub const DOWNLOAD: &str = "\u{f019}";
    pub const MAGNIFIER: &str = "\u{f002}";
}

/// Sizes of the type scale, in points.
pub const SMALL: f32 = 10.5;
pub const BODY: f32 = 12.5;
pub const MONO_SIZE: f32 = 12.0;
pub const HEADING: f32 = 14.0;
/// Rulers and the scales down the sides of the views: monospace, one step
/// under the small style.
pub const RULER: f32 = 10.0;

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("jb-propo".into(), Arc::new(FontData::from_static(PROPO)));
    fonts.font_data.insert(
        "jb-propo-bold".into(),
        Arc::new(FontData::from_static(PROPO_BOLD)),
    );
    fonts
        .font_data
        .insert("jb-mono".into(), Arc::new(FontData::from_static(MONO)));
    // First in each family so it is tried before egui's defaults, which
    // stay as fallbacks for glyphs the font lacks.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "jb-propo".into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jb-mono".into());
    fonts
        .families
        .insert(bold(), vec!["jb-propo-bold".into(), "jb-propo".into()]);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(SMALL, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(BODY, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(BODY, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(MONO_SIZE, FontFamily::Monospace),
            ),
            (TextStyle::Heading, FontId::new(HEADING, bold())),
        ]
        .into();
    });
}
