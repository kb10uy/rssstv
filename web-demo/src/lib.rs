//! Browser bindings for the RSSSTV receive pipeline.
//!
//! The whole decode path is portable, so this crate adds no signal processing
//! of its own: it drives [`ReceivePipeline`] with the PCM the page hands it and
//! reports what the decoder is doing in shapes JavaScript can read.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use rssstv_demodulator::{PipelineOptions, ReceivePipeline};
use rssstv_sstv::{
    image::RgbImage,
    mode::Mode,
    rx::{RxEvent, RxOutcome, RxState, StopReason},
};
use wasm_bindgen::prelude::*;

const MINIMUM_SAMPLE_RATE_HZ: f64 = 6_000.0;
const LOG_LIMIT: usize = 1_024;

/// A finished reception, kept because [`ReceivePipeline::finish`] consumes the
/// pipeline while the JavaScript object outlives it.
struct Finished {
    mode: Mode,
    image: RgbImage,
    state: &'static str,
    completed_rows: u32,
    frequency_offset_hz: f64,
    effective_sample_rate_hz: Option<f64>,
    callsigns: Vec<String>,
    image_revision: u64,
}

enum Stage {
    Running(Box<ReceivePipeline>),
    Finished(Box<Finished>),
}

/// What the page displays after a call to [`SstvReceiver::status`].
#[wasm_bindgen(getter_with_clone)]
pub struct ReceiverStatus {
    /// Detected mode's displayed name, once VIS identifies one.
    pub mode_name: Option<String>,
    /// One of `waiting`, `acquiring`, `decoding`, `complete`, `incomplete`, or
    /// `stopped`.
    pub state: String,
    /// Image rows the decoder has delivered.
    pub completed_rows: u32,
    /// Decoded image width, zero before a mode is detected.
    pub width: u32,
    /// Decoded image height, zero before a mode is detected.
    pub height: u32,
    /// Counter the decoder bumps whenever the image changes.
    pub image_revision: f64,
    /// The front end's AFC offset in hertz.
    pub frequency_offset_hz: f64,
    /// The fitted raster rate, once a raster is acquired.
    pub effective_sample_rate_hz: Option<f64>,
    /// FSKID callsigns, which are only known once the stream is finished.
    pub callsigns: Vec<String>,
    /// Peak amplitude of the most recently pushed packet.
    pub level: f32,
    /// Whether the stream has been closed and no more PCM is accepted.
    pub finished: bool,
}

/// Drives one reception from PCM to a decoded image.
#[wasm_bindgen]
pub struct SstvReceiver {
    stage: Stage,
    sample_rate_hz: u32,
    options: PipelineOptions,
    log: Vec<String>,
    level: f32,
}

