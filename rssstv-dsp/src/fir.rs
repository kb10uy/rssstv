use alloc::{vec, vec::Vec};
use core::f64::consts::PI;

use crate::DspError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Frequency response produced from the low-pass FIR prototype.
pub enum FirKind {
    /// Passes frequencies below the cutoff.
    LowPass,
    /// Passes frequencies above the cutoff.
    HighPass,
    /// Passes frequencies between the lower and upper edges.
    BandPass,
    /// Rejects frequencies between the lower and upper edges.
    BandStop,
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// Parameters for a Kaiser-windowed linear-phase FIR filter.
pub struct FirDesign {
    /// Desired frequency response.
    pub kind: FirKind,
    /// Filter order. It must be a positive even number and produces `order + 1` coefficients.
    pub order: usize,
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Cutoff for low/high-pass filters or lower edge for band filters, in hertz.
    pub lower_frequency_hz: f64,
    /// Upper edge for band filters, in hertz; ignored for low/high-pass filters.
    pub upper_frequency_hz: f64,
    /// Requested Kaiser stop-band attenuation in decibels.
    pub attenuation_db: f64,
    /// Linear gain applied to every generated coefficient.
    pub gain: f64,
}

impl FirDesign {
    /// Designs the filter and returns coefficients ordered newest sample first.
    pub fn coefficients(self) -> Result<Vec<f64>, DspError> {
        validate_fir_design(self)?;

        // Design one normalized low-pass prototype, then spectrally transform it
        // into the requested response. This preserves MMSSTV's order convention.
        let cutoff_hz = match self.kind {
            FirKind::LowPass => self.lower_frequency_hz,
            FirKind::HighPass => self.sample_rate_hz * 0.5 - self.lower_frequency_hz,
            FirKind::BandPass | FirKind::BandStop => {
                (self.upper_frequency_hz - self.lower_frequency_hz) * 0.5
            }
        };
        let alpha = kaiser_alpha(self.attenuation_db);
        let half_order = self.order / 2;
        let angular_cutoff = 2.0 * PI * cutoff_hz / self.sample_rate_hz;
        // Even orders produce an odd-length, linear-phase filter, so only the
        // center coefficient and one symmetric half need to be calculated.
        let mut half = vec![0.0; half_order + 1];

        for (index, coefficient) in half.iter_mut().enumerate() {
            let window = if self.attenuation_db >= 21.0 {
                let ratio = 2.0 * index as f64 / self.order as f64;
                bessel_i0(alpha * libm::sqrt(1.0 - ratio * ratio)) / bessel_i0(alpha)
            } else {
                1.0
            };
            *coefficient = if index == 0 {
                2.0 * cutoff_hz / self.sample_rate_hz
            } else {
                libm::sin(index as f64 * angular_cutoff) * window / (PI * index as f64)
            };
        }

        // Normalize the prototype at DC before applying a spectral transform.
        let sum = half[0] + 2.0 * half[1..].iter().sum::<f64>();
        if sum > 0.0 {
            for coefficient in &mut half {
                *coefficient /= sum;
            }
        }

        match self.kind {
            FirKind::LowPass => {}
            FirKind::HighPass => {
                for (index, coefficient) in half.iter_mut().enumerate() {
                    *coefficient *= libm::cos(index as f64 * PI);
                }
            }
            FirKind::BandPass => {
                let center =
                    PI * (self.lower_frequency_hz + self.upper_frequency_hz) / self.sample_rate_hz;
                for (index, coefficient) in half.iter_mut().enumerate() {
                    *coefficient *= 2.0 * libm::cos(index as f64 * center);
                }
            }
            FirKind::BandStop => {
                let center =
                    PI * (self.lower_frequency_hz + self.upper_frequency_hz) / self.sample_rate_hz;
                half[0] = 1.0 - 2.0 * half[0];
                for (index, coefficient) in half.iter_mut().enumerate().skip(1) {
                    *coefficient *= -2.0 * libm::cos(index as f64 * center);
                }
            }
        }

        let mut coefficients = Vec::with_capacity(self.order + 1);
        coefficients.extend(half.iter().rev().map(|value| value * self.gain));
        coefficients.extend(half.iter().skip(1).map(|value| value * self.gain));
        Ok(coefficients)
    }
}

#[derive(Clone, Debug)]
/// An owned streaming FIR filter with a circular delay line.
pub struct Fir {
    coefficients: Vec<f64>,
    delay: Vec<f64>,
    write_index: usize,
}

impl Fir {
    /// Creates a filter from coefficients ordered newest sample first.
    pub fn new(coefficients: Vec<f64>) -> Result<Self, DspError> {
        if coefficients.is_empty() {
            return Err(DspError::InvalidCoefficientCount);
        }
        if coefficients.iter().any(|value| !value.is_finite()) {
            return Err(DspError::InvalidCoefficient);
        }
        let delay = vec![0.0; coefficients.len()];
        Ok(Self {
            coefficients,
            delay,
            write_index: 0,
        })
    }

