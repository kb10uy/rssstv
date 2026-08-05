//! The failures the interface reports, carried as values rather than text.
//!
//! A worker runs on its own thread and publishes what went wrong in a
//! snapshot, so an error has to survive being cloned out of a mutex. Every
//! error the application shows is one of these, which keeps the point where a
//! failure is described apart from the point where it is displayed.

use std::sync::Arc;

use rssstv_audio::AudioError;
use rssstv_demodulator::DemodulatorError;
use rssstv_dsp::DspError;
use rssstv_modulator::ModulatorError;
use rssstv_rig::RigError;
use rssstv_sstv::SstvError;
use rssstv_template::TemplateError;
use thiserror::Error;

use crate::worker::rig::script::ScriptError;

/// Anything the interface has to tell the operator went wrong.
#[derive(Clone, Debug, Error)]
pub enum AppError {
    /// An audio device could not be opened, listed, or kept running.
    #[error(transparent)]
    Audio(#[from] AudioError),
    /// The receive front end rejected its configuration or its samples.
    #[error(transparent)]
    Demodulator(#[from] DemodulatorError),
    /// The transmit tone stream rejected its configuration.
    #[error(transparent)]
    Modulator(#[from] ModulatorError),
    /// A signal-processing primitive rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] DspError),
    /// An SSTV protocol value or operation was invalid.
    #[error(transparent)]
    Sstv(#[from] SstvError),
    /// A rig transport failed.
    #[error(transparent)]
    Rig(#[from] RigError),
    /// The operator's rig control script could not be loaded or run.
    #[error(transparent)]
    Script(#[from] ScriptError),
    /// A template could not be parsed, rendered, or composed.
    ///
    /// Held behind an [`Arc`] because it carries KDL, image, and SVG errors
    /// that are not themselves cloneable.
    #[error("{0}")]
    Template(Arc<TemplateError>),

    /// A transmission was asked for with no output device selected.
    #[error("no output device is selected")]
    NoOutputDevice,
    /// The playback stream was closed before a transmission could start.
    #[error("playback closed before the transmission started")]
    PlaybackClosed,
    /// The raster decoder rejected a demodulated block.
    #[error("receive decoding failed at sample {sample}: {source}")]
    Decode {
        /// Absolute PCM sample position the decoder stopped at.
        sample: u64,
        /// Why it stopped.
        #[source]
        source: SstvError,
    },
    /// Trailing audio could not be staged for the slant refit.
    #[error("staging the refinement tail failed: {0}")]
    RefinementStaging(#[source] SstvError),
    /// The slant refit over the staged reception failed.
    #[error("slant refinement failed: {0}")]
    Refinement(#[source] SstvError),
    /// Reception could not be restarted after captured audio was lost.
    #[error("reception could not be restarted after a capture overrun")]
    CaptureRestartFailed,
    /// The playback queue ran dry while a picture was being sent.
    #[error("audio playback ran out of samples")]
    PlaybackUnderrun,
    /// A worker panicked while holding its snapshot, so its state is unknown.
    #[error("{0} state is unavailable")]
    WorkerUnavailable(&'static str),
}

impl From<TemplateError> for AppError {
    fn from(error: TemplateError) -> Self {
        Self::Template(Arc::new(error))
    }
}
