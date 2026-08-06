//! The colours the interface paints with, named once.
//!
//! Everything the interface draws that is not egui's own theme is here, so
//! that the palette can be read in one place rather than recovered from the
//! literals scattered through the widgets that use it.

use egui::Color32;

/// Behind the image view, darker than the panel around it.
pub const VIEWPORT: Color32 = Color32::from_rgb(10, 10, 13);

/// The part of the raster no rows have arrived for yet.
pub const PENDING: Color32 = Color32::from_rgb(23, 23, 26);

/// The line marking where the scan has reached.
pub const SCAN_LINE: Color32 = Color32::from_rgb(148, 196, 252);

/// Fills the transmit button, which is the one control that puts a signal out.
pub const TX_BUTTON: Color32 = Color32::from_rgb(140, 40, 40);

/// The receive indicator while a picture is arriving.
pub const RX_ACTIVE: Color32 = Color32::from_rgb(80, 200, 120);

/// The transmit meter while a picture is going out.
pub const TX_LEVEL: Color32 = Color32::from_rgb(200, 60, 60);

/// A failure reported in the status bar.
pub const ERROR: Color32 = Color32::from_rgb(220, 120, 120);

/// Text in a field holding something the application cannot accept.
pub const INVALID: Color32 = Color32::from_rgb(220, 96, 96);
