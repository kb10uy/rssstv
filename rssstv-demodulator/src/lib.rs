//! MMSSTV-style audio front end for SSTV receive decoding.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::{
    cmp::Reverse,
    collections::VecDeque,
    f64::consts::{PI, TAU},
};

use rssstv_dsp::{
    filter::{Fir, FirDesign, FirKind, IirFilter, IirLowPassDesign, IirResponse, Resonator},
    frequency::ZeroCrossingFrequency,
    transform::HilbertTransformer,
};
use rssstv_fskid::{FskDecoder, FskId, FskTone};
use rssstv_sstv::{
    mode::{Mode, Support},
    time::SstvDuration,
};
use thiserror::Error;

const DETECTORS: [(f64, f64); 5] = [
    (1_080.0, 80.0),
    (1_200.0, 100.0),
    (1_320.0, 80.0),
    (1_900.0, 100.0),
    (2_100.0, 100.0),
];
const FSK_MINIMUM_CONTRAST: f64 = 0.125;
const SYNC_DETECTOR_DELAY_PS: u64 = 6_000_000_000;

/// Measured sync intervals kept for mode matching, as in MMSSTV's `CSYNCINT`.
const INTERVAL_HISTORY: usize = 8;

/// Retained intervals that must agree before a mode is reported.
const REQUIRED_AGREEMENT: usize = 6;

/// Line multiples tested against each candidate period.
///
/// A sync pulse lost to noise leaves a gap a whole number of lines wide, so
/// reception can still begin when some pulses were missed.
const MAX_LINE_MULTIPLE: u64 = 3;

/// Fractional tolerance when matching an interval to a candidate period.
///
/// This has to be tighter than the closest pair of candidates: Martin 2 at two
/// lines is 1.6 % away from Martin 1 at one. It also has to be looser than a
/// receiver's clock error and the jitter in locating a pulse, both of which are
/// under a tenth of that.
const INTERVAL_TOLERANCE: f64 = 0.006;

/// Normalized sync strength at which a pulse is considered present.
///
/// The raster decoder extracts its acquisition pulse runs at the same level.
const PULSE_THRESHOLD: f64 = 0.5;

/// Shortest run accepted as a pulse rather than a noise spike, in seconds.
///
/// The narrowest sync pulse among the supported modes is a little over 4 ms.
const MIN_PULSE_SECONDS: f64 = 0.002;

/// Interval below which a second pulse is treated as part of the first.
///
/// The shortest candidate line period is Robot 36's 150 ms, so this is well
/// clear of any real interval while still merging a pulse that wobbles across
/// the threshold.
const MIN_INTERVAL_SECONDS: f64 = 0.05;

/// Which modes a reception may be started in without a VIS header.
///
/// A transmission that was joined after its header, or that never sent one,
/// can only be identified by its raster: the spacing of horizontal sync pulses
/// is the mode's line period. Detection from that spacing is deliberately
/// scoped by the caller, because a period can be matched by more than one mode
/// and an operator who has chosen a mode should not be overridden by a guess.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SyncStart {
    /// Only a VIS header starts a reception.
    #[default]
    Disabled,
    /// Any decode-supported mode whose line period matches.
    Any,
    /// This mode alone, and only when its line period matches.
    Only(Mode),
}

/// Failure while configuring or processing the receive front end.
#[derive(Debug, Error)]
pub enum DemodulatorError {
    /// The physical sample rate cannot represent the SSTV receive band.
    #[error("sample rate {0} Hz is too low for SSTV")]
    SampleRateTooLow(u32),
    /// A PCM sample was not finite.
    #[error("PCM sample {index} is not finite")]
    NonFiniteSample {
        /// Zero-based absolute PCM sample position.
        index: u64,
    },
    /// Processing was requested after the stream was finished.
    #[error("demodulator stream is already finished")]
    AlreadyFinished,
    /// The absolute PCM sample position overflowed.
    #[error("PCM sample position overflow")]
    SamplePositionOverflow,
    /// A DSP processor rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] rssstv_dsp::DspError),
    /// No complete supported conventional VIS sequence was found.
    #[error("no supported conventional VIS code was detected")]
    VisNotDetected,
}

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
        self.front_end.afc.offset_hz
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

/// Ordering key for a candidate: agreement first, then the smallest multiple,
/// then the shortest period.
type MatchRank = (usize, Reverse<u64>, Reverse<u64>);

/// Mode detection from the spacing of horizontal sync pulses.
///
/// This is MMSSTV's `CSYNCINT`. It keeps a short history of measured intervals
/// and compares them against every candidate's line period at one-, two-, and
/// three-line multiples, so a reception can begin despite missed sync pulses or
/// a missed VIS header.
///
/// Only the spacing of pulses matters, so each is located by the start of its
/// run: the measurement is differential and a consistent reference point
/// cancels out.
struct SyncIntervalDetector {
    scope: SyncStart,
    min_pulse_samples: u64,
    min_interval_samples: u64,
    /// Length of the run currently above the threshold, zero when below it.
    run: u64,
    /// Whether the current run has already been accepted as a pulse.
    counted: bool,
    /// Samples since the accepted pulse that opened the current interval.
    since_pulse: u64,
    /// Whether any pulse has been accepted, which opens the first interval.
    started: bool,
    intervals: [u64; INTERVAL_HISTORY],
    filled: usize,
    next: usize,
}

