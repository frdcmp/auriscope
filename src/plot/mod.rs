//! Drawing onto an image, for the plots the CLI writes.
//!
//! The window draws with egui and a GPU. Nothing here does: a plot is a
//! [`ColorImage`] with rectangles, lines and text put into it by hand, which
//! is all an annotated spectrogram needs and is the difference between a
//! headless binary and one that wants a display.
//!
//! [`figure`] is the composition — margins, lanes, ticks, colour bar. This
//! module is the paint underneath it.

pub mod figure;
pub mod text;

use std::path::Path;

use anyhow::{Context, Result};
use egui::{Color32, ColorImage};

pub use figure::{Figure, Lane};
pub use text::Text;

/// Colours a plot is drawn in. Dark, because a spectrogram's colour maps all
/// start at black and a white surround makes the quiet end unreadable.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color32,
    pub panel: Color32,
    pub foreground: Color32,
    pub dim: Color32,
    pub grid: Color32,
    pub axis: Color32,
    pub wave: Color32,
    pub wave_rms: Color32,
    /// The spectrum lane's filled average trace, and the peak line over it.
    pub spectrum: Color32,
    pub spectrum_peak: Color32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color32::from_rgb(16, 16, 20),
            panel: Color32::from_rgb(8, 8, 10),
            foreground: Color32::from_rgb(220, 220, 225),
            dim: Color32::from_rgb(140, 140, 150),
            grid: Color32::from_rgb(70, 70, 80),
            axis: Color32::from_rgb(110, 110, 120),
            wave: Color32::from_rgb(90, 165, 235),
            wave_rms: Color32::from_rgb(150, 205, 255),
            spectrum: Color32::from_rgb(70, 120, 180),
            spectrum_peak: Color32::from_rgb(235, 170, 90),
        }
    }
}

/// An axis-aligned box in whole pixels, which is what every drawing operation
/// here works in. egui's `Rect` is in points and floats; a plot that is going
/// to a PNG wants neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Px {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Px {
    pub fn new(x: i64, y: i64, w: i64, h: i64) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i64 {
        self.y + self.h
    }
}

/// Fill a box, clipped to the image.
pub fn fill(img: &mut ColorImage, r: Px, color: Color32) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in r.y.max(0)..r.bottom().min(h) {
        for x in r.x.max(0)..r.right().min(w) {
            img.pixels[(y * w + x) as usize] = color;
        }
    }
}

/// Blend a colour over a box at `alpha`, leaving what is underneath showing
/// through. Gridlines are drawn this way: a spectrogram has to stay readable
/// through the line that says where a second is.
pub fn tint(img: &mut ColorImage, r: Px, color: Color32, alpha: u8) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for y in r.y.max(0)..r.bottom().min(h) {
        for x in r.x.max(0)..r.right().min(w) {
            let px = &mut img.pixels[(y * w + x) as usize];
            *px = over(*px, color, alpha);
        }
    }
}

/// `src` over `dst` at coverage `a`. These images are opaque throughout — they
/// are on the way to a PNG with no alpha channel — so this is a straight lerp
/// and never has to think about the destination's own alpha.
pub fn over(dst: Color32, src: Color32, a: u8) -> Color32 {
    let f = a as u16;
    let g = 255 - f;
    let mix = |d: u8, s: u8| (((d as u16 * g) + (s as u16 * f)) / 255) as u8;
    Color32::from_rgb(
        mix(dst.r(), src.r()),
        mix(dst.g(), src.g()),
        mix(dst.b(), src.b()),
    )
}

/// A one-pixel horizontal rule.
pub fn hline(img: &mut ColorImage, x: i64, y: i64, len: i64, color: Color32) {
    fill(img, Px::new(x, y, len, 1), color);
}

/// A one-pixel vertical rule.
pub fn vline(img: &mut ColorImage, x: i64, y: i64, len: i64, color: Color32) {
    fill(img, Px::new(x, y, 1, len), color);
}

