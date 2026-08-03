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