impl SyncIntervalDetector {
    fn new(sample_rate_hz: f64) -> Self {
        Self {
            scope: SyncStart::default(),
            min_pulse_samples: (sample_rate_hz * MIN_PULSE_SECONDS) as u64,
            min_interval_samples: (sample_rate_hz * MIN_INTERVAL_SECONDS) as u64,
            run: 0,
            counted: false,
            since_pulse: 0,
            started: false,
            intervals: [0; INTERVAL_HISTORY],
            filled: 0,
            next: 0,
        }
    }

    fn set_scope(&mut self, scope: SyncStart) {
        if self.scope != scope {
            self.scope = scope;
            self.filled = 0;
            self.next = 0;
            self.started = false;
        }
    }

    /// Feeds one sync strength and reports a mode once the history agrees.
    fn process(&mut self, sample_rate_hz: f64, sync_strength: f64) -> Option<Mode> {
        if self.scope == SyncStart::Disabled {
            return None;
        }
        self.since_pulse = self.since_pulse.saturating_add(1);
        if sync_strength >= PULSE_THRESHOLD {
            self.run += 1;
            if !self.counted && self.run >= self.min_pulse_samples {
                self.counted = true;
                // The pulse is placed at the start of its run, which is where
                // the threshold was crossed `run` samples ago.
                return self.accept_pulse(sample_rate_hz);
            }
        } else {
            self.run = 0;
            self.counted = false;
        }
        None
    }

    fn accept_pulse(&mut self, sample_rate_hz: f64) -> Option<Mode> {
        let interval = self.since_pulse.saturating_sub(self.run);
        self.since_pulse = self.run;
        if !self.started {
            self.started = true;
            return None;
        }
        if interval < self.min_interval_samples {
            return None;
        }
        self.intervals[self.next] = interval;
        self.next = (self.next + 1) % INTERVAL_HISTORY;
        self.filled = (self.filled + 1).min(INTERVAL_HISTORY);
        if self.filled < REQUIRED_AGREEMENT {
            return None;
        }
        self.resolve(sample_rate_hz)
    }

    /// Returns the candidate whose period best explains the retained intervals.
    ///
    /// Ties are broken towards the smallest line multiple and then the shortest
    /// period, because the multiples exist to tolerate pulses that were missed:
    /// a period that explains the spacing directly is the stronger claim than
    /// one that has to assume a gap.
    fn resolve(&self, sample_rate_hz: f64) -> Option<Mode> {
        let mut best: Option<(MatchRank, Mode)> = None;
        for mode in candidates(self.scope) {
            let period = mode.spec().period().as_picos();
            let period_samples = period as f64 * sample_rate_hz / 1.0e12;
            for multiple in 1..=MAX_LINE_MULTIPLE {
                let expected = period_samples * multiple as f64;
                let tolerance = expected * INTERVAL_TOLERANCE;
                let agreed = self.intervals[..self.filled]
                    .iter()
                    .filter(|&&interval| (interval as f64 - expected).abs() <= tolerance)
                    .count();
                if agreed < REQUIRED_AGREEMENT {
                    continue;
                }
                let rank = (agreed, Reverse(multiple), Reverse(period));
                if best.is_none_or(|(best_rank, _)| rank > best_rank) {
                    best = Some((rank, mode));
                }
            }
        }
        best.map(|(_, mode)| mode)
    }
}

/// Returns the modes a sync-interval match may report.
fn candidates(scope: SyncStart) -> impl Iterator<Item = Mode> {
    let only = match scope {
        SyncStart::Only(mode) => Some(mode),
        _ => None,
    };
    Mode::ALL.into_iter().filter(move |mode| {
        mode.spec().decode_support() == Support::Supported
            && only.is_none_or(|selected| *mode == selected)
    })
}

struct FrontEndOutput {
    frequency_hz: f64,
    sync_strength: f64,
    mode: Option<Mode>,
    fsk_id: Option<FskId>,
}

struct FrontEnd {
    previous_input: f64,
    band_pass: Fir,
    hilbert: HilbertDiscriminator,
    zero_crossing: ZeroCrossingFrequency,
    level_peak: f64,
    level_decay: f64,
    detectors: Vec<ToneDetector>,
    vis: VisDecoder,
    sync_intervals: SyncIntervalDetector,
    sample_rate_hz: f64,
    fsk: FskDecoder,
    afc: Afc,
}

