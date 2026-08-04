use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Failure reported by an audio adapter.
///
/// Backend types are deliberately not exposed: the host API is an
/// implementation detail of this crate.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AudioError {
    /// No device matched the requested name.
    #[error("no audio device named `{0}` is available")]
    DeviceNotFound(String),
    /// The device exists but offers no configuration this crate can use.
    #[error("audio device `{0}` offers no usable configuration")]
    UnsupportedConfiguration(String),
    /// The requested capture buffer capacity was zero.
    #[error("audio queue capacity must be greater than zero")]
    EmptyCapacity,
    /// The host API failed.
    #[error("audio backend failure: {0}")]
    Backend(String),
}

/// Why a stream that was running stopped on its own.
///
/// The host's own error kinds are not exposed; these are the distinctions the
/// application can act on differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultKind {
    /// The device went away, most often unplugged or switched off.
    Disconnected,
    /// The stream has to be built again before it will carry samples.
    Invalidated,
    /// The host failed in a way this crate cannot classify.
    Backend,
}

/// A stream that stopped without being asked to.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{device}: {detail}")]
pub struct StreamFault {
    /// The device the stream was running on, named as the operator saw it.
    pub device: String,
    pub kind: FaultKind,
    /// What the host said, for the log.
    pub detail: String,
}

/// Where a stream's error callback leaves what it saw.
///
/// The callback runs on the host's own thread and cannot wait for the
/// interface, so it drops a report here and returns. The first report is kept
/// until it is taken: a stream that fails usually fails repeatedly, and the
/// first is the one that says why.
#[derive(Clone, Debug, Default)]
pub struct FaultSlot(Arc<Mutex<Option<StreamFault>>>);

impl FaultSlot {
    pub(crate) fn report(&self, fault: StreamFault) {
        let mut slot = match self.0.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        slot.get_or_insert(fault);
    }

    /// Takes the pending report, if a stream left one.
    pub fn take(&self) -> Option<StreamFault> {
        match self.0.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}
