use rssstv_fskid::{FskId, FskNumber, FskRecord};
use rssstv_sstv::{mode::Mode, time::SstvDuration};

use crate::{
    DemodulatorError,
    frontend::{Detection, FrontEnd},
    sync::SyncStart,
    sync_detector_delay,
};

/// Owned output produced from one incremental PCM input packet.
#[derive(Clone, Debug)]
pub struct DemodulatedChunk {
    detected: Option<(Mode, Detection)>,
    first_sample: u64,
    frequency_hz: Vec<f32>,
    sync_strength: Vec<f32>,
    fsk_ids: Vec<FskId>,
    fsk_numbers: Vec<FskNumber>,
}

impl DemodulatedChunk {
    /// Returns a mode when one was identified in this packet, by a conventional
    /// VIS header or by [`SyncStart`] interval matching.
    ///
    /// With [`Demodulator::set_header_restart`] on, a header arriving during a
    /// reception is reported the same way, and the packet's output samples then
    /// begin after that header rather than at the packet's first sample.
    pub const fn detected_mode(&self) -> Option<Mode> {
        match self.detected {
            Some((mode, _)) => Some(mode),
            None => None,
        }
    }

    /// Returns what identified the mode reported by
    /// [`DemodulatedChunk::detected_mode`].
    pub const fn detection(&self) -> Option<Detection> {
        match self.detected {
            Some((_, detection)) => Some(detection),
            None => None,
        }
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

    /// Returns validated contest numbers completed in this packet.
    pub fn fsk_numbers(&self) -> &[FskNumber] {
        &self.fsk_numbers
    }

    /// Returns the chunk's samples from `offset` on, as the decoder takes them.
    ///
    /// A decoder consumes a chunk in as many calls as it needs, so every
    /// driver holds an offset and re-slices what is left; the arithmetic lives
    /// here so each of them does not carry its own copy.
    pub fn block_from(&self, offset: usize) -> rssstv_sstv::rx::DemodulatedBlock<'_> {
        rssstv_sstv::rx::DemodulatedBlock::new(
            self.first_sample + offset as u64,
            &self.frequency_hz[offset..],
            &self.sync_strength[offset..],
        )
    }
}

/// Stateful incremental SSTV receive front end.
pub struct Demodulator {
    front_end: FrontEnd,
    sample_rate_hz: u32,
    next_sample: u64,
    mode: Option<Mode>,
    header_restart: bool,
    finished: bool,
}

impl Demodulator {
    /// Constructs a receive front end for a physical PCM sample rate.
    pub fn new(sample_rate_hz: u32) -> Result<Self, DemodulatorError> {
        if sample_rate_hz < 6_000 {
            return Err(DemodulatorError::SampleRateTooLow(sample_rate_hz));
        }
        Ok(Self {
            front_end: FrontEnd::new(sample_rate_hz)?,
            sample_rate_hz,
            next_sample: 0,
            mode: None,
            header_restart: false,
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

        let mut detected = None;
        let mut first_sample = self.next_sample;
        let mut frequency_hz = Vec::with_capacity(if self.mode.is_some() {
            samples.len()
        } else {
            0
        });
        let mut sync_strength = Vec::with_capacity(frequency_hz.capacity());
        let mut fsk_ids = Vec::new();
        let mut fsk_numbers = Vec::new();
        for &sample in samples {
            let output = self.front_end.process(f64::from(sample))?;
            self.next_sample += 1;
            match output.fsk_record {
                Some(FskRecord::Id(id)) => fsk_ids.push(id),
                Some(FskRecord::Number(number)) => fsk_numbers.push(number),
                None => {}
            }
            if let Some((mode, detection)) = output.mode
                && (self.mode.is_none() || self.restarts_on(detection))
            {
                self.mode = Some(mode);
                detected = Some((mode, detection));
                // Anything demodulated earlier in this packet belongs to the
                // reception the header just replaced. It is dropped rather
                // than handed on, so the caller's new decoder is not fed the
                // tail of the old picture.
                frequency_hz.clear();
                sync_strength.clear();
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
            detected,
            first_sample: if frequency_hz.is_empty() {
                self.next_sample
            } else {
                first_sample
            },
            frequency_hz,
            sync_strength,
            fsk_ids,
            fsk_numbers,
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

    /// Chooses whether a VIS header may start a reception over.
    ///
    /// A mode is normally identified once and kept, which is what an offline
    /// decode of one transmission wants. A live receiver wants the opposite: a
    /// station that notices a mistake stops and sends again, and its new header
    /// arrives while the abandoned picture is still being decoded. Waiting for
    /// that picture to stop first would consume the header, leaving nothing but
    /// sync spacing to start from. MMSSTV makes the same choice, and has done
    /// so by default since it offered the option at all.
    ///
    /// Only a header restarts. Sync spacing is present throughout every
    /// picture, so matching it again mid-reception would restart continuously.
    pub const fn set_header_restart(&mut self, enabled: bool) {
        self.header_restart = enabled;
    }

    const fn restarts_on(&self, detection: Detection) -> bool {
        self.header_restart && matches!(detection, Detection::Header)
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
    fsk_numbers: Vec<FskNumber>,
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

    /// Returns the validated contest numbers found in the complete audio.
    pub fn fsk_numbers(&self) -> &[FskNumber] {
        &self.fsk_numbers
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
        fsk_numbers: output.fsk_numbers,
    })
}
