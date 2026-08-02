use alloc::vec::Vec;
use core::f64::consts::TAU;

use crate::DspError;

#[derive(Clone, Debug)]
pub struct Vco {
    sample_rate_hz: f64,
    free_frequency_hz: f64,
    control_gain_hz: f64,
    phase: f64,
    sine_table: Vec<f64>,
}

impl Vco {
    pub fn new(
        sample_rate_hz: f64,
        free_frequency_hz: f64,
        control_gain_hz: f64,
    ) -> Result<Self, DspError> {
        validate_sample_rate(sample_rate_hz)?;
        validate_frequency(free_frequency_hz, sample_rate_hz)?;
        if !control_gain_hz.is_finite() {
            return Err(DspError::InvalidFrequency);
        }
        let table_length = libm::ceil(sample_rate_hz * 2.0) as usize;
        let mut sine_table = Vec::with_capacity(table_length);
        for index in 0..table_length {
            sine_table.push(libm::sin(TAU * index as f64 / table_length as f64));
        }
        Ok(Self {
            sample_rate_hz,
            free_frequency_hz,
            control_gain_hz,
            phase: 0.0,
            sine_table,
        })
    }

    pub fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    pub fn free_frequency_hz(&self) -> f64 {
        self.free_frequency_hz
    }

    pub fn control_gain_hz(&self) -> f64 {
        self.control_gain_hz
    }

    pub fn set_free_frequency(&mut self, frequency_hz: f64) -> Result<(), DspError> {
        validate_frequency(frequency_hz, self.sample_rate_hz)?;
        self.free_frequency_hz = frequency_hz;
        Ok(())
    }

    pub fn set_control_gain(&mut self, gain_hz: f64) -> Result<(), DspError> {
        if !gain_hz.is_finite() {
            return Err(DspError::InvalidFrequency);
        }
        self.control_gain_hz = gain_hz;
        Ok(())
    }

    pub fn process_sample(&mut self, control: f64) -> f64 {
        let frequency_hz = self.free_frequency_hz + control * self.control_gain_hz;
        self.phase += frequency_hz / self.sample_rate_hz;
        self.phase -= libm::floor(self.phase);
        self.sine_at_phase(self.phase)
    }

    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    fn sine_at_phase(&self, phase: f64) -> f64 {
        let position = phase * self.sine_table.len() as f64;
        let lower_index = position as usize % self.sine_table.len();
        let upper_index = (lower_index + 1) % self.sine_table.len();
        let fraction = position - libm::floor(position);
        self.sine_table[lower_index]
            + fraction * (self.sine_table[upper_index] - self.sine_table[lower_index])
    }
}

fn validate_sample_rate(sample_rate_hz: f64) -> Result<(), DspError> {
    if !sample_rate_hz.is_finite() || sample_rate_hz < 2.0 {
        Err(DspError::InvalidSampleRate)
    } else {
        Ok(())
    }
}

fn validate_frequency(frequency_hz: f64, sample_rate_hz: f64) -> Result<(), DspError> {
    if !frequency_hz.is_finite() || frequency_hz < 0.0 || frequency_hz >= sample_rate_hz * 0.5 {
        Err(DspError::InvalidFrequency)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tone_has_accurate_frequency() {
        let sample_rate = 11_025.0;
        let frequency = 1_900.0;
        let mut oscillator = Vco::new(sample_rate, frequency, 0.0).unwrap();
        let mut previous = oscillator.process_sample(0.0);
        let mut crossings = 0;
        for _ in 1..11_025 {
            let sample = oscillator.process_sample(0.0);
            if previous < 0.0 && sample >= 0.0 {
                crossings += 1;
            }
            previous = sample;
        }
        assert!((crossings as f64 - frequency).abs() <= 1.0);
    }

    #[test]
    fn control_changes_frequency_and_phase_remains_continuous() {
        let mut oscillator = Vco::new(8_000.0, 1_000.0, 500.0).unwrap();
        let first = oscillator.process_sample(0.0);
        let controlled = oscillator.process_sample(1.0);
        assert!((first - libm::sin(TAU / 8.0)).abs() < 1e-8);
        assert!((controlled - libm::sin(TAU * 2.5 / 8.0)).abs() < 1e-8);
    }

    #[test]
    fn phase_reset_reproduces_initial_sample() {
        let mut oscillator = Vco::new(11_025.0, 1_900.0, 400.0).unwrap();
        let expected = oscillator.process_sample(0.25);
        oscillator.process_sample(-0.5);
        oscillator.reset_phase();
        assert_eq!(oscillator.process_sample(0.25), expected);
    }

    #[test]
    fn negative_frequency_wraps_phase() {
        let mut oscillator = Vco::new(8_000.0, 1_000.0, 2_000.0).unwrap();
        let sample = oscillator.process_sample(-1.0);
        assert!((sample + libm::sin(TAU / 8.0)).abs() < 1e-8);
    }
}
