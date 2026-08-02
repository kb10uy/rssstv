/// Retention policy for demodulated samples used by staged refinement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Staging {
    /// Do not retain samples after they leave the streaming working buffer.
    #[default]
    Disabled,
    /// Retain at most `max_samples` paired frequency and synchronization samples.
    Memory {
        /// Hard sample limit. Appending beyond it returns a typed error.
        max_samples: usize,
    },
}

/// Receive synchronization and retention options.
///
/// Synchronization strengths are expected to be normalized to `0.0..=1.0`.
/// Live synchronization accepts a pulse only when its peak is at least `0.35`
/// and its local peak-to-background contrast is at least `0.20`; these are
/// relative normalized criteria, not MMSSTV's integer-domain thresholds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxConfig {
    /// Enables stable live raster epoch corrections without altering input samples.
    pub live_sync: bool,
    /// Stops normally after synchronization failures persist in leaky history.
    pub auto_stop: bool,
    /// Controls immutable sample retention for whole-image refinement.
    pub staging: Staging,
}

impl Default for RxConfig {
    fn default() -> Self {
        Self {
            live_sync: false,
            auto_stop: false,
            staging: Staging::Disabled,
        }
    }
}
