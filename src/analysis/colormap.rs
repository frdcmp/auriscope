//! Colour maps for the spectrogram.

use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMap {
    Amber,
    Ember,
    Magma,
    Inferno,
    Viridis,
    Plasma,
    Turbo,
    Grey,
    /// Three user-chosen colours for the quiet, medium and loud ends, over
    /// black; see [`ColorMap::lut_with`].
    Custom,
}

/// The three colours of a custom palette: quiet, medium, loud.
pub type CustomStops = [[u8; 3]; 3];

/// Starting point for a custom palette: violet through orange to ivory, so
/// nothing in it competes with a blue waveform.
pub const DEFAULT_CUSTOM: CustomStops = [[60, 20, 90], [230, 110, 40], [255, 245, 220]];

impl ColorMap {
    pub const ALL: [ColorMap; 9] = [
        ColorMap::Amber,
        ColorMap::Ember,
        ColorMap::Magma,
        ColorMap::Inferno,
        ColorMap::Viridis,
        ColorMap::Plasma,
        ColorMap::Turbo,
        ColorMap::Grey,
        ColorMap::Custom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorMap::Amber => "Amber",
            ColorMap::Ember => "Ember",
            ColorMap::Magma => "Magma",
            ColorMap::Inferno => "Inferno",
            ColorMap::Viridis => "Viridis",
            ColorMap::Plasma => "Plasma",
            ColorMap::Turbo => "Turbo",
            ColorMap::Grey => "Grey",
            ColorMap::Custom => "Custom",
        }
    }

    /// 256-entry lookup table, index 0 = quietest. `Custom` uses the
    /// default stops; see [`ColorMap::lut_with`].
    pub fn lut(self, contrast: f32) -> Vec<Color32> {
        self.lut_with(contrast, &DEFAULT_CUSTOM)
    }

    /// As [`ColorMap::lut`], with the colours a `Custom` palette should use.
    /// `contrast` is a gamma on the level before it is coloured.
    pub fn lut_with(self, contrast: f32, custom: &CustomStops) -> Vec<Color32> {
        let g = contrast.clamp(0.25, 4.0) as f64;
        let custom_stops = [
            (0.0, [0, 0, 0]),
            (0.35, custom[0]),
            (0.70, custom[1]),
            (1.0, custom[2]),
        ];
        (0..256)
            .map(|i| self.eval((i as f64 / 255.0).powf(g), &custom_stops))
            .collect()
    }

    fn eval(self, t: f64, custom: &[(f64, [u8; 3])]) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            ColorMap::Grey => {
                let v = (t * 255.0).round() as u8;
                Color32::from_rgb(v, v, v)
            }
            ColorMap::Amber => gradient(&AMBER_STOPS, t),
            ColorMap::Ember => gradient(&EMBER_STOPS, t),
            ColorMap::Custom => gradient(custom, t),
            _ => {
                let g = match self {
                    ColorMap::Magma => colorous::MAGMA,
                    ColorMap::Inferno => colorous::INFERNO,
                    ColorMap::Viridis => colorous::VIRIDIS,
                    ColorMap::Plasma => colorous::PLASMA,
                    ColorMap::Turbo => colorous::TURBO,
                    ColorMap::Amber | ColorMap::Ember | ColorMap::Grey | ColorMap::Custom => {
                        unreachable!()
                    }
                };
                let c = g.eval_continuous(t);
                Color32::from_rgb(c.r, c.g, c.b)
            }
        }
    }
}

/// Black through deep navy and blue, across a muted bridge into orange,
/// amber and near-white: the palette that restoration-suite spectrograms are
/// known for. Quiet material sits in the blues, anything that matters glows.
const AMBER_STOPS: [(f64, [u8; 3]); 7] = [
    (0.00, [0, 0, 0]),
    (0.18, [8, 18, 56]),
    (0.38, [24, 70, 168]),
    (0.55, [96, 98, 152]),
    (0.70, [216, 112, 40]),
    (0.86, [247, 198, 72]),
    (1.00, [255, 255, 240]),
];

