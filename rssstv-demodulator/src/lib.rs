//! MMSSTV-style audio front end for SSTV receive decoding.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod afc;
mod demodulator;
mod error;
mod frontend;
mod hilbert;
mod pipeline;
mod sync;
mod vis;

use rssstv_sstv::{
    mode::{Mode, Support},
    time::SstvDuration,
};

pub use demodulator::{DemodulatedAudio, DemodulatedChunk, Demodulator, demodulate};
pub use error::DemodulatorError;
pub use frontend::Detection;
pub use pipeline::{PipelineError, PipelineOptions, PipelineOutcome, ReceivePipeline};
pub use sync::SyncStart;
pub use vis::VisDetection;

const SYNC_DETECTOR_DELAY_PS: u64 = 6_000_000_000;

/// Returns sync-envelope delay relative to demodulated frequency output.
///
/// The receive decoder uses this to place its search for a pulse, not to place
/// the raster, so the single measured figure covers every supported mode even
/// though the envelope's exact lag varies with pulse length and picture content.
///
/// Modes without an implemented raster decoder return zero. Which modes have
/// one is read from the mode table, so a decoder landing there is enough for
/// its delay to apply.
pub const fn sync_detector_delay(mode: Mode) -> SstvDuration {
    match mode.spec().decode_support() {
        Support::Supported => SstvDuration::from_picos(SYNC_DETECTOR_DELAY_PS),
        _ => SstvDuration::ZERO,
    }
}
