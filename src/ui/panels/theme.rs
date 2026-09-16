//! The colours the panels share.
//!
//! Every panel draws from the same small palette, so it lives in one place
//! rather than being restated per file. Colours used by a single card or
//! widget stay next to the code that draws them — only what crosses a module
//! boundary is here.

use eframe::egui;
use egui::Color32;

/// The side panel's own background, a shade below the cards it holds.
pub const SIDE_BG: Color32 = Color32::from_rgb(22, 23, 26);

/// The card a panel's contents sit on, and the accent that marks anything
/// interactive or worth the eye landing on.
pub const CARD_BG: Color32 = Color32::from_rgb(31, 32, 36);
pub const ACCENT: Color32 = Color32::from_rgb(86, 156, 214);

/// A label and its reading: the key recedes, the value carries.
pub const KEY: Color32 = Color32::from_gray(140);
pub const VAL: Color32 = Color32::from_gray(228);

/// The verdict colours, used wherever a reading can be fine, worth a look, or
/// wrong: level bars, the loudness card, the update check.
pub const GOOD: Color32 = Color32::from_rgb(120, 200, 130);
pub const WARN: Color32 = Color32::from_rgb(247, 198, 72);
pub const BAD: Color32 = Color32::from_rgb(255, 110, 110);
