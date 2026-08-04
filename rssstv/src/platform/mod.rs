//! Platform integration, gathered behind one module.
//!
//! Everything that only makes sense on one operating system lives here, so the
//! rest of the application can be read without stepping over `#[cfg]`. Each
//! platform provides the same set of items and the compiler enforces that: a
//! new operation has to be answered on every platform before the build passes,
//! even if the answer is to do nothing.
//!
//! The menu bar is the one deliberate exception. It stays in [`crate::menu`]
//! because its platform split is between two renderers of a shared model
//! rather than between operating systems.

use std::{
    io,
    path::Path,
    process::{Command, Stdio},
};

use egui::IconData;
use image::ImageFormat;

#[cfg_attr(target_os = "windows", path = "windows.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(
    not(any(target_os = "windows", target_os = "macos")),
    path = "other.rs"
)]
mod imp;

/// Font families the interface is drawn with, in priority order.
///
/// The platform's own UI face is named rather than left to the font crate's
/// list, which puts `Noto Sans JP` first. On Windows that resolves to
/// `NotoSansJP-VF.ttf`, a variable font whose weight axis defaults to Thin;
/// egui does not apply variable axes, so the whole interface would be drawn
/// hairline.
pub use imp::UI_FONTS;

/// Prepares the process before any window exists.
///
/// Called once at startup, before the event loop is built, for work that has
/// to be in place before the first window is created.
pub use imp::prepare_process;

/// Prepares the main window once the platform has created it.
///
/// Called from the eframe creation hook, which is the earliest point a native
/// window handle exists.
pub use imp::prepare_window;

/// Returns the icon the window and task switcher should show.
///
/// Windows reads it back out of the executable's own resources, which the
/// build script embeds from `assets/icon.ico`: the shell already shows that
/// icon on the file, so the window shows the same artwork rather than a second
/// copy that could drift from it. Every other platform has no such resource
/// section and decodes [`embedded_icon`] instead, as does Windows when the
/// resource is missing because the build had no resource compiler.
pub use imp::window_icon;

/// The application icon, compiled into the binary.
///
/// Used by the platforms that have nowhere else to read it from.
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");

/// Decodes the icon embedded in the binary.
fn embedded_icon() -> Option<IconData> {
    let image = image::load_from_memory_with_format(ICON_PNG, ImageFormat::Png)
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Some(IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

/// Opens `path` in the platform's file manager.
///
/// The child is waited on by a detached thread rather than left unclaimed, so
/// a long-lived session does not accumulate zombies on the platforms that
/// create them.
pub fn reveal_directory(path: &Path) -> io::Result<()> {
    let Some(program) = imp::FILE_MANAGER else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "opening a directory is not supported on this platform",
        ));
    };

    let mut child = Command::new(program)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whichever platform this runs on has to produce a usable icon.
    #[test]
    fn the_platform_icon_loads() {
        let icon = window_icon().expect("the application icon should be available");
        assert!(icon.width > 0 && icon.height > 0);
        assert_eq!(
            icon.rgba.len(),
            icon.width as usize * icon.height as usize * 4
        );
        assert!(
            icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0),
            "the icon should not be fully transparent"
        );
    }

    /// The bundled copy has to stay decodable even where nothing reads it, so
    /// a platform that starts needing it is not surprised at runtime.
    #[test]
    fn the_embedded_icon_decodes() {
        let icon = embedded_icon().expect("the embedded icon should decode");
        assert_eq!(icon.width, icon.height);
    }
}
