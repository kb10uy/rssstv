use crate::time::SstvDuration;

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
/// Acquisition extracts pulse runs at `0.50` or above.
/// Live synchronization accepts a pulse only when its peak is at least `0.35`
/// and its local peak-to-background contrast is at least `0.20`; these are
/// relative normalized criteria, not MMSSTV's integer-domain thresholds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxConfig {
    /// Enables stable live raster epoch corrections without altering input samples.
    pub live_sync: bool,
    /// Stops normally after synchronization failures persist in leaky history.
    pub auto_stop: bool,
    /// Compensates synchronization-envelope delay relative to frequency input.
    ///
    /// Leave this at zero for synchronization samples already aligned with the
    /// frequency input.
    pub sync_detector_delay: SstvDuration,
    /// Controls immutable sample retention for whole-image refinement.
    pub staging: Staging,
}

pub(super) fn sync_detector_delay_samples(
    sample_rate_hz: u32,
    sync_detector_delay: SstvDuration,
) -> f64 {
    f64::from(sample_rate_hz) * sync_detector_delay.as_picos() as f64 / 1.0e12
}
