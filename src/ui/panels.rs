//! The window's panels: the title bar and resize borders, the transport bar
//! across the top, the side panel of readouts, the status bar, and the
//! settings dialog.
//!
//! Each draws into its own frame and reads what it needs from `App`. The
//! shared parts live in three modules the rest build on: `theme` for the
//! palette, `widgets` for the card and key/value block, and `chrome` for the
//! window decorations we draw ourselves.

mod chrome;
mod file_cards;
mod meter_cards;
mod settings;
mod sidebar;
pub(super) mod theme;
mod transport;
mod widgets;

pub use chrome::{resize_borders, title_bar};
pub use settings::settings_window;
pub use sidebar::{side_panel, status_bar};
pub use transport::top_bar;
