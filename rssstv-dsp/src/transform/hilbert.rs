use alloc::vec::Vec;
use core::f64::consts::PI;

use crate::{
    DspError,
    filter::Fir,
    validate::{validate_frequency_range, validate_order},
};

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
        sample_rate_hz: f64,
        order: usize,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut transformer = HilbertTransformer::new(sample_rate_hz, 64, 500.0, 3_000.0).unwrap();
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
}
