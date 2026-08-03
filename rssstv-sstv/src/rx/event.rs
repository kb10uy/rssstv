use crate::image::RgbImage;

/// Observable state of a receive decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxState {
    /// Collecting a bounded synchronization acquisition window.
    Acquiring,
    /// Raster timing is acquired and image rows are being decoded.
    Decoding {
        /// Number of image rows whose row events have been delivered.
        completed_rows: usize,
    },
    /// Every active image row has been decoded and delivered.
    Complete,
    /// Synchronization was persistently unusable and AutoStop terminated decoding.
    Stopped {
        /// Number of image rows delivered before stopping.
        completed_rows: usize,
        /// Why automatic termination occurred.
        reason: StopReason,
    },
}

/// Terminal AutoStop cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    /// Weak, missing, or inconsistent sync persisted until the leaky error
    /// score reached its termination threshold.
    SynchronizationLost,
}

/// One bounded notification produced by [`super::RxDecoder::process`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RxEvent {
    /// The raster rate was refitted while decoding and the rows already
    /// decoded were redrawn from retained samples.
    SlantAdjusted {
        /// Raster unit after which the refit was applied.
        unit: usize,
        /// Effective physical samples per protocol second after the refit.
        effective_sample_rate_hz: f64,
    },
    /// A stable raster clock was acquired.
    RasterAcquired {
        /// Physical source sample corresponding to the effective raster epoch.
        source_epoch: u64,
        /// Effective physical samples per protocol second.
        effective_sample_rate_hz: f64,
    },
    /// One output row is now available in the decoder image.
    RowDecoded {
        /// Zero-based image row.
        row: usize,
    },
    /// Live synchronization changed the raster source epoch.
    PhaseAdjusted {
        /// Raster unit whose stable observations triggered the correction.
        unit: usize,
        /// Signed epoch correction in physical samples.
        displacement_samples: i64,
        /// Corrected absolute raster epoch.
        source_epoch: u64,
    },
    /// AutoStop reached a normal terminal state.
    Stopped {
        /// AutoStop cause.
        reason: StopReason,
    },
}

/// Result of one streaming process call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxProcess {
    consumed: usize,
    event: Option<RxEvent>,
}

impl RxProcess {
    pub(super) const fn new(consumed: usize, event: Option<RxEvent>) -> Self {
        Self { consumed, event }
    }

    /// Returns the consumed prefix length of the supplied block.
    pub const fn consumed(self) -> usize {
        self.consumed
    }

    /// Returns the single event, if any.
    pub const fn event(self) -> Option<RxEvent> {
        self.event
    }
}

/// Owned result returned when receive processing is finished by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RxOutcome {
    /// All active rows were decoded.
    Complete(RgbImage),
    /// Input ended before all active rows were decoded.
    Incomplete {
        /// Partially decoded image.
        image: RgbImage,
        /// State at the end of input.
        state: RxState,
    },
    /// AutoStop terminated normally with a partial image.
    Stopped {
        /// Partially decoded image.
        image: RgbImage,
        /// AutoStop cause.
        reason: StopReason,
    },
}
