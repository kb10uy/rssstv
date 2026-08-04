//! The threads that run a reception or a transmission, and the audio devices
//! they are attached to.
//!
//! The interface never blocks on any of this. Each worker owns its own state
//! and publishes a snapshot the interface reads once per frame.

pub mod audio;
pub mod compose;
pub mod receive;
pub mod transmit;