impl FrontEnd {
    fn new(sample_rate_hz: f64) -> Result<Self, DemodulatorError> {
        let mut order = (24.0 * sample_rate_hz / 11_025.0).round() as usize;
        order = order.max(12);
        if !order.is_multiple_of(2) {
            order += 1;
        }
        let upper = 2_600.0_f64.min(sample_rate_hz * 0.5 - 100.0);
        let band_pass = Fir::from_design(FirDesign {
            kind: FirKind::BandPass,
            order,
            sample_rate_hz,
            lower_frequency_hz: 1_000.0,
            upper_frequency_hz: upper,
            attenuation_db: 20.0,
            gain: 1.0,
        })?;
        let detectors = DETECTORS
            .into_iter()
            .map(|(frequency, bandwidth)| ToneDetector::new(frequency, bandwidth, sample_rate_hz))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            previous_input: 0.0,
            band_pass,
            hilbert: HilbertDiscriminator::new(sample_rate_hz)?,
            zero_crossing: ZeroCrossingFrequency::new(sample_rate_hz)?,
            level_peak: 1.0e-6,
            level_decay: (-1.0 / (sample_rate_hz * 0.1)).exp(),
            detectors,
            vis: VisDecoder::new(sample_rate_hz),
            sync_intervals: SyncIntervalDetector::new(sample_rate_hz),
            sample_rate_hz,
            fsk: FskDecoder::new(sample_rate_hz as u32),
            afc: Afc::new(sample_rate_hz),
        })
    }

    fn process(&mut self, input: f64) -> Result<FrontEndOutput, DemodulatorError> {
        let averaged = (input + self.previous_input) * 0.5;
        self.previous_input = input;
        let filtered = self.band_pass.process_sample(averaged);

        self.level_peak = (self.level_peak * self.level_decay).max(filtered.abs());
        let detector_input = (filtered / self.level_peak.max(1.0e-6)).clamp(-1.0, 1.0);
        let mut envelopes = [0.0; 5];
        for (envelope, detector) in envelopes.iter_mut().zip(&mut self.detectors) {
            *envelope = detector.process(detector_input);
        }
        let competing = envelopes[0]
            .max(envelopes[2])
            .max(envelopes[3])
            .max(envelopes[4])
            .max(1.0e-6);
        let sync_strength = (envelopes[1] / (envelopes[1] + competing)).clamp(0.0, 1.0);
        // A header identifies the mode outright, so it is preferred over an
        // inference drawn from the raster's timing.
        let mode = self.vis.process(envelopes).or_else(|| {
            self.sync_intervals
                .process(self.sample_rate_hz, sync_strength)
        });
        let difference = (envelopes[3] - envelopes[4]).abs();
        let fsk_tone = if difference < FSK_MINIMUM_CONTRAST {
            FskTone::Ambiguous
        } else if envelopes[3] > envelopes[4] {
            FskTone::Mark
        } else {
            FskTone::Space
        };
        let fsk_id = self.fsk.process(fsk_tone);

        let measured = self.zero_crossing.process_sample(filtered);
        let changed = self.afc.process(sync_strength, measured);
        if changed {
            for (detector, (nominal, _)) in self.detectors.iter_mut().zip(DETECTORS) {
                detector.retune(nominal + self.afc.offset_hz)?;
            }
        }

        let frequency = self.hilbert.process(filtered);
        Ok(FrontEndOutput {
            frequency_hz: (frequency - self.afc.offset_hz).clamp(0.0, 3_000.0),
            sync_strength,
            mode,
            fsk_id,
        })
    }

    fn enable_afc(&mut self) {
        self.afc.enabled = true;
    }

    fn set_sync_start(&mut self, scope: SyncStart) {
        self.sync_intervals.set_scope(scope);
    }

    fn finish_afc(&mut self) -> Result<(), DemodulatorError> {
        if self.afc.finish_run() {
            for (detector, (nominal, _)) in self.detectors.iter_mut().zip(DETECTORS) {
                detector.retune(nominal + self.afc.offset_hz)?;
            }
        }
        Ok(())
    }
}

struct ToneDetector {
    resonator: Resonator,
    envelope: IirFilter,
}

impl ToneDetector {
    fn new(
        frequency_hz: f64,
        bandwidth_hz: f64,
        sample_rate_hz: f64,
    ) -> Result<Self, rssstv_dsp::DspError> {
        Ok(Self {
            resonator: Resonator::new(frequency_hz, sample_rate_hz, bandwidth_hz)?,
            envelope: IirFilter::from_low_pass(IirLowPassDesign {
                order: 2,
                sample_rate_hz,
                cutoff_hz: 50.0,
                response: IirResponse::Butterworth,
            })?,
        })
    }

    fn process(&mut self, sample: f64) -> f64 {
        self.envelope
            .process_sample(self.resonator.process_sample(sample).abs())
            .max(0.0)
    }

    fn retune(&mut self, frequency_hz: f64) -> Result<(), rssstv_dsp::DspError> {
        self.resonator.set_frequency(frequency_hz)
    }
}

struct HilbertDiscriminator {
    transformer: HilbertTransformer,
    sample_rate_hz: f64,
    phase_history: [f64; 4],
    phase_history_len: usize,
    next_phase: usize,
    phase_lag: usize,
    output_filter: IirFilter,
    held_frequency: f64,
}

impl HilbertDiscriminator {
    fn new(sample_rate_hz: f64) -> Result<Self, rssstv_dsp::DspError> {
        let (order, phase_lag) = if sample_rate_hz < 16_000.0 {
            (12, 1)
        } else if sample_rate_hz < 40_000.0 {
            (24, 2)
        } else {
            (48, 4)
        };
        let upper_frequency_hz = sample_rate_hz * 0.5 - 100.0;
        Ok(Self {
            transformer: HilbertTransformer::new(order, sample_rate_hz, 100.0, upper_frequency_hz)?,
            sample_rate_hz,
            phase_history: [0.0; 4],
            phase_history_len: 0,
            next_phase: 0,
            phase_lag,
            output_filter: IirFilter::from_low_pass(IirLowPassDesign {
                order: 3,
                sample_rate_hz,
                cutoff_hz: 1_800.0_f64.min(sample_rate_hz * 0.45),
                response: IirResponse::Butterworth,
            })?,
            held_frequency: 1_900.0,
        })
    }

