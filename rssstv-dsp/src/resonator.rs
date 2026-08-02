use core::f64::consts::PI;

use crate::DspError;

#[derive(Clone, Copy, Debug, PartialEq)]
/// A two-pole narrow-band resonator for tone-envelope detection.
pub struct Resonator {
    sample_rate_hz: f64,
    frequency_hz: f64,
    bandwidth_hz: f64,
    input_gain: f64,
    feedback_1: f64,
    feedback_2: f64,
    state_1: f64,
    state_2: f64,
}

impl Resonator {
    /// Creates a resonator centered at `frequency_hz` with the requested bandwidth.
    pub fn new(
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
    ) -> Result<Self, DspError> {
        validate_parameters(frequency_hz, sample_rate_hz, bandwidth_hz)?;
        let mut resonator = Self {
            sample_rate_hz,
            frequency_hz,
            bandwidth_hz,
            input_gain: 0.0,
            feedback_1: 0.0,
            feedback_2: 0.0,
            state_1: 0.0,
            state_2: 0.0,
        };
        resonator.update_coefficients();
        Ok(resonator)
    }

    /// Returns the current center frequency in hertz.
    pub fn frequency_hz(&self) -> f64 {
        self.frequency_hz
    }

    /// Retunes the center frequency while preserving resonator state.
    pub fn set_frequency(&mut self, frequency_hz: f64) -> Result<(), DspError> {
        validate_parameters(frequency_hz, self.sample_rate_hz, self.bandwidth_hz)?;
        self.frequency_hz = frequency_hz;
        // Retuning intentionally preserves state so AFC updates do not reset the
        // detector envelope.
        self.update_coefficients();
        Ok(())
    }

    /// Processes one sample without allocating.
    pub fn process_sample(&mut self, sample: f64) -> f64 {
        // Two-pole resonator: y[n] = a0*x[n] + b1*y[n-1] + b2*y[n-2].
        let mut output = sample * self.input_gain
            + self.state_1 * self.feedback_1
            + self.state_2 * self.feedback_2;
        self.state_2 = self.state_1;
        if output.abs() < 1e-37 {
            output = 0.0;
        }
        self.state_1 = output;
        output
    }

    /// Processes a block in place.
    pub fn process_in_place(&mut self, samples: &mut [f64]) {
        for sample in samples {
            *sample = self.process_sample(*sample);
        }
    }

    /// Clears the resonator history without changing its tuning.
    pub fn reset(&mut self) {
        self.state_1 = 0.0;
        self.state_2 = 0.0;
    }

    fn update_coefficients(&mut self) {
        let angular_frequency = 2.0 * PI * self.frequency_hz / self.sample_rate_hz;
        // Bandwidth places the pole radius; frequency places the pole angle.
        self.feedback_1 = 2.0
            * libm::exp(-PI * self.bandwidth_hz / self.sample_rate_hz)
            * libm::cos(angular_frequency);
        self.feedback_2 = libm::exp(-2.0 * PI * self.bandwidth_hz / self.sample_rate_hz);
        self.feedback_2 = -self.feedback_2;
        // Preserve the detector scaling used by MMSSTV's tone-envelope bank.
        self.input_gain = if self.bandwidth_hz == 0.0 {
            libm::sin(angular_frequency)
        } else {
            libm::sin(angular_frequency) * self.bandwidth_hz / (self.sample_rate_hz / 6.0)
        };
    }
}

fn validate_parameters(
    frequency_hz: f64,
    sample_rate_hz: f64,
    bandwidth_hz: f64,
) -> Result<(), DspError> {
    if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
        return Err(DspError::InvalidSampleRate);
    }
    if !frequency_hz.is_finite() || frequency_hz <= 0.0 || frequency_hz >= sample_rate_hz * 0.5 {
        return Err(DspError::InvalidFrequency);
    }
    if !bandwidth_hz.is_finite() || bandwidth_hz < 0.0 {
        return Err(DspError::InvalidBandwidth);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_at(resonator_frequency: f64, input_frequency: f64) -> f64 {
        let sample_rate = 11_025.0;
        let mut resonator = Resonator::new(resonator_frequency, sample_rate, 80.0).unwrap();
        let mut sum = 0.0;
        for index in 0..22_050 {
            let sample = libm::sin(2.0 * PI * input_frequency * index as f64 / sample_rate);
            let output = resonator.process_sample(sample);
            if index >= 11_025 {
                sum += output * output;
            }
        }
        libm::sqrt(sum / 11_025.0)
    }

    #[test]
    fn center_frequency_has_higher_response_than_adjacent_detector() {
        let center = rms_at(1_200.0, 1_200.0);
        let adjacent = rms_at(1_200.0, 1_320.0);
        assert!(center > adjacent * 2.0);
    }

    #[test]
    fn retuning_preserves_state_and_reset_clears_it() {
        let mut resonator = Resonator::new(1_200.0, 11_025.0, 80.0).unwrap();
        resonator.process_sample(1.0);
        resonator.set_frequency(1_320.0).unwrap();
        assert_ne!(resonator.process_sample(0.0), 0.0);
        resonator.reset();
        assert_eq!(resonator.process_sample(0.0), 0.0);
    }
}
