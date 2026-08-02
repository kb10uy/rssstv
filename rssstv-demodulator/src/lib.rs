//! MMSSTV-style audio front end for SSTV receive decoding.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};

use rssstv_dsp::fir::{Fir, FirDesign, FirKind, HilbertTransformer};
use rssstv_dsp::frequency::ZeroCrossingFrequency;
use rssstv_dsp::iir::{IirFilter, IirLowPassDesign, IirResponse};
use rssstv_dsp::resonator::Resonator;
use rssstv_sstv::mode::Mode;
use thiserror::Error;

const DETECTORS: [(f64, f64); 5] = [
    (1_080.0, 80.0),
    (1_200.0, 100.0),
    (1_320.0, 80.0),
    (1_900.0, 100.0),
    (2_100.0, 100.0),
];
const SYNC_ENVELOPE_ADVANCE_SECONDS: f64 = 0.006;

/// Failure while configuring or processing the receive front end.
#[derive(Debug, Error)]
pub enum DemodulatorError {
    /// The physical sample rate cannot represent the SSTV receive band.
    #[error("sample rate {0} Hz is too low for SSTV")]
    SampleRateTooLow(u32),
    /// A PCM sample was not finite.
    #[error("PCM sample {index} is not finite")]
    NonFiniteSample {
        /// Zero-based PCM sample index.
        index: usize,
    },
    /// A DSP processor rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] rssstv_dsp::DspError),
    /// No complete supported conventional VIS sequence was found.
    #[error("no supported conventional VIS code was detected")]
    VisNotDetected,
}

/// Demodulated SSTV data and the mode selected from conventional VIS.
#[derive(Clone, Debug)]
pub struct DemodulatedAudio {
    mode: Mode,
    first_sample: u64,
    frequency_hz: Vec<f32>,
    sync_strength: Vec<f32>,
    frequency_offset_hz: f64,
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

    /// Returns normalized horizontal-sync confidence.
    pub fn sync_strength(&self) -> &[f32] {
        &self.sync_strength
    }

    /// Returns the final smoothed receiver frequency offset.
    pub const fn frequency_offset_hz(&self) -> f64 {
        self.frequency_offset_hz
    }
}

/// Demodulates normalized mono PCM and detects its conventional VIS mode.
pub fn demodulate(
    samples: &[f32],
    sample_rate_hz: u32,
) -> Result<DemodulatedAudio, DemodulatorError> {
    if sample_rate_hz < 6_000 {
        return Err(DemodulatorError::SampleRateTooLow(sample_rate_hz));
    }
    for (index, sample) in samples.iter().enumerate() {
        if !sample.is_finite() {
            return Err(DemodulatorError::NonFiniteSample { index });
        }
    }

    let rate = sample_rate_hz as f64;
    let mut front_end = FrontEnd::new(rate)?;
    let mut frequency_hz = Vec::with_capacity(samples.len());
    let mut sync_strength = Vec::with_capacity(samples.len());
    let mut detection = None;

    for (index, &sample) in samples.iter().enumerate() {
        let output = front_end.process(sample as f64)?;
        frequency_hz.push(output.frequency_hz as f32);
        sync_strength.push(output.sync_strength as f32);
        if detection.is_none()
            && let Some(mode) = output.mode
        {
            detection = Some((mode, index + 1));
            front_end.enable_afc();
        }
    }
    front_end.finish_afc()?;

    let (mode, first) = detection.ok_or(DemodulatorError::VisNotDetected)?;
    let advance = (rate * SYNC_ENVELOPE_ADVANCE_SECONDS).round() as usize;
    if advance > 0 {
        for index in first..sync_strength.len() {
            sync_strength[index] = sync_strength
                .get(index + advance)
                .copied()
                .unwrap_or_default();
        }
    }

    Ok(DemodulatedAudio {
        mode,
        first_sample: first as u64,
        frequency_hz: frequency_hz.split_off(first),
        sync_strength: sync_strength.split_off(first),
        frequency_offset_hz: front_end.afc.offset_hz,
    })
}

struct FrontEndOutput {
    frequency_hz: f64,
    sync_strength: f64,
    mode: Option<Mode>,
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
            || !(0.003..=0.020).contains(&duration)
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

    fn tone(samples: &mut Vec<f32>, rate: u32, frequency: f64, seconds: f64, phase: &mut f64) {
        for _ in 0..(rate as f64 * seconds).round() as usize {
            samples.push((*phase).sin() as f32 * 0.8);
            *phase = (*phase + TAU * frequency / rate as f64).rem_euclid(TAU);
        }
    }

    fn vis_signal(mode: Mode, rate: u32, offset: f64) -> Vec<f32> {
        let mut samples = Vec::new();
        let mut phase = 0.0;
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

    #[test]
    fn detects_parity_inclusive_vis() {
        let samples = vis_signal(Mode::Scottie2, 8_000, 0.0);
        let output = demodulate(&samples, 8_000).unwrap();
        assert_eq!(output.mode(), Mode::Scottie2);
    }

    #[test]
    fn rejects_low_sample_rate() {
        assert!(matches!(
            demodulate(&[], 4_000),
            Err(DemodulatorError::SampleRateTooLow(4_000))
        ));
    }
}