/// Black through wine and red into orange, gold and ivory: the Amber
/// palette's warmth without its blue half, for use over a blue waveform.
const EMBER_STOPS: [(f64, [u8; 3]); 5] = [
    (0.00, [0, 0, 0]),
    (0.25, [70, 10, 20]),
    (0.50, [190, 50, 30]),
    (0.75, [250, 160, 50]),
    (1.00, [255, 250, 225]),
];

/// Piecewise-linear interpolation through ordered `(position, colour)` stops.
fn gradient(stops: &[(f64, [u8; 3])], t: f64) -> Color32 {
    let mut lo = stops[0];
    for &hi in &stops[1..] {
        if t <= hi.0 {
            let span = (hi.0 - lo.0).max(1e-9);
            let u = ((t - lo.0) / span).clamp(0.0, 1.0);
            let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * u).round() as u8;
            return Color32::from_rgb(
                mix(lo.1[0], hi.1[0]),
                mix(lo.1[1], hi.1[1]),
                mix(lo.1[2], hi.1[2]),
            );
        }
        lo = hi;
    }
    let c = stops[stops.len() - 1].1;
    Color32::from_rgb(c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_map_is_monotonic_in_brightness_and_spans_the_range() {
        for m in ColorMap::ALL {
            let lut = m.lut(1.0);
            assert_eq!(lut.len(), 256);
            // Turbo is a rainbow, not a sequential map: both ends are dark
            // and its brightness is not monotonic. Only its length is checked.
            if m == ColorMap::Turbo {
                continue;
            }
            let lum =
                |c: Color32| 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
            // Loud end brighter than quiet end by a wide margin.
            assert!(lum(lut[255]) - lum(lut[0]) > 150.0, "{m:?}");
            // Not required to be strictly monotonic (turbo dips), but the
            // top quarter must be brighter than the bottom quarter.
            let bot: f32 = lut[..64].iter().map(|&c| lum(c)).sum::<f32>() / 64.0;
            let top: f32 = lut[192..].iter().map(|&c| lum(c)).sum::<f32>() / 64.0;
            assert!(top > bot, "{m:?}");
        }
    }

    #[test]
    fn contrast_gamma_moves_the_midpoint() {
        let flat = ColorMap::Grey.lut(1.0)[128].r();
        let hard = ColorMap::Grey.lut(2.0)[128].r();
        let soft = ColorMap::Grey.lut(0.5)[128].r();
        assert!(hard < flat && flat < soft, "{hard} {flat} {soft}");
    }

    #[test]
    fn custom_palette_follows_its_stops_and_ember_has_no_blue() {
        let stops: CustomStops = [[0, 200, 0], [200, 0, 0], [255, 255, 255]];
        let lut = ColorMap::Custom.lut_with(1.0, &stops);
        assert_eq!(lut[0], Color32::BLACK);
        let quiet = lut[(0.35 * 255.0) as usize];
        assert!(quiet.g() > 150 && quiet.r() < 40, "{quiet:?}");
        let mid = lut[(0.70 * 255.0) as usize];
        assert!(mid.r() > 150 && mid.g() < 40, "{mid:?}");
        assert_eq!(lut[255], Color32::WHITE);
        for c in ColorMap::Ember.lut(1.0) {
            assert!(
                c.b() <= c.r().max(c.g()),
                "ember should never lean blue: {c:?}"
            );
        }
    }

    #[test]
    fn amber_runs_blue_then_orange() {
        let lut = ColorMap::Amber.lut(1.0);
        let quiet = lut[90];
        let loud = lut[190];
        assert!(quiet.b() > quiet.r(), "quiet end should be blue: {quiet:?}");
        assert!(loud.r() > loud.b(), "loud end should be orange: {loud:?}");
    }
}