    fn process(&mut self, sample: f64) -> f64 {
        let analytic = self.transformer.process_sample(sample);
        let magnitude = analytic.in_phase.hypot(analytic.quadrature);
        let phase = analytic.quadrature.atan2(analytic.in_phase);
        let previous = (self.phase_history_len == self.phase_lag)
            .then_some(self.phase_history[self.next_phase]);
        self.phase_history[self.next_phase] = phase;
        self.next_phase = (self.next_phase + 1) % self.phase_lag;
        self.phase_history_len = (self.phase_history_len + 1).min(self.phase_lag);
        if let Some(previous) = previous
            && magnitude > 1.0e-8
        {
            let delta = (phase - previous + PI).rem_euclid(TAU) - PI;
            self.held_frequency = (delta.abs() * self.sample_rate_hz
                / (TAU * self.phase_lag as f64))
                .clamp(0.0, 3_000.0);
        }
        self.output_filter.process_sample(self.held_frequency)
    }
}

struct Afc {
    enabled: bool,
    sample_rate_hz: f64,
    active: bool,
    run_samples: usize,
    measurements: Vec<f64>,
    offsets: VecDeque<f64>,
    offset_hz: f64,
    inhibit_samples: usize,
}

impl Afc {
    fn new(sample_rate_hz: f64) -> Self {
        Self {
            enabled: false,
            sample_rate_hz,
            active: false,
            run_samples: 0,
            measurements: Vec::new(),
            offsets: VecDeque::with_capacity(15),
            offset_hz: 0.0,
            inhibit_samples: 0,
        }
    }

    fn process(&mut self, sync_strength: f64, measurement: Option<f64>) -> bool {
        if !self.enabled {
            return false;
        }
        if self.inhibit_samples > 0 {
            self.inhibit_samples -= 1;
        }
        if sync_strength >= 0.58 {
            self.active = true;
            self.run_samples += 1;
            let expected = 1_200.0 + self.offset_hz;
            if let Some(value) = measurement.filter(|value| (value - expected).abs() <= 100.0) {
                self.measurements.push(value);
            }
            false
        } else if self.active {
            self.finish_run()
        } else {
            false
        }
    }

    fn finish_run(&mut self) -> bool {
        if !self.active {
            return false;
        }
        self.active = false;
        let duration = self.run_samples as f64 / self.sample_rate_hz;
        self.run_samples = 0;
        if self.inhibit_samples > 0
            || !(0.003..=0.050).contains(&duration)
            || self.measurements.len() < 2
        {
            self.measurements.clear();
            return false;
        }
        self.measurements.sort_by(f64::total_cmp);
        let measured = self.measurements.iter().sum::<f64>() / self.measurements.len() as f64;
        self.measurements.clear();
        let offset = measured - 1_200.0;
        if offset.abs() > 150.0 {
            return false;
        }
        if self.offsets.len() == 15 {
            self.offsets.pop_front();
        }
        self.offsets.push_back(offset);
        self.offset_hz = self.offsets.iter().sum::<f64>() / 15.0;
        self.inhibit_samples = (self.sample_rate_hz * 0.1) as usize;
        true
    }
}

#[derive(Clone, Copy)]
enum VisState {
    Search,
    FirstLeader { samples: usize },
    Break { samples: usize },
    SecondLeader { samples: usize },
    Cells { sample: usize, cell: usize },
}

struct VisDecoder {
    state: VisState,
    sample_rate_hz: f64,
    cell_sums: [[f64; 3]; 10],
}

impl VisDecoder {
    fn new(sample_rate_hz: f64) -> Self {
        Self {
            state: VisState::Search,
            sample_rate_hz,
            cell_sums: [[0.0; 3]; 10],
        }
    }

    fn process(&mut self, envelope: [f64; 5]) -> Option<Mode> {
        let dominant_1900 = envelope[3] > envelope[1]
            && envelope[3] > envelope[0]
            && envelope[3] > envelope[2]
            && envelope[3] > envelope[4];
        let dominant_1200 = envelope[1] > envelope[3]
            && envelope[1] > envelope[0]
            && envelope[1] > envelope[2]
            && envelope[1] > envelope[4];
        let leader_min = (self.sample_rate_hz * 0.22) as usize;
        let break_min = (self.sample_rate_hz * 0.004) as usize;
        let break_max = (self.sample_rate_hz * 0.030) as usize;
        self.state = match self.state {
            VisState::Search if dominant_1900 => VisState::FirstLeader { samples: 1 },
            VisState::Search => VisState::Search,
            VisState::FirstLeader { samples } if dominant_1900 => VisState::FirstLeader {
                samples: samples + 1,
            },
            VisState::FirstLeader { samples } if dominant_1200 && samples >= leader_min => {
                VisState::Break { samples: 1 }
            }
            VisState::FirstLeader { .. } => VisState::Search,
            VisState::Break { samples } if dominant_1200 && samples < break_max => {
                VisState::Break {
                    samples: samples + 1,
                }
            }
            VisState::Break { samples } if dominant_1900 && samples >= break_min => {
                VisState::SecondLeader { samples: 1 }
            }
            VisState::Break { .. } => VisState::Search,
            VisState::SecondLeader { samples } if dominant_1900 => VisState::SecondLeader {
                samples: samples + 1,
            },
            VisState::SecondLeader { samples } if dominant_1200 && samples >= leader_min => {
                self.cell_sums = [[0.0; 3]; 10];
                VisState::Cells { sample: 0, cell: 0 }
            }
            VisState::SecondLeader { .. } => VisState::Search,
            VisState::Cells { sample, cell } => {
                let cell_samples = (self.sample_rate_hz * 0.030).round() as usize;
                let within = sample % cell_samples;
                if within >= cell_samples / 4 && within < cell_samples * 3 / 4 {
                    self.cell_sums[cell][0] += envelope[0];
                    self.cell_sums[cell][1] += envelope[1];
                    self.cell_sums[cell][2] += envelope[2];
                }
                let next_sample = sample + 1;
                if next_sample == cell_samples {
                    if cell == 9 {
                        let mode = self.finish_cells();
                        self.state = VisState::Search;
                        return mode;
                    }
                    VisState::Cells {
                        sample: 0,
                        cell: cell + 1,
                    }
                } else {
                    VisState::Cells {
                        sample: next_sample,
                        cell,
                    }
                }
            }
        };
        None
    }