/// Copy `src` into `dst` with its top-left corner at `x, y`, clipped.
pub fn blit(dst: &mut ColorImage, src: &ColorImage, x: i64, y: i64) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    for sy in 0..sh {
        let dy = y + sy;
        if dy < 0 || dy >= dh {
            continue;
        }
        for sx in 0..sw {
            let dx = x + sx;
            if dx < 0 || dx >= dw {
                continue;
            }
            dst.pixels[(dy * dw + dx) as usize] = src.pixels[(sy * sw + sx) as usize];
        }
    }
}

/// Write an image out as a PNG.
///
/// Three channels, not four: these images are opaque, so the alpha carries no
/// information, and a viewer that reads it differently cannot turn the picture
/// blank. egui's pixels are premultiplied, which at full alpha is the same
/// bytes anyway.
pub fn write_png(path: &Path, img: &ColorImage) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        img.width() as u32,
        img.height() as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut rgb = Vec::with_capacity(img.pixels.len() * 3);
    for px in &img.pixels {
        rgb.extend_from_slice(&[px.r(), px.g(), px.b()]);
    }
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&rgb))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// A round number near `span / target`: 1, 2 or 5 times a power of ten. What
/// keeps tick labels at values a reader recognises instead of wherever an
/// even division of the span happens to land.
///
/// Nearest rather than next one up, and nearest in the log of the value since
/// that is where these steps are evenly spaced. Rounding up would turn an
/// awkward span into half the ticks that were asked for: eighths of 86
/// seconds are 10.75 apart, which is a step of 10, not one of 20.
pub fn nice_step(span: f64, target: usize) -> f64 {
    if !span.is_finite() || span <= 0.0 || target == 0 {
        return 1.0;
    }
    let raw = span / target as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let n = raw / mag;
    // Midpoints in the log: sqrt(1*2), sqrt(2*5), sqrt(5*10).
    let mult = if n <= 1.414 {
        1.0
    } else if n <= 3.162 {
        2.0
    } else if n <= 7.071 {
        5.0
    } else {
        10.0
    };
    mult * mag
}

/// Tick positions on a linear axis, at round values inside `start..=end`.
pub fn linear_ticks(start: f64, end: f64, target: usize) -> Vec<f64> {
    let step = nice_step(end - start, target);
    let first = (start / step).ceil() * step;
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let v = first + step * i as f64;
        // A tick a whisker past the end is the same tick: the multiply above
        // cannot land exactly on a value the subtraction produced.
        if v > end + step * 1e-6 || out.len() > 1000 {
            break;
        }
        out.push(v);
        i += 1;
    }
    out
}

/// Tick positions on a logarithmic frequency axis: 1, 2 and 5 in every decade
/// the range covers, which is the series an ear reads frequencies in.
pub fn log_ticks(min_hz: f64, max_hz: f64) -> Vec<f64> {
    let mut out = Vec::new();
    if !min_hz.is_finite() || min_hz <= 0.0 || max_hz <= min_hz {
        return out;
    }
    let mut decade = 10f64.powf(min_hz.log10().floor());
    while decade <= max_hz {
        for m in [1.0, 2.0, 5.0] {
            let v = decade * m;
            if v >= min_hz && v <= max_hz {
                out.push(v);
            }
        }
        decade *= 10.0;
    }
    out
}

/// A time in seconds, at the precision `step` justifies: no more decimals than
/// tell two neighbouring ticks apart, and minutes once the numbers are long
/// enough that seconds alone stop being readable.
pub fn format_time(t: f64, step: f64, clock: bool) -> String {
    let decimals = (-step.log10().floor()).clamp(0.0, 3.0) as usize;
    if clock {
        let neg = t < 0.0;
        let a = t.abs();
        let m = (a / 60.0).floor();
        let s = a - m * 60.0;
        let sign = if neg { "-" } else { "" };
        // Two digits of seconds, plus the point and any decimals: "1:23" and
        // "1:23.5", never "1:023".
        let width = if decimals == 0 { 2 } else { decimals + 3 };
        format!("{sign}{m:.0}:{s:0>width$.decimals$}")
    } else {
        format!("{t:.decimals$}")
    }
}