#[wasm_bindgen]
impl SstvReceiver {
    /// Builds a receiver for PCM arriving at `sample_rate_hz`.
    ///
    /// Pass the rate of the source itself — an `AudioBuffer`'s or an
    /// `AudioContext`'s `sampleRate` — because nothing here resamples. Rates
    /// below 6 kHz are rejected.
    ///
    /// `live_slant` refits the raster while the picture is still arriving, and
    /// `staging_seconds` bounds how much demodulated audio is retained for that
    /// and for the refit performed by [`SstvReceiver::finish`], at three bytes
    /// per sample.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sample_rate_hz: f64,
        live_slant: bool,
        staging_seconds: f64,
    ) -> Result<SstvReceiver, JsError> {
        console_error_panic_hook::set_once();
        if !sample_rate_hz.is_finite() || sample_rate_hz < MINIMUM_SAMPLE_RATE_HZ {
            return Err(JsError::new(&format!(
                "the sample rate must be at least {MINIMUM_SAMPLE_RATE_HZ} Hz, not {sample_rate_hz}"
            )));
        }
        if !staging_seconds.is_finite() || staging_seconds < 0.0 {
            return Err(JsError::new("the staging length must not be negative"));
        }
        let sample_rate_hz = sample_rate_hz.round() as u32;
        let options = PipelineOptions {
            live_slant,
            staging_max_samples: (f64::from(sample_rate_hz) * staging_seconds) as usize,
        };
        Ok(SstvReceiver {
            stage: Stage::Running(Box::new(open(sample_rate_hz, options)?)),
            sample_rate_hz,
            options,
            log: Vec::new(),
            level: 0.0,
        })
    }

    /// Feeds one packet of normalized mono PCM.
    pub fn push(&mut self, samples: &[f32]) -> Result<(), JsError> {
        let Stage::Running(pipeline) = &mut self.stage else {
            return Err(JsError::new("the stream is already finished"));
        };
        self.level = samples.iter().fold(0.0_f32, |peak, sample| {
            let magnitude = sample.abs();
            if magnitude > peak { magnitude } else { peak }
        });
        let mut entries = Vec::new();
        pipeline
            .process(samples, |event| {
                if let Some(entry) = describe(event) {
                    entries.push(entry);
                }
            })
            .map_err(|error| JsError::new(&error.to_string()))?;
        self.record(entries);
        Ok(())
    }

    /// Reports what the decoder is doing.
    pub fn status(&self) -> ReceiverStatus {
        match &self.stage {
            Stage::Running(pipeline) => {
                let mode = pipeline.demodulator().mode();
                let Some(decoder) = pipeline.decoder() else {
                    return ReceiverStatus {
                        mode_name: mode.map(|mode| mode.spec().name().to_owned()),
                        state: "waiting".to_owned(),
                        completed_rows: 0,
                        width: 0,
                        height: 0,
                        image_revision: 0.0,
                        frequency_offset_hz: pipeline.demodulator().frequency_offset_hz(),
                        effective_sample_rate_hz: None,
                        callsigns: Vec::new(),
                        level: self.level,
                        finished: false,
                    };
                };
                let (state, completed_rows) = match decoder.state() {
                    RxState::Acquiring => ("acquiring", 0),
                    RxState::Decoding { completed_rows } => ("decoding", completed_rows as u32),
                    RxState::Complete => ("complete", decoder.image().size().height() as u32),
                    RxState::Stopped { completed_rows, .. } => ("stopped", completed_rows as u32),
                };
                let size = decoder.image().size();
                ReceiverStatus {
                    mode_name: Some(decoder.mode().spec().name().to_owned()),
                    state: state.to_owned(),
                    completed_rows,
                    width: size.width() as u32,
                    height: size.height() as u32,
                    image_revision: decoder.image_revision() as f64,
                    frequency_offset_hz: pipeline.demodulator().frequency_offset_hz(),
                    effective_sample_rate_hz: decoder.effective_sample_rate_hz(),
                    callsigns: Vec::new(),
                    level: self.level,
                    finished: false,
                }
            }
            Stage::Finished(finished) => ReceiverStatus {
                mode_name: Some(finished.mode.spec().name().to_owned()),
                state: finished.state.to_owned(),
                completed_rows: finished.completed_rows,
                width: finished.image.size().width() as u32,
                height: finished.image.size().height() as u32,
                image_revision: finished.image_revision as f64,
                frequency_offset_hz: finished.frequency_offset_hz,
                effective_sample_rate_hz: finished.effective_sample_rate_hz,
                callsigns: finished.callsigns.clone(),
                level: self.level,
                finished: true,
            },
        }
    }

    /// Returns the decoded image as RGBA bytes, ready for `ImageData`.
    ///
    /// Repainting on every frame would copy the picture for nothing, so call
    /// this only when `image_revision` has changed.
    pub fn image_rgba(&self) -> Option<Vec<u8>> {
        let image = match &self.stage {
            Stage::Running(pipeline) => pipeline.decoder()?.image(),
            Stage::Finished(finished) => &finished.image,
        };
        let mut rgba = Vec::with_capacity(image.pixels().len() * 4);
        for pixel in image.pixels() {
            rgba.extend_from_slice(&[pixel.r, pixel.g, pixel.b, 0xff]);
        }
        Some(rgba)
    }

    /// Takes the events recorded since the last call.
    pub fn drain_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Closes the stream, refits the raster from the staged audio, and reports.
    pub fn finish(&mut self) -> Result<ReceiverStatus, JsError> {
        let stage = core::mem::replace(
            &mut self.stage,
            Stage::Running(Box::new(open(self.sample_rate_hz, self.options)?)),
        );
        let Stage::Running(pipeline) = stage else {
            self.stage = stage;
            return Err(JsError::new("the stream is already finished"));
        };
        let image_revision = pipeline
            .decoder()
            .map_or(0, |decoder| decoder.image_revision())
            + 1;
        let outcome = pipeline
            .finish()
            .map_err(|error| JsError::new(&error.to_string()))?;
        let (image, state, completed_rows) = match outcome.outcome {
            RxOutcome::Complete(image) => {
                let rows = image.size().height() as u32;
                (image, "complete", rows)
            }
            RxOutcome::Incomplete { image, state } => {
                let rows = match state {
                    RxState::Decoding { completed_rows }
                    | RxState::Stopped { completed_rows, .. } => completed_rows as u32,
                    RxState::Complete => image.size().height() as u32,
                    RxState::Acquiring => 0,
                };
                (image, "incomplete", rows)
            }
            RxOutcome::Stopped { image, .. } => {
                let rows = image.size().height() as u32;
                (image, "stopped", rows)
            }
        };
        self.stage = Stage::Finished(Box::new(Finished {
            mode: outcome.mode,
            image,
            state,
            completed_rows,
            frequency_offset_hz: outcome.frequency_offset_hz,
            effective_sample_rate_hz: outcome.effective_sample_rate_hz,
            callsigns: outcome
                .fsk_ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            image_revision,
        }));
        Ok(self.status())
    }

    /// Starts over at the same sample rate, discarding the current reception.
    ///
    /// The pipeline does not restart itself on a second VIS header, so a page
    /// that receives one picture after another calls this between them.
    pub fn reset(&mut self) -> Result<(), JsError> {
        self.stage = Stage::Running(Box::new(open(self.sample_rate_hz, self.options)?));
        self.log.clear();
        self.level = 0.0;
        Ok(())
    }
}

impl SstvReceiver {
    fn record(&mut self, entries: Vec<String>) {
        self.log.extend(entries);
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }
}

fn open(sample_rate_hz: u32, options: PipelineOptions) -> Result<ReceivePipeline, JsError> {
    ReceivePipeline::new(sample_rate_hz, options).map_err(|error| JsError::new(&error.to_string()))
}

fn describe(event: &RxEvent) -> Option<String> {
    match event {
        RxEvent::RowDecoded { .. } => None,
        RxEvent::RasterAcquired {
            source_epoch,
            effective_sample_rate_hz,
        } => Some(format!(
            "raster acquired at sample {source_epoch}, {effective_sample_rate_hz:.2} Hz"
        )),
        RxEvent::SlantAdjusted {
            unit,
            effective_sample_rate_hz,
        } => Some(format!(
            "slant refitted after unit {unit}, {effective_sample_rate_hz:.2} Hz"
        )),
        RxEvent::PhaseAdjusted {
            unit,
            displacement_samples,
            ..
        } => Some(format!(
            "phase corrected by {displacement_samples:+} samples at unit {unit}"
        )),
        RxEvent::Stopped {
            reason: StopReason::SynchronizationLost,
        } => Some("stopped: synchronization lost".to_owned()),
    }
}
