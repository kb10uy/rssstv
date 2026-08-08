//! macOS integration.

use std::path::PathBuf;

use egui::IconData;

pub const UI_FONTS: [&str; 2] = ["Hiragino Sans", "Helvetica Neue"];

pub const FILE_MANAGER: Option<&str> = Some("open");

/// The bundle keeps the manual under `Contents/Resources`, a sibling of the
/// `Contents/MacOS` directory the executable runs from.
pub fn manual_fallback() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    Some(
        executable
            .parent()?
            .parent()?
            .join("Resources/help/index.html"),
    )
}

pub const APP_DIRECTORY: &str = "RSSSTV";

/// The window is themed by AppKit from the system appearance, and the menu
/// bar is attached by muda, so nothing has to be arranged in advance.
pub fn prepare_process() {}

pub fn prepare_window(_cc: &eframe::CreationContext<'_>) {}

/// Keeping the machine awake needs an `IOPMAssertion`, which is not wired up
/// yet, so activity is accepted and discarded.
pub type Host = super::InertPlatform;

pub type Claim = super::FileLock;

pub fn claim_single_instance() -> Option<Claim> {
    super::lock_file_claim()
}

pub fn window_icon() -> Option<IconData> {
    super::embedded_icon()
}
