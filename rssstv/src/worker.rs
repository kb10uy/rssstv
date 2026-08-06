//! The threads that run a reception or a transmission, and the audio devices
//! they are attached to.
//!
//! The interface never blocks on any of this. Each worker owns its own state
//! and publishes a snapshot the interface reads once per frame.

pub mod audio;
pub mod compose;
pub mod receive;
pub mod rig;
pub mod transmit;

use std::sync::Mutex;

/// Applies a change to a published snapshot, leaving it alone if the worker
/// that owns it panicked while holding the lock.
pub(crate) fn update<T>(snapshot: &Mutex<T>, update: impl FnOnce(&mut T)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

/// Asks the interface to draw a frame it has no other reason to draw.
///
/// A worker runs ahead of the interface, and nothing the operator does makes a
/// decoded row appear, so the frame that shows one has to be asked for by
/// whoever produced it. Without this the interface would have to redraw at the
/// monitor's rate whether or not anything had happened.
///
/// A waker with no context behind it does nothing, which is what a worker
/// driven by a test holds.
#[derive(Clone, Default)]
pub struct Waker(Option<egui::Context>);

impl Waker {
    pub const fn new(context: egui::Context) -> Self {
        Self(Some(context))
    }

    pub fn wake(&self) {
        if let Some(context) = self.0.as_ref() {
            context.request_repaint();
        }
    }
}

impl core::fmt::Debug for Waker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("Waker")
            .field(&self.0.is_some())
            .finish()
    }
}
