use rssstv_fskid::FskId;
use rssstv_sstv::{mode::Mode, time::SstvDuration};

use crate::{DemodulatorError, frontend::FrontEnd, sync::SyncStart, sync_detector_delay};

/// Owned output produced from one incremental PCM input packet.
#[derive(Clone, Debug)]
pub struct DemodulatedChunk {
    detected_mode: Option<Mode>,
    first_sample: u64,
    frequency_hz: Vec<f32>,
    sync_strength: Vec<f32>,
    fsk_ids: Vec<FskId>,
}

impl DemodulatedChunk {
    /// Returns a mode when one was first identified in this packet, by a
    /// conventional VIS header or by [`SyncStart`] interval matching.
    pub const fn detected_mode(&self) -> Option<Mode> {
        self.detected_mode
    }

    /// Returns the absolute position of the first output sample.
    pub const fn first_sample(&self) -> u64 {
        self.first_sample
    }

    /// Returns AFC-corrected instantaneous image frequencies.
    pub fn frequency_hz(&self) -> &[f32] {
        &self.frequency_hz
    }

    /// Returns causal normalized horizontal-sync confidence.
    pub fn sync_strength(&self) -> &[f32] {
        &self.sync_strength
    }

    /// Returns validated station identifiers completed in this packet.
    pub fn fsk_ids(&self) -> &[FskId] {
        &self.fsk_ids
    }
}

/// Stateful incremental SSTV receive front end.
pub struct Demodulator {
    front_end: FrontEnd,
    sample_rate_hz: u32,
    next_sample: u64,
    mode: Option<Mode>,
    finished: bool,
}

impl Demodulator {
    /// Constructs a receive front end for a physical PCM sample rate.
    pub fn new(sample_rate_hz: u32) -> Result<Self, DemodulatorError> {
        if sample_rate_hz < 6_000 {
            return Err(DemodulatorError::SampleRateTooLow(sample_rate_hz));
        }
        Ok(Self {
            front_end: FrontEnd::new(f64::from(sample_rate_hz))?,
            sample_rate_hz,
            next_sample: 0,
            mode: None,
            finished: false,
        })
    }

    /// Processes one contiguous packet of normalized mono PCM.
    pub fn process(&mut self, samples: &[f32]) -> Result<DemodulatedChunk, DemodulatorError> {
        if self.finished {
            return Err(DemodulatorError::AlreadyFinished);
        }
        let end_sample = self
            .next_sample
            .checked_add(samples.len() as u64)
            .ok_or(DemodulatorError::SamplePositionOverflow)?;
        for (offset, sample) in samples.iter().enumerate() {
            if !sample.is_finite() {
                return Err(DemodulatorError::NonFiniteSample {
                    index: self.next_sample + offset as u64,
                });
            }
        }

        let mut detected_mode = None;
        let mut first_sample = self.next_sample;
        let mut frequency_hz = Vec::with_capacity(if self.mode.is_some() {
            samples.len()
        } else {
            0
        });
        let mut sync_strength = Vec::with_capacity(frequency_hz.capacity());
        let mut fsk_ids = Vec::new();
        for &sample in samples {
            let output = self.front_end.process(f64::from(sample))?;
            self.next_sample += 1;
            if let Some(id) = output.fsk_id {
                fsk_ids.push(id);
            }
            if self.mode.is_none()
                && let Some(mode) = output.mode
            {
                self.mode = Some(mode);
                detected_mode = Some(mode);
                first_sample = self.next_sample;
                self.front_end.enable_afc();
                continue;
            }
            if self.mode.is_some() {
                if frequency_hz.is_empty() {
                    first_sample = self.next_sample - 1;
                }
                frequency_hz.push(output.frequency_hz as f32);
                sync_strength.push(output.sync_strength as f32);
            }
        }
        debug_assert_eq!(self.next_sample, end_sample);

        Ok(DemodulatedChunk {
            detected_mode,
            first_sample: if frequency_hz.is_empty() {
                self.next_sample
            } else {
                first_sample
            },
            frequency_hz,
            sync_strength,
            fsk_ids,
        })
    }

    /// Finishes pending AFC state and returns the detected mode.
    pub fn finish(&mut self) -> Result<Mode, DemodulatorError> {
        if self.finished {
            return Err(DemodulatorError::AlreadyFinished);
        }
        self.front_end.finish_afc()?;
        self.finished = true;
        self.mode.ok_or(DemodulatorError::VisNotDetected)
    }

    /// Returns the detected mode, if available.
    pub const fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// Chooses whether a reception may start without a VIS header, and in
    /// which modes.
    ///
    /// Defaults to [`SyncStart::Disabled`], so a caller that only ever decodes
    /// complete transmissions is unaffected. Changing the scope discards the
    /// interval history, because it was matched against a different candidate
    /// set. It has no effect once a mode has been detected.
    pub fn set_sync_start(&mut self, scope: SyncStart) {
        self.front_end.set_sync_start(scope);
    }

    /// Returns the current smoothed receiver frequency offset.
    pub const fn frequency_offset_hz(&self) -> f64 {
        self.front_end.offset_hz()
    }

    /// Returns the absolute position of the next PCM sample.
    pub const fn next_sample(&self) -> u64 {
        self.next_sample
    }

    /// Returns the configured physical PCM sample rate.
    pub const fn sample_rate_hz(&self) -> u32 {
        self.sample_rate_hz
    }
}

/// Demodulated SSTV data and the mode selected from conventional VIS.
#[derive(Clone, Debug)]
pub struct DemodulatedAudio {
    mode: Mode,
    first_sample: u64,
    frequency_hz: Vec<f32>,
    sync_strength: Vec<f32>,
    sync_detector_delay: SstvDuration,
    frequency_offset_hz: f64,
    fsk_ids: Vec<FskId>,
}

impl DemodulatedAudio {
    /// Returns the mode selected from the parity-inclusive VIS byte.
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Returns the physical sample position immediately after VIS framing.
    pub const fn first_sample(&self) -> u64 {
        self.first_sample
    }

    /// Returns AFC-corrected instantaneous image frequencies.
    pub fn frequency_hz(&self) -> &[f32] {
        &self.frequency_hz
    }

    /// Returns causal normalized horizontal-sync confidence.
    pub fn sync_strength(&self) -> &[f32] {
        &self.sync_strength
    }

    /// Returns synchronization-envelope delay relative to frequency output.
    pub const fn sync_detector_delay(&self) -> SstvDuration {
        self.sync_detector_delay
    }

    /// Returns the final smoothed receiver frequency offset.
    pub const fn frequency_offset_hz(&self) -> f64 {
        self.frequency_offset_hz
    }

    /// Returns the validated station identifiers found in the complete audio.
    pub fn fsk_ids(&self) -> &[FskId] {
        &self.fsk_ids
    }
}

/// Demodulates normalized mono PCM and detects its conventional VIS mode.
pub fn demodulate(
    samples: &[f32],
    sample_rate_hz: u32,
) -> Result<DemodulatedAudio, DemodulatorError> {
    let mut demodulator = Demodulator::new(sample_rate_hz)?;
    let output = demodulator.process(samples)?;
    let mode = demodulator.finish()?;
    Ok(DemodulatedAudio {
        mode,
        first_sample: output.first_sample,
        frequency_hz: output.frequency_hz,
        sync_strength: output.sync_strength,
        sync_detector_delay: sync_detector_delay(mode),
        frequency_offset_hz: demodulator.frequency_offset_hz(),
        fsk_ids: output.fsk_ids,
    })
}
