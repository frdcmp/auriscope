//! Text on an image, without a window.
//!
//! egui's text stack is two halves: laying glyphs out and rasterising them
//! into an atlas is plain CPU work in `epaint`, and only turning that into
//! triangles needs a renderer. A plot drawn offscreen wants the first half and
//! not the second, so [`Text`] lays a string out, then copies the glyph's
//! pixels straight out of the atlas into the image.
//!
//! The atlas holds coverage as alpha over white, so a glyph is blended in at
//! whatever colour is asked for rather than being a fixed colour in the font.

use egui::epaint::text::{FontDefinitions, Fonts, TextOptions};
use egui::{Color32, ColorImage, FontId};

/// A laid-out-once font stack. Building it parses the embedded fonts, so it is
/// made once per plot and reused for every label on it.
pub struct Text {
    fonts: Fonts,
}

/// Points to pixels. Layout is done at 1:1 so a point is a pixel and the
/// caller's sizes mean what they say on the image.
const PPP: f32 = 1.0;

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl Text {
    pub fn new() -> Self {
        Self {
            fonts: Fonts::new(TextOptions::default(), FontDefinitions::default()),
        }
    }

    /// How much room `s` takes, in pixels.
    pub fn measure(&mut self, s: &str, font: &FontId) -> (f32, f32) {
        let galley = self.fonts.with_pixels_per_point(PPP).layout_no_wrap(
            s.to_owned(),
            font.clone(),
            Color32::WHITE,
        );
        (galley.rect.width(), galley.rect.height())
    }

    /// Draw `s` with its top-left corner at `x, y`.
    pub fn draw(
        &mut self,
        img: &mut ColorImage,
        x: f32,
        y: f32,
        s: &str,
        font: &FontId,
        color: Color32,
    ) {
        // Layout first and the atlas after: laying out may rasterise new
        // glyphs into the atlas, so the two borrows cannot overlap.
        let galley =
            self.fonts
                .with_pixels_per_point(PPP)
                .layout_no_wrap(s.to_owned(), font.clone(), color);
        let atlas = self.fonts.texture_atlas().image();
        let (aw, ah) = (atlas.width(), atlas.height());
        let (w, h) = (img.width(), img.height());

        for row in &galley.rows {
            for glyph in &row.row.glyphs {
                let uv = glyph.uv_rect;
                if uv.is_nothing() {
                    continue;
                }
                let gx = (x + row.pos.x + glyph.pos.x + uv.offset.x).round() as i64;
                let gy = (y + row.pos.y + glyph.pos.y + uv.offset.y).round() as i64;
                let gw = (uv.max[0] - uv.min[0]) as i64;
                let gh = (uv.max[1] - uv.min[1]) as i64;
                for ry in 0..gh {
                    let dy = gy + ry;
                    let sy = uv.min[1] as i64 + ry;
                    if dy < 0 || dy >= h as i64 || sy < 0 || sy >= ah as i64 {
                        continue;
                    }
                    for rx in 0..gw {
                        let dx = gx + rx;
                        let sx = uv.min[0] as i64 + rx;
                        if dx < 0 || dx >= w as i64 || sx < 0 || sx >= aw as i64 {
                            continue;
                        }
                        let cov = atlas.pixels[sy as usize * aw + sx as usize].a();
                        if cov == 0 {
                            continue;
                        }
                        let dst = &mut img.pixels[dy as usize * w + dx as usize];
                        *dst = super::over(*dst, color, cov);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string takes room, and a longer one takes more. Exact metrics belong
    /// to the font; what this holds to is that measuring works at all without
    /// a window, which is the whole point of the module.
    #[test]
    fn measuring_needs_no_window() {
        let mut text = Text::new();
        let font = FontId::monospace(12.0);
        let (w, h) = text.measure("1.5 s", &font);
        assert!(w > 0.0 && h > 0.0);
        let (wider, _) = text.measure("1.5 seconds", &font);
        assert!(wider > w);
    }

    /// Drawing puts pixels down inside the image and leaves the rest of it
    /// alone, including when the label runs off the edge.
    #[test]
    fn drawing_marks_the_image_and_stays_inside_it() {
        let mut text = Text::new();
        let font = FontId::monospace(12.0);
        let mut img = ColorImage::filled([64, 24], Color32::BLACK);
        text.draw(&mut img, 2.0, 2.0, "20k", &font, Color32::WHITE);
        assert!(img.pixels.iter().any(|p| *p != Color32::BLACK));

        // Off the edge in both directions: clipped, not a panic.
        let mut edge = ColorImage::filled([16, 16], Color32::BLACK);
        text.draw(&mut edge, -40.0, -40.0, "20k", &font, Color32::WHITE);
        text.draw(&mut edge, 100.0, 100.0, "20k", &font, Color32::WHITE);
    }
}
