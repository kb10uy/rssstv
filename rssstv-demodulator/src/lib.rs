//! MMSSTV-style audio front end for SSTV receive decoding.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod afc;
mod demodulator;
mod error;
mod frontend;
mod hilbert;
mod sync;
mod vis;

use rssstv_sstv::{mode::Mode, time::SstvDuration};

pub use demodulator::{DemodulatedAudio, DemodulatedChunk, Demodulator, demodulate};
pub use error::DemodulatorError;
pub use sync::SyncStart;

const SYNC_DETECTOR_DELAY_PS: u64 = 6_000_000_000;

/// Returns sync-envelope delay relative to demodulated frequency output.
///
/// The receive decoder uses this to place its search for a pulse, not to place
/// the raster, so the single measured figure covers every supported mode even
/// though the envelope's exact lag varies with pulse length and picture content.
///
/// Modes without an implemented raster decoder return zero.
pub const fn sync_detector_delay(mode: Mode) -> SstvDuration {
    let delay = match mode {
        Mode::Martin1
        | Mode::Martin2
        | Mode::Scottie1
        | Mode::Scottie2
        | Mode::ScottieDx
        | Mode::Robot36
        | Mode::Robot72
        | Mode::Pd50
        | Mode::Pd90
        | Mode::Pd120
        | Mode::Pd160
        | Mode::Pd180
        | Mode::Pd240
        | Mode::Pd290 => SYNC_DETECTOR_DELAY_PS,
        _ => 0,
    };
    SstvDuration::from_picos(delay)
}