    fn finish_cells(&self) -> Option<Mode> {
        if self.cell_sums[0][1] <= self.cell_sums[0][0].max(self.cell_sums[0][2])
            || self.cell_sums[9][1] <= self.cell_sums[9][0].max(self.cell_sums[9][2])
        {
            return None;
        }
        let mut raw = 0_u8;
        for bit in 0..8 {
            let sums = self.cell_sums[bit + 1];
            if sums[0] > sums[2] {
                raw |= 1 << bit;
            }
        }
        Mode::from_raw_vis(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rssstv_sstv::{
        RxDecoder, TxEncoder,
        image::{ImageSize, Rgb8, RgbImage},
        rx::{DemodulatedBlock, RxConfig},
    };
    use rstest::rstest;

    /// Measured delay of the band-pass and discriminator chain, in milliseconds.
    const GROUP_DELAY_MS: f64 = 2.05;

    fn tone(samples: &mut Vec<f32>, rate: u32, frequency: f64, seconds: f64, phase: &mut f64) {
        for _ in 0..(rate as f64 * seconds).round() as usize {
            samples.push((*phase).sin() as f32 * 0.8);
            *phase = (*phase + TAU * frequency / rate as f64).rem_euclid(TAU);
        }
    }

    fn vis_signal(mode: Mode, rate: u32, offset: f64) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut phase = 0.0_f64;
        tone(&mut samples, rate, 1_900.0 + offset, 0.3, &mut phase);
        tone(&mut samples, rate, 1_200.0 + offset, 0.01, &mut phase);
        tone(&mut samples, rate, 1_900.0 + offset, 0.3, &mut phase);
        tone(&mut samples, rate, 1_200.0 + offset, 0.03, &mut phase);
        let raw = mode.spec().raw_vis().unwrap();
        for bit in 0..8 {
            let frequency = if raw & (1 << bit) == 0 {
                1_300.0
            } else {
                1_100.0
            };
            tone(&mut samples, rate, frequency + offset, 0.03, &mut phase);
        }
        tone(&mut samples, rate, 1_200.0 + offset, 0.03, &mut phase);
        tone(&mut samples, rate, 1_900.0 + offset, 0.1, &mut phase);
        samples
    }

    fn fsk_id_signal(samples: &mut Vec<f32>, rate: u32, phase: &mut f64) {
        const SYMBOLS: [u8; 9] = [0x2a, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25];
        tone(samples, rate, 2_100.0, 0.1, phase);
        tone(samples, rate, 1_900.0, 0.022, phase);
        for symbol in SYMBOLS {
            for bit in 0..6 {
                let frequency = if symbol & (1 << bit) == 0 {
                    2_100.0
                } else {
                    1_900.0
                };
                tone(samples, rate, frequency, 0.022, phase);
            }
        }
        tone(samples, rate, 2_100.0, 0.1, phase);
    }

    #[rstest]
    #[case(8_000, 1)]
    #[case(11_025, 1)]
    #[case(16_000, 2)]
    #[case(22_050, 2)]
    #[case(40_000, 4)]
    #[case(48_000, 4)]
    fn hilbert_phase_lag_tracks_sample_rate(#[case] rate: u32, #[case] expected: usize) {
        let discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        assert_eq!(discriminator.phase_lag, expected);
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(16_000)]
    #[case(22_050)]
    #[case(40_000)]
    #[case(48_000)]
    fn hilbert_phase_lag_preserves_frequency_scale(#[case] rate: u32) {
        let expected = 1_900.0;
        let mut discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        for sample in 0..rate / 5 {
            let estimate = discriminator.process(phase.sin());
            phase = (phase + TAU * expected / f64::from(rate)).rem_euclid(TAU);
            if sample >= rate / 10 {
                sum += estimate;
            }
        }
        let estimate = sum / f64::from(rate / 10);
        assert!(
            (estimate - expected).abs() < 2.0,
            "{rate} Hz produced {estimate} Hz"
        );
    }

    #[rstest]
    #[case(1_500.0)]
    #[case(1_550.0)]
    #[case(1_900.0)]
    #[case(2_300.0)]
    fn hilbert_image_tones_have_low_residual_ripple(#[case] frequency: f64) {
        let rate = 48_000;
        let mut discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        let mut squared = 0.0;
        let mut count = 0.0;
        for sample in 0..rate / 2 {
            let estimate = discriminator.process(phase.sin());
            phase = (phase + TAU * frequency / f64::from(rate)).rem_euclid(TAU);
            if sample >= rate / 4 {
                sum += estimate;
                squared += estimate * estimate;
                count += 1.0;
            }
        }
        let mean = sum / count;
        let standard_deviation = (squared / count - mean * mean).sqrt();
        assert!(
            standard_deviation < 6.0,
            "{frequency} Hz residual ripple was {standard_deviation} Hz"
        );
    }

    /// Transmits a vertical edge and checks where the decoded raster puts it.
    ///
    /// This is the end-to-end statement of horizontal alignment: the modulated
    /// audio, the demodulator's own group delay, and the decoder's raster phase
    /// all have to agree before a picture stops sliding off one side.
    #[rstest]
    #[case(Mode::Martin1, 48_000)]
    #[case(Mode::Martin2, 48_000)]
    #[case(Mode::Scottie2, 48_000)]
    #[case(Mode::Robot36, 48_000)]
    #[case(Mode::Robot72, 48_000)]
    #[case(Mode::Pd50, 48_000)]
    #[case(Mode::Martin2, 8_000)]
    #[case(Mode::Scottie2, 8_000)]
    #[case(Mode::Robot36, 8_000)]
    #[case(Mode::Pd50, 8_000)]
    fn a_transmitted_edge_decodes_where_it_was_sent(#[case] mode: Mode, #[case] rate: u32) {
        let width = mode.spec().width() as usize;
        let size = ImageSize::new(width, mode.spec().height() as usize).unwrap();
        let mut image = RgbImage::new(size, Rgb8::new(0, 0, 0));
        for row in 0..size.height() {
            for x in width / 2..width {
                if let Some(pixel) = image.row_mut(row).and_then(|row| row.get_mut(x)) {
                    *pixel = Rgb8::new(255, 255, 255);
                }
            }
        }
        let scan = mode.scan();
        let leading_ps = scan
            .leading()
            .iter()
            .map(|segment| segment.duration().as_picos())
            .sum::<u64>();
        let stop_ps = 910_000_000_000 + leading_ps + mode.spec().period().as_picos() * 14;
        let stop_sample = u128::from(stop_ps) * u128::from(rate) / 1_000_000_000_000;
        let mut samples = Vec::new();
        let mut phase = 0.0_f64;
        let mut written = 0_u64;
        for timed in TxEncoder::new(mode, image).unwrap() {
            let deadline = timed.until().as_picos() * u64::from(rate) / 1_000_000_000_000;
            while written < deadline && u128::from(written) < stop_sample {
                samples.push((phase.sin() * 0.8) as f32);
                phase = (phase + TAU * f64::from(timed.frequency().as_hz()) / f64::from(rate))
                    .rem_euclid(TAU);
                written += 1;
            }
            if u128::from(written) >= stop_sample {
                break;
            }
        }
        let output = demodulate(&samples, rate).unwrap();
        let mut decoder = RxDecoder::with_config(
            mode,
            rate,
            RxConfig {
                sync_detector_delay: output.sync_detector_delay(),
                ..RxConfig::default()
            },
        )
        .unwrap();
        let mut offset = 0;
        while let Ok(processed) = decoder.process(DemodulatedBlock::new(
            output.first_sample() + offset as u64,
            &output.frequency_hz()[offset..],
            &output.sync_strength()[offset..],
        )) {
            offset += processed.consumed();
            if processed.consumed() == 0 && processed.event().is_none() {
                break;
            }
        }
        let rows = mode.spec().rows_per_raster_unit() as usize;
        let row = decoder.image().row(rows * 6).unwrap();
        let edge = (0..width).find(|x| row[*x].g > 128);
        assert!(
            edge.is_some_and(|edge| edge.abs_diff(width / 2) <= 1),
            "{mode:?} @{rate}: the edge decoded at {edge:?} instead of {}",
            width / 2,
        );
    }

    fn raster_epoch_error_ms(mode: Mode, rate: u32) -> f64 {
        let size =
            ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize).unwrap();
        let image = RgbImage::new(size, Rgb8::new(128, 128, 128));
        let scan = mode.scan();
        let leading_ps = scan
            .leading()
            .iter()
            .map(|segment| segment.duration().as_picos())
            .sum::<u64>();
        let epoch_ps = 910_000_000_000 + leading_ps;
        let stop_ps = epoch_ps + mode.spec().period().as_picos() * 40;
        let stop_sample = u128::from(stop_ps) * u128::from(rate) / 1_000_000_000_000;
        let mut samples = Vec::new();
        let mut phase = 0.0_f64;
        let mut written = 0_u64;
        for timed in TxEncoder::new(mode, image).unwrap() {
            let deadline = timed.until().as_picos() * u64::from(rate) / 1_000_000_000_000;
            while written < deadline && u128::from(written) < stop_sample {
                samples.push((phase.sin() * 0.8) as f32);
                phase = (phase + TAU * f64::from(timed.frequency().as_hz()) / f64::from(rate))
                    .rem_euclid(TAU);
                written += 1;
            }
            if u128::from(written) >= stop_sample {
                break;
            }
        }

        let output = demodulate(&samples, rate).unwrap();
        let mut decoder = RxDecoder::with_config(
            mode,
            rate,
            RxConfig {
                sync_detector_delay: output.sync_detector_delay(),
                ..RxConfig::default()
            },
        )
        .unwrap();
        let mut offset = 0;
        while offset < output.frequency_hz().len() && decoder.source_epoch().is_none() {
            let processed = decoder
                .process(DemodulatedBlock::new(
                    output.first_sample() + offset as u64,
                    &output.frequency_hz()[offset..],
                    &output.sync_strength()[offset..],
                ))
                .unwrap();
            offset += processed.consumed();
        }
        let actual = decoder.source_epoch().unwrap() as f64;
        let expected = epoch_ps as f64 * f64::from(rate) / 1.0e12;
        let period = mode.spec().period().as_picos() as f64 * f64::from(rate) / 1.0e12;
        let mut error = (actual - expected).rem_euclid(period);
        if error > period / 2.0 {
            error -= period;
        }
        error * 1_000.0 / f64::from(rate)
    }

