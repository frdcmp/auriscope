//! Colour maps for the spectrogram.

use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMap {
    Amber,
    Magma,
    Inferno,
    Viridis,
    Plasma,
    Turbo,
    Grey,
}

impl ColorMap {
    pub const ALL: [ColorMap; 7] = [
        ColorMap::Amber,
        ColorMap::Magma,
        ColorMap::Inferno,
        ColorMap::Viridis,
        ColorMap::Plasma,
        ColorMap::Turbo,
        ColorMap::Grey,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorMap::Amber => "Amber",
            ColorMap::Magma => "Magma",
            ColorMap::Inferno => "Inferno",
            ColorMap::Viridis => "Viridis",
            ColorMap::Plasma => "Plasma",
            ColorMap::Turbo => "Turbo",
            ColorMap::Grey => "Grey",
        }
    }

    /// 256-entry lookup table, index 0 = quietest.
    ///
    /// `contrast` is a gamma on the normalised level: 1.0 is linear in dB,
    /// above 1 pushes the quiet end towards the floor colour and stretches
    /// the loud end, below 1 lifts the quiet end.
    pub fn lut(self, contrast: f32) -> Vec<Color32> {
        let g = contrast.clamp(0.25, 4.0) as f64;
        (0..256)
            .map(|i| self.eval((i as f64 / 255.0).powf(g)))
            .collect()
    }

    fn eval(self, t: f64) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            ColorMap::Grey => {
                let v = (t * 255.0).round() as u8;
                Color32::from_rgb(v, v, v)
            }
            ColorMap::Amber => amber(t),
            _ => {
                let g = match self {
                    ColorMap::Magma => colorous::MAGMA,
                    ColorMap::Inferno => colorous::INFERNO,
                    ColorMap::Viridis => colorous::VIRIDIS,
                    ColorMap::Plasma => colorous::PLASMA,
                    ColorMap::Turbo => colorous::TURBO,
                    ColorMap::Amber | ColorMap::Grey => unreachable!(),
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

fn amber(t: f64) -> Color32 {
    let mut lo = AMBER_STOPS[0];
    for &hi in &AMBER_STOPS[1..] {
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
    let c = AMBER_STOPS[AMBER_STOPS.len() - 1].1;
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
    fn amber_runs_blue_then_orange() {
        let lut = ColorMap::Amber.lut(1.0);
        let quiet = lut[90];
        let loud = lut[190];
        assert!(quiet.b() > quiet.r(), "quiet end should be blue: {quiet:?}");
        assert!(loud.r() > loud.b(), "loud end should be orange: {loud:?}");
    }
}
