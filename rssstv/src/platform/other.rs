//! Integration for the platforms without a dedicated implementation.
//!
//! Linux is the platform this is written for; anything else the project has
//! not been tried on lands here too and gets the same neutral answers.

use std::path::PathBuf;

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

/// Only Linux has distribution packages that install the manual; another
/// platform reaching here has no package and therefore no fallback.
pub fn manual_fallback() -> Option<PathBuf> {
    cfg!(target_os = "linux").then(|| PathBuf::from("/usr/share/doc/rssstv/help/index.html"))
}

/// Linux keeps its per-application directories lowercase, under a base
/// directory that is already hidden. The platforms that show these to the
/// operator name them the way the application is named.
pub const APP_DIRECTORY: &str = if cfg!(target_os = "linux") {
    "rssstv"
} else {
    "RSSSTV"
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