    /// Designs and creates a filter from `design`.
    pub fn from_design(design: FirDesign) -> Result<Self, DspError> {
        Self::new(design.coefficients()?)
    }

    /// Returns the active coefficient sequence.
    pub fn coefficients(&self) -> &[f64] {
        &self.coefficients
    }

    /// Returns the filter order, one less than the coefficient count.
    pub fn order(&self) -> usize {
        self.coefficients.len() - 1
    }

    /// Returns the linear-phase group delay in samples.
    pub fn group_delay_samples(&self) -> f64 {
        self.order() as f64 * 0.5
    }

    /// Processes one sample without allocating.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        self.delay[self.write_index] = sample;
        let mut delay_index = self.write_index;
        let mut output = 0.0;
        // Coefficient zero multiplies the newest sample. Walking the ring
        // backwards avoids shifting the complete delay line for every sample.
        for coefficient in &self.coefficients {
            output += coefficient * self.delay[delay_index];
            delay_index = if delay_index == 0 {
                self.delay.len() - 1
            } else {
                delay_index - 1
            };
        }
        self.write_index += 1;
        if self.write_index == self.delay.len() {
            self.write_index = 0;
        }
        output
    }

    /// Filters a block in place with the same state transitions as per-sample processing.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Replaces equal-length coefficients while preserving the delay-line state.
    pub fn replace_coefficients(&mut self, coefficients: Vec<f64>) -> Result<(), DspError> {
        if coefficients.len() != self.coefficients.len() {
            return Err(DspError::InvalidCoefficientCount);
        }
        if coefficients.iter().any(|value| !value.is_finite()) {
            return Err(DspError::InvalidCoefficient);
        }
        // Keeping the delay line allows seamless switching between equal-order
        // receive filters without introducing a fresh startup transient.
        self.coefficients = coefficients;
        Ok(())
    }

    /// Clears the delay line without changing coefficients.
    pub fn reset(&mut self) {
        self.delay.fill(0.0);
        self.write_index = 0;
    }

