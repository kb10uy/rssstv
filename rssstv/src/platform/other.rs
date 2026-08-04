//! Integration for the platforms without a dedicated implementation.
//!
//! Linux is the platform this is written for; anything else the project has
//! not been tried on lands here too and gets the same neutral answers.

use egui::IconData;

pub const UI_FONTS: [&str; 3] = ["Noto Sans CJK JP", "Noto Sans", "DejaVu Sans"];

/// Only Linux has a file manager this can name. Another platform reaching
/// here reports that revealing a directory is unsupported rather than
/// spawning a command that does not exist.
pub const FILE_MANAGER: Option<&str> = if cfg!(target_os = "linux") {
    Some("xdg-open")
} else {
    None
};

pub fn prepare_process() {}

pub fn prepare_window(_cc: &eframe::CreationContext<'_>) {}

/// Keeping the machine awake needs the desktop's inhibit service, which is not
/// wired up yet, so activity is accepted and discarded.
pub type Host = super::InertPlatform;

pub type Claim = super::FileLock;

pub fn claim_single_instance() -> Option<Claim> {
    super::lock_file_claim()
}

pub fn window_icon() -> Option<IconData> {
    super::embedded_icon()
}
