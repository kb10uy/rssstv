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