/// A frequency, in the units it is usually said in: hertz below a kilohertz,
/// kilohertz above, and never more decimals than the value has.
pub fn format_hz(hz: f64) -> String {
    if hz == 0.0 {
        // The bottom of a linear axis: "0", not "0.0".
        "0".into()
    } else if hz >= 1000.0 {
        let k = hz / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else if hz >= 10.0 {
        format!("{hz:.0}")
    } else {
        format!("{hz:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_round_numbers() {
        assert_eq!(nice_step(10.0, 10), 1.0);
        assert_eq!(nice_step(1.0, 5), 0.2);
        assert_eq!(nice_step(3.7, 4), 1.0);
        // Not 20: eighths of this span are 10.75 apart, and a step of 10 is
        // the nearer round number as well as the one that keeps eight ticks.
        assert_eq!(nice_step(86.0, 8), 10.0);
        // Degenerate spans must not divide by zero or loop forever.
        assert_eq!(nice_step(0.0, 5), 1.0);
        assert_eq!(nice_step(1.0, 0), 1.0);
    }

    #[test]
    fn linear_ticks_land_inside_the_span() {
        let ticks = linear_ticks(0.0, 1.0, 5);
        assert_eq!(ticks, vec![0.0, 0.2, 0.4, 0.6000000000000001, 0.8, 1.0]);
        for t in linear_ticks(3.3, 7.1, 4) {
            assert!((3.3..=7.1).contains(&t));
        }
        assert!(linear_ticks(5.0, 5.0, 5).len() <= 1);
    }

    #[test]
    fn log_ticks_follow_the_decades() {
        assert_eq!(
            log_ticks(20.0, 20000.0).first().copied(),
            Some(20.0),
            "the first tick at or above the floor"
        );
        let ticks = log_ticks(100.0, 1000.0);
        assert_eq!(ticks, vec![100.0, 200.0, 500.0, 1000.0]);
        assert!(log_ticks(0.0, 1000.0).is_empty());
    }

    #[test]
    fn labels_say_only_what_the_step_justifies() {
        assert_eq!(format_time(1.5, 0.5, false), "1.5");
        assert_eq!(format_time(12.0, 5.0, false), "12");
        assert_eq!(format_time(0.15, 0.05, false), "0.15");
        assert_eq!(format_time(83.0, 10.0, true), "1:23");
        assert_eq!(format_time(83.5, 0.5, true), "1:23.5");
    }

    #[test]
    fn frequencies_are_said_the_usual_way() {
        assert_eq!(format_hz(0.0), "0");
        assert_eq!(format_hz(20.0), "20");
        assert_eq!(format_hz(1000.0), "1k");
        assert_eq!(format_hz(1500.0), "1.5k");
        assert_eq!(format_hz(22050.0), "22.1k");
    }

    #[test]
    fn drawing_is_clipped_to_the_image() {
        let mut img = ColorImage::filled([4, 4], Color32::BLACK);
        fill(&mut img, Px::new(-2, -2, 3, 3), Color32::WHITE);
        assert_eq!(img.pixels[0], Color32::WHITE);
        assert_eq!(img.pixels[1], Color32::BLACK);
        fill(&mut img, Px::new(3, 3, 100, 100), Color32::RED);
        assert_eq!(img.pixels[15], Color32::RED);

        let src = ColorImage::filled([2, 2], Color32::GREEN);
        blit(&mut img, &src, 3, 0);
        assert_eq!(img.pixels[3], Color32::GREEN);
        blit(&mut img, &src, -1, -1);
        assert_eq!(img.pixels[0], Color32::GREEN);
    }
}