    #[test]
    fn detects_parity_inclusive_vis() {
        let samples = vis_signal(Mode::Scottie2, 8_000, 0.0);
        let output = demodulate(&samples, 8_000).unwrap();
        assert_eq!(output.mode(), Mode::Scottie2);
    }

    #[test]
    fn synchronization_envelope_is_causal_at_end_of_input() {
        let rate = 8_000;
        let mut samples = vis_signal(Mode::Robot36, rate, 0.0);
        let mut phase = 0.0;
        tone(&mut samples, rate, 1_200.0, 0.05, &mut phase);
        let output = demodulate(&samples, rate).unwrap();
        assert!(output.sync_strength().last().copied().unwrap() > 0.5);
    }

    #[test]
    fn detector_delay_is_calibrated_only_for_supported_raster_families() {
        assert_eq!(
            sync_detector_delay(Mode::Martin2).as_picos(),
            SYNC_DETECTOR_DELAY_PS
        );
        assert_eq!(
            sync_detector_delay(Mode::Scottie2).as_picos(),
            SYNC_DETECTOR_DELAY_PS
        );
        assert_eq!(
            sync_detector_delay(Mode::Robot36).as_picos(),
            SYNC_DETECTOR_DELAY_PS
        );
        assert_eq!(
            sync_detector_delay(Mode::Pd50).as_picos(),
            SYNC_DETECTOR_DELAY_PS
        );
        assert_eq!(sync_detector_delay(Mode::Avt90), SstvDuration::ZERO);
    }

