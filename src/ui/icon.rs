//! The app icon, for the title bar and the window.
//!
//! Raw RGBA rasterised from `assets/io.github.frdcmp.Auriscope.svg`, so no
//! image decoder is needed at runtime. Regenerate after changing the SVG:
//!
//!     for s in 40 128; do
//!       rsvg-convert -w $s -h $s assets/io.github.frdcmp.Auriscope.svg -o /tmp/i.png
//!       python3 -c "from PIL import Image; open('assets/icon-$s.rgba','wb').write(
//!         Image.open('/tmp/i.png').convert('RGBA').tobytes())"
//!     done
//!
//! Two sizes because wgpu has no mipmaps here: 40 px draws 1:1 at 2× scale
//! and as a clean 2:1 box filter at 1×; 128 px is what window managers want.

use eframe::egui;

const SMALL: (&[u8], usize) = (include_bytes!("../../assets/icon-40.rgba"), 40);
const LARGE: (&[u8], usize) = (include_bytes!("../../assets/icon-128.rgba"), 128);

/// Size the title bar draws the icon at, in points.
pub const TITLE_SIZE: f32 = 20.0;

/// For `ViewportBuilder::with_icon`: taskbar, alt-tab, and the title bar of
/// window managers that draw one.
pub fn window_icon() -> egui::IconData {
    let (rgba, side) = LARGE;
    egui::IconData {
        rgba: rgba.to_vec(),
        width: side as u32,
        height: side as u32,
    }
}

/// The small icon as a texture for our own title bar.
pub fn title_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let (rgba, side) = SMALL;
    let image = egui::ColorImage::from_rgba_unmultiplied([side, side], rgba);
    ctx.load_texture("app-icon", image, egui::TextureOptions::LINEAR)
}
