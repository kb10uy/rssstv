//! MMSSTV-style audio front end for SSTV receive decoding.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};

use rssstv_dsp::fir::{Fir, FirDesign, FirKind, HilbertTransformer};
use rssstv_dsp::frequency::ZeroCrossingFrequency;
use rssstv_dsp::iir::{IirFilter, IirLowPassDesign, IirResponse};
use rssstv_dsp::resonator::Resonator;
use rssstv_fskid::{FskDecoder, FskId, FskTone};
use rssstv_sstv::mode::Mode;
use rssstv_sstv::time::SstvDuration;
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
    /// Returns a mode when conventional VIS was first detected in this packet.
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

    /// Returns the detected conventional VIS mode, if available.
    pub const fn mode(&self) -> Option<Mode> {
        self.mode
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
        let mode = self.vis.process(envelopes);
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
    previous_phase: Option<f64>,
    output_filter: IirFilter,
    held_frequency: f64,
}

impl HilbertDiscriminator {
    fn new(sample_rate_hz: f64) -> Result<Self, rssstv_dsp::DspError> {
        let order = if sample_rate_hz < 16_000.0 {
            12
        } else if sample_rate_hz < 40_000.0 {
            24
        } else {
            48
        };
        Ok(Self {
            transformer: HilbertTransformer::new(order, sample_rate_hz, 1_000.0, 2_700.0)?,
            sample_rate_hz,
            previous_phase: None,
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
        if let Some(previous) = self.previous_phase.replace(phase)
            && magnitude > 1.0e-8
        {
            let delta = (phase - previous + PI).rem_euclid(TAU) - PI;
            self.held_frequency = (delta.abs() * self.sample_rate_hz / TAU).clamp(0.0, 3_000.0);
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
    use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};
    use rssstv_sstv::rx::{DemodulatedBlock, RxConfig};
    use rssstv_sstv::{RxDecoder, TxEncoder};
    use rstest::rstest;

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

    #[rstest]
    #[case(Mode::Martin2, 8_000, 2.0)]
    #[case(Mode::Martin2, 48_000, 2.0)]
    #[case(Mode::Scottie2, 8_000, 3.1)]
    #[case(Mode::Robot36, 8_000, 2.3)]
    #[case(Mode::Robot36, 48_000, 2.3)]
    #[case(Mode::Pd50, 8_000, 1.0)]
    #[case(Mode::Pd50, 48_000, 1.0)]
    fn aligns_causal_sync_with_frequency_output(
        #[case] mode: Mode,
        #[case] rate: u32,
        #[case] expected_ms: f64,
    ) {
        let offset_ms = raster_epoch_error_ms(mode, rate);
        assert!(
            (offset_ms - expected_ms).abs() <= 0.75,
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
}