    #[test]
    fn tracks_repeated_offset_sync_pulses() {
        let rate = 8_000.0;
        let mut afc = Afc::new(rate);
        afc.enabled = true;
        for _ in 0..24 {
            for sample in 0..(rate * 0.009) as usize {
                afc.process(0.8, (sample % 3 == 0).then_some(1_240.0));
            }
            for _ in 0..(rate * 0.11) as usize {
                afc.process(0.1, None);
            }
        }
        assert!(
            (afc.offset_hz - 40.0).abs() < 1.0,
            "offset was {} Hz",
            afc.offset_hz
        );
    }

    #[test]
    fn detects_trailing_jl1his_fskid() {
        let rate = 8_000;
        let mut samples = vis_signal(Mode::Scottie2, rate, 0.0);
        let mut phase = 0.0;
        fsk_id_signal(&mut samples, rate, &mut phase);

        let output = demodulate(&samples, rate).unwrap();

        assert_eq!(output.fsk_ids().len(), 1);
        assert_eq!(output.fsk_ids()[0].as_str(), "JL1HIS");
    }

    #[rstest]
    #[case(1)]
    #[case(73)]
    #[case(1_024)]
    fn incremental_packets_match_batch_output(#[case] packet_size: usize) {
        let rate = 8_000;
        let mut samples = vis_signal(Mode::Scottie2, rate, 0.0);
        let mut phase = 0.0;
        fsk_id_signal(&mut samples, rate, &mut phase);
        let expected = demodulate(&samples, rate).unwrap();
        let mut demodulator = Demodulator::new(rate).unwrap();
        let mut detected = Vec::new();
        let mut first_sample = None;
        let mut frequency_hz = Vec::new();
        let mut sync_strength = Vec::new();
        let mut fsk_ids = Vec::new();
        for packet in samples.chunks(packet_size) {
            let output = demodulator.process(packet).unwrap();
            detected.extend(output.detected_mode());
            if !output.frequency_hz().is_empty() {
                first_sample.get_or_insert(output.first_sample());
            }
            frequency_hz.extend_from_slice(output.frequency_hz());
            sync_strength.extend_from_slice(output.sync_strength());
            fsk_ids.extend_from_slice(output.fsk_ids());
        }
        assert_eq!(demodulator.finish().unwrap(), expected.mode());
        assert_eq!(detected, [expected.mode()]);
        assert_eq!(first_sample, Some(expected.first_sample()));
        assert_eq!(frequency_hz, expected.frequency_hz());
        assert_eq!(sync_strength, expected.sync_strength());
        assert_eq!(fsk_ids, expected.fsk_ids());
        assert_eq!(
            demodulator.process(&[]).unwrap_err().to_string(),
            DemodulatorError::AlreadyFinished.to_string()
        );
    }

    #[test]
    fn incremental_error_reports_absolute_sample_position_without_consuming_packet() {
        let mut demodulator = Demodulator::new(8_000).unwrap();
        demodulator.process(&[0.0; 10]).unwrap();
        assert!(matches!(
            demodulator.process(&[0.0, f32::NAN]),
            Err(DemodulatorError::NonFiniteSample { index: 11 })
        ));
        assert_eq!(demodulator.next_sample(), 10);
    }

