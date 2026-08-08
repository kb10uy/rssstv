//! Platform integration, gathered behind one module.
//!
//! Everything that only makes sense on one operating system lives here, so the
//! rest of the application can be read without stepping over `#[cfg]`. Each
//! platform provides the same set of items and the compiler enforces that: a
//! new operation has to be answered on every platform before the build passes,
//! even if the answer is to do nothing.
//!
//! The menu bar is the one deliberate exception. It stays in [`crate::ui::menu`]
//! because its platform split is between two renderers of a shared model
//! rather than between operating systems.

use std::{
    io,
    path::Path,
    process::{Command, Stdio},
};

use egui::IconData;
use image::ImageFormat;

#[cfg_attr(target_os = "windows", path = "platform/windows.rs")]
#[cfg_attr(target_os = "macos", path = "platform/macos.rs")]
#[cfg_attr(
    not(any(target_os = "windows", target_os = "macos")),
    path = "platform/other.rs"
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

/// The directory name the application keeps its own files under.
///
/// Named by the platform because the conventions differ: the directories are
/// hidden on Linux and shown to the operator elsewhere.
pub use imp::APP_DIRECTORY;

/// Where an installed copy keeps the manual's first page, when the platform
/// has one.
///
/// A release archive carries the manual beside the executable, but an
/// installed copy puts the binary where nothing sits beside it: a Linux
/// package keeps the pages under `/usr/share/doc`, and the macOS bundle keeps
/// them in its own `Resources` directory.
pub use imp::manual_fallback;

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

/// Takes a window off the screen without destroying it.
///
/// Only Windows has anything to take off the screen this way: it is the one
/// platform where the menu bar owns a window handle of its own.
#[cfg(target_os = "windows")]
pub use imp::hide_window;

/// The application icon, compiled into the binary.
///
/// Used by the platforms that have nowhere else to read it from.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

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

/// What the application is doing, as far as the platform cares.
///
/// Reported so the machine does not go to sleep in the middle of a picture.
/// Only the states worth keeping the machine awake for are distinguished; an
/// open device that nothing is arriving on is [`Activity::Idle`], so leaving
/// the application running does not hold sleep off indefinitely.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Activity {
    #[default]
    Idle,
    /// A picture is being decoded.
    Receiving,
    /// A picture is being sent.
    Transmitting,
}

/// Platform side effects that follow application state.
///
/// Unlike the rest of this module these are not one-shot, so they are reached
/// through a trait: the application holds one of these, and a test can hold a
/// recording one instead and assert on what the interface asked for.
pub trait Platform {
    /// Reports what the application is doing.
    ///
    /// Called only when the activity changes, so an implementation may treat
    /// each call as a transition rather than a repeat.
    fn set_activity(&mut self, activity: Activity);

    /// Opens `path` with whatever the platform opens it with.
    ///
    /// Reached through the platform rather than called directly so a test can
    /// hold one that answers without launching anything: the interface offers
    /// this on every directory it keeps, and a suite that took each of them at
    /// its word would bury the machine in file manager windows.
    fn open_path(&mut self, path: &Path) -> io::Result<()> {
        open_path(path)
    }
}

/// Returns the platform the application is running on.
pub fn host() -> Box<dyn Platform> {
    Box::<imp::Host>::default()
}

/// A platform with nothing to say about activity.
///
/// The real platform on the targets that cannot report it yet, so it is
/// compiled only there rather than being carried into a build that would never
/// construct it. Everything it does not answer for itself, such as opening a
/// directory, still reaches the shared implementation.
#[cfg(not(target_os = "windows"))]
#[derive(Default)]
pub struct InertPlatform;

#[cfg(not(target_os = "windows"))]
impl Platform for InertPlatform {
    fn set_activity(&mut self, _activity: Activity) {}
}

/// A platform that answers without doing anything at all.
///
/// `InertPlatform` is the real platform on the targets without their own
/// integration, so it still opens directories for the operator. Tests want the
/// opposite, and take this instead.
#[cfg(test)]
#[derive(Default)]
pub struct QuietPlatform;

#[cfg(test)]
impl Platform for QuietPlatform {
    fn set_activity(&mut self, _activity: Activity) {}

    fn open_path(&mut self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// A claim on being the only running copy of the application.
///
/// Held for the lifetime of the process; releasing it lets the next launch
/// through. The application takes a claim because it holds audio devices
/// open, and a second copy silently failing to acquire them is harder to
/// understand than not opening at all.
pub struct SingleInstance(imp::Claim);

/// Claims the right to be the only running copy.
///
/// Returns `None` when another copy already holds the claim, having first
/// asked that copy to come to the front, so the launch the operator just made
/// still puts the application in front of them.
pub fn claim_single_instance() -> Option<SingleInstance> {
    imp::claim_single_instance().map(SingleInstance)
}

impl SingleInstance {
    /// Records where this copy's window is, so a later launch can raise it.
    ///
    /// Called once the platform has created the window, which is the earliest
    /// point there is anything to record.
    pub fn publish_window(&self, cc: &eframe::CreationContext<'_>) {
        self.0.publish_window(cc);
    }
}

/// A claim backed by a locked file, for the platforms without a better one.
///
/// The lock is released by the operating system when the process ends, so a
/// copy that crashed does not keep the next launch out.
#[cfg(not(target_os = "windows"))]
pub struct FileLock {
    /// Never read: the lock lasts exactly as long as the file is open, so
    /// holding the handle is the whole point of the field.
    #[expect(dead_code, reason = "held open to hold the lock")]
    file: std::fs::File,
}

#[cfg(not(target_os = "windows"))]
impl FileLock {
    /// Nothing is published: raising another process's window needs a
    /// compositor-specific route that this application does not have yet, so
    /// a second launch reports the situation and exits instead.
    pub fn publish_window(&self, _cc: &eframe::CreationContext<'_>) {}
}

/// Takes an exclusive lock on a file named after the application.
#[cfg(not(target_os = "windows"))]
fn lock_file_claim() -> Option<FileLock> {
    use std::fs::OpenOptions;

    let directory = directories::BaseDirs::new()
        .and_then(|directories| directories.runtime_dir().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(concat!(env!("CARGO_PKG_NAME"), ".lock")))
        .ok()?;
    file.try_lock().ok()?;
    Some(FileLock { file })
}

/// Opens `path` with whatever the platform opens it with.
///
/// A directory reaches the file manager and the manual reaches the browser
/// through one command, because each platform's file manager is also its shell
/// opener. There is nothing to choose between here, so nothing does.
///
/// The child is waited on by a detached thread rather than left unclaimed, so
/// a long-lived session does not accumulate zombies on the platforms that
/// create them.
pub fn open_path(path: &Path) -> io::Result<()> {
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