    fn delayed_sample(&self, delay: usize) -> f64 {
        let newest = if self.write_index == 0 {
            self.delay.len() - 1
        } else {
            self.write_index - 1
        };
        self.delay[(newest + self.delay.len() - delay % self.delay.len()) % self.delay.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
/// One sample of an analytic signal.
pub struct AnalyticSample {
    /// Original signal delayed to match the quadrature path.
    pub in_phase: f64,
    /// Hilbert-transformed quadrature component.
    pub quadrature: f64,
}

#[derive(Clone, Debug)]
/// Streaming band-limited Hilbert transformer.
pub struct HilbertTransformer {
    filter: Fir,
}

impl HilbertTransformer {
    /// Creates a transformer for the given passband and sampling frequency.
    pub fn new(
        order: usize,
        sample_rate_hz: f64,
        lower_frequency_hz: f64,
        upper_frequency_hz: f64,
    ) -> Result<Self, DspError> {
        Ok(Self {
            filter: Fir::new(hilbert_coefficients(
                order,
                sample_rate_hz,
                lower_frequency_hz,
                upper_frequency_hz,
            )?)?,
        })
    }

    /// Produces aligned in-phase and quadrature components for one input sample.
    pub fn process_sample(&mut self, sample: f64) -> AnalyticSample {
        let quadrature = self.filter.process_sample(sample);
        AnalyticSample {
            // Match the real path to the Hilbert FIR's linear-phase delay.
            in_phase: self.filter.delayed_sample(self.filter.order() / 2),
            quadrature,
        }
    }

    /// Clears both signal paths without changing the filter design.
    pub fn reset(&mut self) {
        self.filter.reset();
    }
}

/// Designs an odd-symmetric, Hamming-windowed Hilbert FIR.
///
/// Orders below eight retain MMSSTV's normalization by the coefficient absolute
/// sum. Orders of eight and above are unnormalized, so gain is discontinuous at
/// that boundary for compatibility with the reference implementation.
pub fn hilbert_coefficients(
    order: usize,
    sample_rate_hz: f64,
    lower_frequency_hz: f64,
    upper_frequency_hz: f64,
) -> Result<Vec<f64>, DspError> {
    validate_order(order)?;
    validate_frequency_range(sample_rate_hz, lower_frequency_hz, upper_frequency_hz)?;

    let center = order / 2;
    let sample_period = 1.0 / sample_rate_hz;
    let lower_angular = 2.0 * PI * lower_frequency_hz;
    let upper_angular = 2.0 * PI * upper_frequency_hz;
    let mut coefficients = Vec::with_capacity(order + 1);

    // The difference of two band-limited ideal Hilbert responses is windowed
    // directly. Its odd symmetry fixes the quadrature polarity used downstream.
    for index in 0..=order {
        let offset = index as isize - center as isize;
        let (lower, upper) = if offset == 0 {
            (0.0, 0.0)
        } else {
            let lower_argument = offset as f64 * lower_angular * sample_period;
            let upper_argument = offset as f64 * upper_angular * sample_period;
            (
                libm::cos(lower_argument) / lower_argument,
                libm::cos(upper_argument) / upper_argument,
            )
        };
        let window = 0.54 - 0.46 * libm::cos(2.0 * PI * index as f64 / order as f64);
        coefficients.push(
            -(2.0 * upper_frequency_hz * sample_period * upper
                - 2.0 * lower_frequency_hz * sample_period * lower)
                * window,
        );
    }

    if order < 8 {
        let magnitude = coefficients.iter().map(|value| value.abs()).sum::<f64>();
        if magnitude > 0.0 {
            for coefficient in &mut coefficients {
                *coefficient /= magnitude;
            }
        }
    }
    Ok(coefficients)
}

fn validate_fir_design(design: FirDesign) -> Result<(), DspError> {
    validate_order(design.order)?;
    if !design.attenuation_db.is_finite() || design.attenuation_db < 0.0 {
        return Err(DspError::InvalidAttenuation);
    }
    if kaiser_alpha(design.attenuation_db) > 700.0 {
        return Err(DspError::InvalidAttenuation);
    }
    if !design.gain.is_finite() {
        return Err(DspError::InvalidGain);
    }
    match design.kind {
        FirKind::LowPass | FirKind::HighPass => {
            validate_frequency(design.sample_rate_hz, design.lower_frequency_hz)
        }
        FirKind::BandPass | FirKind::BandStop => validate_frequency_range(
            design.sample_rate_hz,
            design.lower_frequency_hz,
            design.upper_frequency_hz,
        ),
    }
}

fn validate_order(order: usize) -> Result<(), DspError> {
    if order == 0 || !order.is_multiple_of(2) {
        Err(DspError::InvalidOrder)
    } else {
        Ok(())
    }
}

fn validate_frequency(sample_rate_hz: f64, frequency_hz: f64) -> Result<(), DspError> {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err(DspError::InvalidSampleRate);
    }
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || frequency_hz >= sample_rate_hz * 0.5 {
        return Err(DspError::InvalidFrequency);
    }
    Ok(())
}

fn validate_frequency_range(
    sample_rate_hz: f64,
    lower_frequency_hz: f64,
    upper_frequency_hz: f64,
) -> Result<(), DspError> {
    validate_frequency(sample_rate_hz, lower_frequency_hz)?;
    validate_frequency(sample_rate_hz, upper_frequency_hz)?;
    if lower_frequency_hz >= upper_frequency_hz {
        return Err(DspError::InvalidFrequency);
    }
    Ok(())
}

fn kaiser_alpha(attenuation_db: f64) -> f64 {
    if attenuation_db >= 50.0 {
        0.1102 * (attenuation_db - 8.7)
    } else if attenuation_db >= 21.0 {
        0.5842 * libm::pow(attenuation_db - 21.0, 0.4) + 0.07886 * (attenuation_db - 21.0)
    } else {
        0.0
    }
}

fn bessel_i0(value: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for index in 1..=1_024 {
        let index = f64::from(index);
        term *= 0.5 * value / index;
        let square = term * term;
        sum += square;
        if !sum.is_finite() || 1e-8 * sum > square {
            return sum;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn response_at(coefficients: &[f64], frequency_hz: f64, sample_rate_hz: f64) -> f64 {
        let angular = 2.0 * PI * frequency_hz / sample_rate_hz;
        let (real, imaginary) = coefficients.iter().enumerate().fold(
            (0.0, 0.0),
            |(real, imaginary), (index, coefficient)| {
                let phase = angular * index as f64;
                (
                    real + coefficient * libm::cos(phase),
                    imaginary - coefficient * libm::sin(phase),
                )
            },
        );
        libm::sqrt(real * real + imaginary * imaginary)
    }

    fn low_pass_design() -> FirDesign {
        FirDesign {
            kind: FirKind::LowPass,
            order: 24,
            sample_rate_hz: 11_025.0,
            lower_frequency_hz: 1_800.0,
            upper_frequency_hz: 0.0,
            attenuation_db: 40.0,
            gain: 1.0,
        }
    }

    #[test]
    fn designed_fir_has_expected_length_symmetry_and_dc_gain() {
        let coefficients = low_pass_design().coefficients().unwrap();
        assert_eq!(coefficients.len(), 25);
        for (left, right) in coefficients.iter().zip(coefficients.iter().rev()) {
            assert!((left - right).abs() < 1e-15);
        }
        assert!((coefficients.iter().sum::<f64>() - 1.0).abs() < 1e-14);
    }

    #[test]
    fn impulse_response_is_the_coefficient_sequence() {
        let coefficients = low_pass_design().coefficients().unwrap();
        let mut filter = Fir::new(coefficients.clone()).unwrap();
        let output: Vec<_> = core::iter::once(1.0)
            .chain(core::iter::repeat_n(0.0, coefficients.len() - 1))
            .map(|sample| filter.process_sample(sample))
            .collect();
        assert_eq!(output, coefficients);
    }

    #[test]
    fn block_and_sample_processing_are_equal() {
        let coefficients = low_pass_design().coefficients().unwrap();
        let mut block_filter = Fir::new(coefficients.clone()).unwrap();
        let mut sample_filter = Fir::new(coefficients).unwrap();
        let mut block = [1.0, -0.5, 0.25, 0.0, 0.75];
        let expected = block.map(|sample| sample_filter.process_sample(sample));
        block_filter.process_in_place(&mut block);
        assert_eq!(block, expected);
    }

    #[rstest]
    #[case(0)]
    #[case(3)]
    fn invalid_fir_orders_are_rejected(#[case] order: usize) {
        let mut design = low_pass_design();
        design.order = order;
        assert_eq!(design.coefficients(), Err(DspError::InvalidOrder));
    }

    #[test]
    fn extreme_attenuation_is_rejected_without_iterating_indefinitely() {
        let mut design = low_pass_design();
        design.attenuation_db = f64::MAX;
        assert_eq!(design.coefficients(), Err(DspError::InvalidAttenuation));
    }

    #[rstest]
    #[case(FirKind::LowPass, 200.0, 3_000.0)]
    #[case(FirKind::HighPass, 3_000.0, 200.0)]
    #[case(FirKind::BandPass, 1_500.0, 200.0)]
    #[case(FirKind::BandStop, 200.0, 1_500.0)]
    fn transformed_filters_pass_and_reject_expected_bands(
        #[case] kind: FirKind,
        #[case] pass_frequency_hz: f64,
        #[case] stop_frequency_hz: f64,
    ) {
        let coefficients = FirDesign {
            kind,
            order: 128,
            sample_rate_hz: 8_000.0,
            lower_frequency_hz: 1_000.0,
            upper_frequency_hz: 2_000.0,
            attenuation_db: 60.0,
            gain: 1.0,
        }
        .coefficients()
        .unwrap();
        assert!(response_at(&coefficients, pass_frequency_hz, 8_000.0) > 0.95);
        assert!(response_at(&coefficients, stop_frequency_hz, 8_000.0) < 0.002);
    }

    #[test]
    fn hilbert_coefficients_are_antisymmetric() {
        let coefficients = hilbert_coefficients(12, 11_025.0, 500.0, 2_800.0).unwrap();
        assert_eq!(coefficients[6], 0.0);
        for (left, right) in coefficients.iter().zip(coefficients.iter().rev()) {
            assert!((left + right).abs() < 1e-15);
        }
    }

    #[test]
    fn hilbert_transformer_produces_quadrature_for_a_passband_tone() {
        let sample_rate_hz = 8_000.0;
        let frequency_hz = 1_500.0;
        let mut transformer = HilbertTransformer::new(64, sample_rate_hz, 500.0, 3_000.0).unwrap();
        let mut in_phase_power = 0.0;
        let mut quadrature_power = 0.0;
        let mut cross = 0.0;
        for index in 0..4_096 {
            let sample = libm::sin(2.0 * PI * frequency_hz * index as f64 / sample_rate_hz);
            let analytic = transformer.process_sample(sample);
            if index >= 512 {
                in_phase_power += analytic.in_phase * analytic.in_phase;
                quadrature_power += analytic.quadrature * analytic.quadrature;
                cross += analytic.in_phase * analytic.quadrature;
            }
        }
        let normalized_cross = cross / libm::sqrt(in_phase_power * quadrature_power);
        assert!(normalized_cross.abs() < 0.002);
        assert!((quadrature_power / in_phase_power - 1.0).abs() < 0.02);
    }

    #[test]
    fn non_finite_coefficients_have_a_distinct_error() {
        assert_eq!(
            Fir::new(vec![1.0, f64::NAN]).err(),
            Some(DspError::InvalidCoefficient)
        );
        let mut filter = Fir::new(vec![1.0, 0.0]).unwrap();
        assert_eq!(
            filter.replace_coefficients(vec![1.0, f64::INFINITY]),
            Err(DspError::InvalidCoefficient)
        );
    }

    #[test]
    fn replacing_coefficients_preserves_history() {
        let mut filter = Fir::new(vec![1.0, 0.0, 0.0]).unwrap();
        assert_eq!(filter.process_sample(2.0), 2.0);
        filter.replace_coefficients(vec![0.0, 1.0, 0.0]).unwrap();
        assert_eq!(filter.process_sample(3.0), 2.0);
    }
}