    /// The acquired epoch trails the protocol raster by the demodulator's own
    /// group delay, which is the delay the picture samples carry as well. That
    /// figure has to be the same for every mode and rate: anything mode-specific
    /// left in it would displace that mode's picture horizontally.
    #[rstest]
    #[case(Mode::Martin2, 8_000)]
    #[case(Mode::Martin2, 48_000)]
    #[case(Mode::Scottie2, 8_000)]
    #[case(Mode::Robot36, 8_000)]
    #[case(Mode::Robot36, 48_000)]
    #[case(Mode::Pd50, 8_000)]
    #[case(Mode::Pd50, 48_000)]
    fn aligns_causal_sync_with_frequency_output(#[case] mode: Mode, #[case] rate: u32) {
        let offset_ms = raster_epoch_error_ms(mode, rate);
        assert!(
            (offset_ms - GROUP_DELAY_MS).abs() <= 0.4,
            "raster epoch offset was {offset_ms} ms"
        );
    }

    #[test]
    fn rejects_low_sample_rate() {
        assert!(matches!(
            demodulate(&[], 4_000),
            Err(DemodulatorError::SampleRateTooLow(4_000))
        ));
    }

    /// Drives the interval detector with a pulse train whose gaps are given in
    /// seconds, and returns the first mode it reports.
    ///
    /// The train opens with a pulse, so `gaps` closes one interval each and
    /// [`REQUIRED_AGREEMENT`] of them are needed before anything is reported.
    fn detect_from_gaps(scope: SyncStart, rate: f64, gaps: &[f64]) -> Option<Mode> {
        let mut detector = SyncIntervalDetector::new(rate);
        detector.set_scope(scope);
        let pulse_samples = (rate * 0.006).round() as usize;
        let mut detected = None;
        let feed = |detector: &mut SyncIntervalDetector,
                    samples: usize,
                    strength: f64,
                    detected: &mut Option<Mode>| {
            for _ in 0..samples {
                if let Some(mode) = detector.process(rate, strength) {
                    detected.get_or_insert(mode);
                }
            }
        };
        feed(&mut detector, pulse_samples, 1.0, &mut detected);
        for gap in gaps {
            let quiet = (rate * gap).round() as usize - pulse_samples;
            feed(&mut detector, quiet, 0.0, &mut detected);
            feed(&mut detector, pulse_samples, 1.0, &mut detected);
        }
        detected
    }

    fn period_seconds(mode: Mode) -> f64 {
        mode.spec().period().as_picos() as f64 / 1.0e12
    }

    /// A transmission joined after its header has nothing but its raster to
    /// identify it, and the spacing of sync pulses is the mode's line period.
    #[rstest]
    #[case(Mode::Robot36)]
    #[case(Mode::Martin1)]
    #[case(Mode::Martin2)]
    #[case(Mode::Scottie1)]
    #[case(Mode::Pd120)]
    #[case(Mode::Pd290)]
    fn a_line_rate_pulse_train_identifies_its_mode(#[case] mode: Mode) {
        let gaps = vec![period_seconds(mode); INTERVAL_HISTORY];
        assert_eq!(detect_from_gaps(SyncStart::Any, 8_000.0, &gaps), Some(mode));
    }

    #[test]
    fn no_mode_is_reported_while_sync_start_is_disabled() {
        let gaps = vec![period_seconds(Mode::Martin1); INTERVAL_HISTORY];
        assert_eq!(detect_from_gaps(SyncStart::Disabled, 8_000.0, &gaps), None);
    }

    /// The operator's choice is confirmed, not overridden: a period that
    /// belongs to some other mode starts nothing.
    #[test]
    fn a_scoped_detector_ignores_a_period_that_is_not_its_own() {
        let gaps = vec![period_seconds(Mode::Martin1); INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Only(Mode::Martin1), 8_000.0, &gaps),
            Some(Mode::Martin1)
        );
        assert_eq!(
            detect_from_gaps(SyncStart::Only(Mode::Scottie1), 8_000.0, &gaps),
            None
        );
    }

    /// Pulses lost to noise leave a gap a whole number of lines wide, which is
    /// why the multiples are tested at all.
    #[test]
    fn a_train_with_missed_pulses_still_identifies_its_mode() {
        let gaps = vec![period_seconds(Mode::Martin1) * 3.0; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Martin1)
        );
    }

    /// Robot 36 at two lines is exactly Robot 72 at one. Nothing in the signal
    /// separates them, so the reading that assumes no missed pulse wins.
    #[test]
    fn an_ambiguous_period_resolves_to_the_smallest_line_multiple() {
        let gaps = vec![period_seconds(Mode::Robot72); INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Robot72)
        );
    }

    /// The tolerance has to separate the closest pair of candidates, which is
    /// Martin 2 at two lines against Martin 1 at one, 1.6 % apart.
    #[test]
    fn neighbouring_candidates_are_not_confused() {
        let gaps = vec![period_seconds(Mode::Martin2) * 2.0; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Martin2)
        );
    }

    /// Random spacing is not a raster.
    #[test]
    fn unrelated_intervals_report_nothing() {
        let gaps = [0.21, 0.33, 0.19, 0.41, 0.27, 0.36, 0.23, 0.31];
        assert_eq!(detect_from_gaps(SyncStart::Any, 8_000.0, &gaps), None);
    }

    /// A receiver's clock is never exact, so a match cannot demand one.
    #[test]
    fn a_mistimed_raster_is_still_identified() {
        let gaps = vec![period_seconds(Mode::Scottie1) * 1.0003; INTERVAL_HISTORY];
        assert_eq!(
            detect_from_gaps(SyncStart::Any, 8_000.0, &gaps),
            Some(Mode::Scottie1)
        );
    }
}
