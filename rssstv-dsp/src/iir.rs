use alloc::vec::Vec;
use core::f64::consts::PI;

use crate::DspError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IirResponse {
    Butterworth,
    Chebyshev { ripple_db: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IirLowPassDesign {
    pub order: usize,
    pub sample_rate_hz: f64,
    pub cutoff_hz: f64,
    pub response: IirResponse,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SosCoefficients {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl IirLowPassDesign {
    pub fn coefficients(self) -> Result<Vec<SosCoefficients>, DspError> {
        validate_design(self)?;
        let section_count = self.order.div_ceil(2);
        let mut sections = Vec::with_capacity(section_count);
        // Prewarp the digital cutoff before mapping the analog prototype poles
        // through the bilinear transform.
        let warped_cutoff = libm::tan(PI * self.cutoff_hz / self.sample_rate_hz);
        let chebyshev_u = match self.response {
            IirResponse::Butterworth => None,
            IirResponse::Chebyshev { ripple_db } => Some(
                libm::asinh(1.0 / libm::sqrt(libm::pow(10.0, 0.1 * ripple_db) - 1.0))
                    / self.order as f64,
            ),
        };
        let mut pole_index = (self.order & 1) + 1;

        // Pair conjugate prototype poles into real second-order sections.
        for _ in 0..self.order / 2 {
            let angle = pole_index as f64 * PI / (2.0 * self.order as f64);
            let (natural_frequency, damping) = if let Some(u) = chebyshev_u {
                let d1 = libm::sinh(u) * libm::cos(angle);
                let d2 = libm::cosh(u) * libm::sin(angle);
                let natural_frequency = libm::sqrt(d1 * d1 + d2 * d2);
                (natural_frequency, d1 / natural_frequency)
            } else {
                (1.0, libm::cos(angle))
            };
            let scaled = warped_cutoff * natural_frequency;
            let denominator = 1.0 + 2.0 * scaled * damping + scaled * scaled;
            let recursive_1 = -2.0 * (scaled * scaled - 1.0) / denominator;
            let recursive_2 = -(1.0 - 2.0 * scaled * damping + scaled * scaled) / denominator;
            let b0 = scaled * scaled / denominator;
            sections.push(SosCoefficients {
                b0,
                b1: 2.0 * b0,
                b2: b0,
                // The source recurrence stores positive feedback terms; the
                // public SOS representation uses the standard denominator signs.
                a1: -recursive_1,
                a2: -recursive_2,
            });
            pole_index += 2;
        }

        // An even-order Chebyshev response reaches its ripple minimum at DC.
        if let IirResponse::Chebyshev { ripple_db } = self.response
            && self.order.is_multiple_of(2)
        {
            let scale = libm::pow(
                1.0 / libm::pow(10.0, ripple_db / 20.0),
                1.0 / (self.order / 2) as f64,
            );
            for section in &mut sections {
                section.b0 *= scale;
                section.b1 *= scale;
                section.b2 *= scale;
            }
        }

        if !self.order.is_multiple_of(2) {
            let natural_frequency = chebyshev_u.map_or(1.0, libm::sinh);
            let scaled = warped_cutoff * natural_frequency;
            let denominator = 1.0 + scaled;
            let recursive = -(scaled - 1.0) / denominator;
            let b0 = scaled / denominator;
            sections.push(SosCoefficients {
                b0,
                b1: b0,
                b2: 0.0,
                a1: -recursive,
                a2: 0.0,
            });
        }
        Ok(sections)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SosState {
    z1: f64,
    z2: f64,
}

#[derive(Clone, Debug)]
pub struct IirFilter {
    coefficients: Vec<SosCoefficients>,
    states: Vec<SosState>,
}

impl IirFilter {
    pub fn new(coefficients: Vec<SosCoefficients>) -> Result<Self, DspError> {
        if coefficients.is_empty()
            || coefficients.iter().any(|section| {
                [section.b0, section.b1, section.b2, section.a1, section.a2]
                    .iter()
                    .any(|value| !value.is_finite())
            })
        {
            return Err(DspError::InvalidCoefficientCount);
        }
        let states = alloc::vec![SosState::default(); coefficients.len()];
        Ok(Self {
            coefficients,
            states,
        })
    }

    pub fn from_low_pass(design: IirLowPassDesign) -> Result<Self, DspError> {
        Self::new(design.coefficients()?)
    }

    pub fn coefficients(&self) -> &[SosCoefficients] {
        &self.coefficients
    }

    pub fn process_sample(&mut self, mut sample: f64) -> f64 {
        for (coefficient, state) in self.coefficients.iter().zip(&mut self.states) {
            // Transposed direct form II limits internal state growth while using
            // only two delay values per section.
            let output = coefficient.b0 * sample + state.z1;
            state.z1 = coefficient.b1 * sample - coefficient.a1 * output + state.z2;
            state.z2 = coefficient.b2 * sample - coefficient.a2 * output;
            // Prevent subnormal state from degrading sustained real-time work.
            if state.z1.abs() < 1e-37 {
                state.z1 = 0.0;
            }
            if state.z2.abs() < 1e-37 {
                state.z2 = 0.0;
            }
            sample = output;
        }
        sample
    }

    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    pub fn reset(&mut self) {
        self.states.fill(SosState::default());
    }
}

fn validate_design(design: IirLowPassDesign) -> Result<(), DspError> {
    if design.order == 0 {
        return Err(DspError::InvalidOrder);
    }
    if !design.sample_rate_hz.is_finite() || design.sample_rate_hz <= 0.0 {
        return Err(DspError::InvalidSampleRate);
    }
    if !design.cutoff_hz.is_finite()
        || design.cutoff_hz <= 0.0
        || design.cutoff_hz >= design.sample_rate_hz * 0.5
    {
        return Err(DspError::InvalidFrequency);
    }
    if let IirResponse::Chebyshev { ripple_db } = design.response
        && (!ripple_db.is_finite() || ripple_db <= 0.0)
    {
        return Err(DspError::InvalidRipple);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(2)]
    #[case(3)]
    #[case(6)]
    fn low_pass_converges_to_unity_for_dc(#[case] order: usize) {
        let mut filter = IirFilter::from_low_pass(IirLowPassDesign {
            order,
            sample_rate_hz: 11_025.0,
            cutoff_hz: 900.0,
            response: IirResponse::Butterworth,
        })
        .unwrap();
        let mut output = 0.0;
        for _ in 0..10_000 {
            output = filter.process_sample(1.0);
        }
        assert!((output - 1.0).abs() < 1e-12);
    }

    #[test]
    fn reset_restores_initial_state() {
        let design = IirLowPassDesign {
            order: 3,
            sample_rate_hz: 11_025.0,
            cutoff_hz: 1_800.0,
            response: IirResponse::Butterworth,
        };
        let mut filter = IirFilter::from_low_pass(design).unwrap();
        let expected = filter.process_sample(1.0);
        filter.process_sample(0.5);
        filter.reset();
        assert_eq!(filter.process_sample(1.0), expected);
    }

    #[test]
    fn chebyshev_even_order_applies_requested_dc_gain() {
        let ripple_db = 0.5;
        let mut filter = IirFilter::from_low_pass(IirLowPassDesign {
            order: 4,
            sample_rate_hz: 11_025.0,
            cutoff_hz: 900.0,
            response: IirResponse::Chebyshev { ripple_db },
        })
        .unwrap();
        let mut output = 0.0;
        for _ in 0..10_000 {
            output = filter.process_sample(1.0);
        }
        let expected = libm::pow(10.0, -ripple_db / 20.0);
        assert!((output - expected).abs() < 1e-12);
    }

    #[test]
    fn block_and_sample_processing_are_equal() {
        let design = IirLowPassDesign {
            order: 2,
            sample_rate_hz: 8_000.0,
            cutoff_hz: 50.0,
            response: IirResponse::Butterworth,
        };
        let mut block_filter = IirFilter::from_low_pass(design).unwrap();
        let mut sample_filter = IirFilter::from_low_pass(design).unwrap();
        let mut block = [1.0, -1.0, 0.5, 0.25];
        let expected = block.map(|sample| sample_filter.process_sample(sample));
        block_filter.process_in_place(&mut block);
        assert_eq!(block, expected);
    }
}
