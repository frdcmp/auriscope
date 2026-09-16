//! Perceptually uniform colour maps for the spectrogram.

use egui::Color32;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMap {
    Magma,
    Inferno,
    Viridis,
    Plasma,
    Turbo,
    Grey,
}

impl ColorMap {
    pub const ALL: [ColorMap; 6] = [
        ColorMap::Magma,
        ColorMap::Inferno,
        ColorMap::Viridis,
        ColorMap::Plasma,
        ColorMap::Turbo,
        ColorMap::Grey,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColorMap::Magma => "Magma",
            ColorMap::Inferno => "Inferno",
            ColorMap::Viridis => "Viridis",
            ColorMap::Plasma => "Plasma",
            ColorMap::Turbo => "Turbo",
            ColorMap::Grey => "Grey",
        }
    }

    /// 256-entry lookup table, index 0 = quietest.
    pub fn lut(self) -> Vec<Color32> {
        (0..256)
            .map(|i| {
                let t = i as f64 / 255.0;
                match self {
                    ColorMap::Grey => {
                        let v = (t * 255.0) as u8;
                        Color32::from_rgb(v, v, v)
                    }
                    _ => {
                        let g = match self {
                            ColorMap::Magma => colorous::MAGMA,
                            ColorMap::Inferno => colorous::INFERNO,
                            ColorMap::Viridis => colorous::VIRIDIS,
                            ColorMap::Plasma => colorous::PLASMA,
                            ColorMap::Turbo => colorous::TURBO,
                            ColorMap::Grey => unreachable!(),
                        };
                        let c = g.eval_continuous(t);
                        Color32::from_rgb(c.r, c.g, c.b)
                    }
                }
            })
            .collect()
    }
}
