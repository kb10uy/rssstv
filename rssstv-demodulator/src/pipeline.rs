//! The demodulator-to-decoder wiring every offline receiver repeats.
//!
//! A VIS detection constructs the decoder, and each demodulated chunk is then
//! fed to it in as many calls as it takes. The loop looks simple but has been
//! copied wrong before, so it lives here once; the application's receive
//! worker keeps a loop of its own because it also restarts, publishes, and
//! refines mid-stream.

use rssstv_sstv::{
    RxDecoder, SstvError,
    mode::Mode,
    rx::{RxConfig, RxEvent, RxOutcome, RxState, Staging},
};

use rssstv_fskid::FskId;
use thiserror::Error;

use crate::{DemodulatedChunk, Demodulator, DemodulatorError, sync_detector_delay};

/// What a [`ReceivePipeline`] is told to do beyond the defaults.
#[derive(Clone, Copy, Debug)]
pub struct PipelineOptions {
    /// Whether the raster is refit while it is still being decoded.
    pub live_slant: bool,
    /// How many demodulated samples may be retained for refinement.
    pub staging_max_samples: usize,
}

/// An error from any stage of the wiring.
#[derive(Clone, Debug, Error)]
pub enum PipelineError {
    /// The front end rejected its configuration or its samples.
    #[error(transparent)]
    Demodulator(#[from] DemodulatorError),
    /// A detected mode has no raster decoder.
    #[error("the detected mode cannot be decoded: {0}")]
    UnsupportedMode(#[source] SstvError),
    /// The raster decoder rejected a demodulated block.
    #[error("receive decoding failed at sample {sample}: {source}")]
    Decode {
        /// Absolute sample position the decoder stopped at.
        sample: u64,
        /// Why it stopped.
        #[source]
        source: SstvError,
    },
    /// The decoder neither consumed samples nor produced an event.
    #[error("receive decoder made no progress")]
    Stalled,
    /// Image data arrived before any mode was detected.
    #[error("demodulator produced image data before VIS detection")]
    DataBeforeDetection,
    /// The stream ended without a detected mode.
    #[error("no SSTV mode was detected")]
    NotDetected,
    /// The staged refit over the finished reception failed.
    #[error("slant refinement failed: {0}")]
    Refinement(#[source] SstvError),
}

/// What the stream amounted to once it ended.
#[derive(Debug)]
pub struct PipelineOutcome {
    /// The detected mode.
    pub mode: Mode,
    /// The front end's final AFC offset in hertz.
    pub frequency_offset_hz: f64,
    /// The fitted raster rate, when a raster was acquired.
    pub effective_sample_rate_hz: Option<f64>,
    /// The decoded image and how far decoding got.
    pub outcome: RxOutcome,
    /// Every station identifier decoded along the way.
    pub fsk_ids: Vec<FskId>,
}

/// Drives PCM through the demodulator and the decoder it detects.
pub struct ReceivePipeline {
    demodulator: Demodulator,
    decoder: Option<RxDecoder>,
    options: PipelineOptions,
    fsk_ids: Vec<FskId>,
}

impl ReceivePipeline {
    /// Builds the front end; the decoder follows once a mode is detected.
    pub fn new(sample_rate_hz: u32, options: PipelineOptions) -> Result<Self, DemodulatorError> {
        Ok(Self {
            demodulator: Demodulator::new(sample_rate_hz)?,
            decoder: None,
            options,
            fsk_ids: Vec::new(),
        })
    }

    /// Feeds one packet of normalized mono PCM through the whole pipeline.
    ///
    /// `on_event` hears every decoder event in order, for a caller that
    /// traces or counts them; one that does not passes `|_| {}`.
    pub fn process(
        &mut self,
        samples: &[f32],
        on_event: impl FnMut(&RxEvent),
    ) -> Result<(), PipelineError> {
        let chunk = self.demodulator.process(samples)?;
        self.feed(&chunk, on_event)
    }

    fn feed(
        &mut self,
        chunk: &DemodulatedChunk,
        mut on_event: impl FnMut(&RxEvent),
    ) -> Result<(), PipelineError> {
        self.fsk_ids.extend_from_slice(chunk.fsk_ids());
        if let Some(mode) = chunk.detected_mode() {
            self.decoder = Some(
                RxDecoder::with_config(
                    mode,
                    self.demodulator.sample_rate_hz(),
                    RxConfig {
                        live_sync: true,
                        live_slant: self.options.live_slant,
                        auto_stop: false,
                        sync_detector_delay: sync_detector_delay(mode),
                        staging: Staging::Memory {
                            max_samples: self.options.staging_max_samples,
                        },
                    },
                )
                .map_err(PipelineError::UnsupportedMode)?,
            );
        }
        if chunk.frequency_hz().is_empty() {
            return Ok(());
        }
        let decoder = self
            .decoder
            .as_mut()
            .ok_or(PipelineError::DataBeforeDetection)?;
        let mut offset = 0;
        while offset < chunk.frequency_hz().len() {
            let sample = chunk.first_sample() + offset as u64;
            match decoder.state() {
                RxState::Complete => {
                    decoder
                        .stage_for_refinement(chunk.block_from(offset))
                        .map_err(|source| PipelineError::Decode { sample, source })?;
                    break;
                }
                RxState::Stopped { .. } => break,
                _ => {}
            }
            let processed = decoder.process(chunk.block_from(offset)).map_err(|error| {
                PipelineError::Decode {
                    sample: sample + error.consumed() as u64,
                    source: error.error(),
                }
            })?;
            if let Some(event) = processed.event() {
                on_event(&event);
            }
            offset += processed.consumed();
            if processed.consumed() == 0 && processed.event().is_none() {
                return Err(PipelineError::Stalled);
            }
        }
        Ok(())
    }

    /// Returns the decoder, once a mode has been detected.
    pub fn decoder(&self) -> Option<&RxDecoder> {
        self.decoder.as_ref()
    }

    /// Returns the decoder mutably, for a caller that closes it out by hand
    /// rather than through [`ReceivePipeline::finish`].
    pub fn decoder_mut(&mut self) -> Option<&mut RxDecoder> {
        self.decoder.as_mut()
    }

    /// Returns the front end being driven.
    pub const fn demodulator(&self) -> &Demodulator {
        &self.demodulator
    }

    /// Closes the stream, refines a completed raster, and reports.
    pub fn finish(mut self) -> Result<PipelineOutcome, PipelineError> {
        let mode = self.demodulator.finish()?;
        let frequency_offset_hz = self.demodulator.frequency_offset_hz();
        let mut decoder = self.decoder.take().ok_or(PipelineError::NotDetected)?;
        if decoder.state() == RxState::Complete {
            decoder.refine_staged().map_err(PipelineError::Refinement)?;
        }
        Ok(PipelineOutcome {
            mode,
            frequency_offset_hz,
            effective_sample_rate_hz: decoder.effective_sample_rate_hz(),
            outcome: decoder.finish(),
            fsk_ids: self.fsk_ids,
        })
    }
}
