use alloc::vec::Vec;
use core::f64::consts::TAU;

use crate::DspError;

const SINE_TABLE_LENGTH: usize = 4_096;

#[derive(Clone, Debug)]
/// A phase-continuous, table-based voltage-controlled sine oscillator.
pub struct Vco {
    sample_rate_hz: f64,
    free_frequency_hz: f64,
    control_gain_hz: f64,
    phase: f64,
    sine_table: Vec<f64>,
}

impl Vco {
    /// Creates a VCO with frequencies in hertz and a linearly interpolated sine table.
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
        let mut sine_table = Vec::with_capacity(SINE_TABLE_LENGTH);
        for index in 0..SINE_TABLE_LENGTH {
            sine_table.push(libm::sin(TAU * index as f64 / SINE_TABLE_LENGTH as f64));
        }
        Ok(Self {
            sample_rate_hz,
            free_frequency_hz,
            control_gain_hz,
            phase: 0.0,
            sine_table,
        })
    }

    /// Returns the sampling frequency in hertz.
    pub fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    /// Returns the free-running frequency in hertz.
    pub fn free_frequency_hz(&self) -> f64 {
        self.free_frequency_hz
    }

    /// Returns the frequency deviation produced by one control unit, in hertz.
    pub fn control_gain_hz(&self) -> f64 {
        self.control_gain_hz
    }

    /// Changes the free-running frequency without resetting phase.
    pub fn set_free_frequency(&mut self, frequency_hz: f64) -> Result<(), DspError> {
        validate_frequency(frequency_hz, self.sample_rate_hz)?;
        self.free_frequency_hz = frequency_hz;
        Ok(())
    }

    /// Changes the frequency deviation per control unit without resetting phase.
    pub fn set_control_gain(&mut self, gain_hz: f64) -> Result<(), DspError> {
        if !gain_hz.is_finite() {
            return Err(DspError::InvalidFrequency);
        }
        self.control_gain_hz = gain_hz;
        Ok(())
    }

    /// Advances the oscillator and returns one sine sample.
    ///
    /// Instantaneous frequency is `free_frequency_hz + control * control_gain_hz`.
    pub fn process_sample(&mut self, control: f64) -> Result<f64, DspError> {
        let frequency_hz = self.free_frequency_hz + control * self.control_gain_hz;
        if !frequency_hz.is_finite() || frequency_hz.abs() >= self.sample_rate_hz * 0.5 {
            return Err(DspError::InvalidFrequency);
        }
        // Advance before lookup to match the original VCO's sample timing.
        self.phase += frequency_hz / self.sample_rate_hz;
        // x - floor(x) wraps positive and negative frequencies into [0, 1).
        self.phase -= libm::floor(self.phase);
        Ok(self.sine_at_phase(self.phase))
    }

    /// Resets phase to zero without changing oscillator frequencies.
    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    fn sine_at_phase(&self, phase: f64) -> f64 {
        let position = phase * self.sine_table.len() as f64;
        let lower_index = position as usize % self.sine_table.len();
        let upper_index = (lower_index + 1) % self.sine_table.len();
        let fraction = position - libm::floor(position);
        // Linear interpolation favors spectral quality over bit compatibility
        // with MMSSTV's truncated table lookup.
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
        let mut previous = oscillator.process_sample(0.0).unwrap();
        let mut crossings = 0;
        for _ in 1..11_025 {
            let sample = oscillator.process_sample(0.0).unwrap();
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
        let first = oscillator.process_sample(0.0).unwrap();
        let controlled = oscillator.process_sample(1.0).unwrap();
        assert!((first - libm::sin(TAU / 8.0)).abs() < 1e-8);
        assert!((controlled - libm::sin(TAU * 2.5 / 8.0)).abs() < 1e-8);
    }

    #[test]
    fn phase_reset_reproduces_initial_sample() {
        let mut oscillator = Vco::new(11_025.0, 1_900.0, 400.0).unwrap();
        let expected = oscillator.process_sample(0.25).unwrap();
        oscillator.process_sample(-0.5).unwrap();
        oscillator.reset_phase();
        assert_eq!(oscillator.process_sample(0.25).unwrap(), expected);
    }

    #[test]
    fn negative_frequency_wraps_phase() {
        let mut oscillator = Vco::new(8_000.0, 1_000.0, 2_000.0).unwrap();
        let sample = oscillator.process_sample(-1.0).unwrap();
        assert!((sample + libm::sin(TAU / 8.0)).abs() < 1e-8);
    }

    #[test]
    fn rejects_non_finite_and_nyquist_controlled_frequencies() {
        let mut oscillator = Vco::new(8_000.0, 1_000.0, 2_000.0).unwrap();
        assert_eq!(
            oscillator.process_sample(1.5),
            Err(DspError::InvalidFrequency)
        );
        assert_eq!(
            oscillator.process_sample(f64::NAN),
            Err(DspError::InvalidFrequency)
        );
    }

    #[test]
    fn table_size_is_independent_of_sample_rate() {
        let low = Vco::new(8_000.0, 1_000.0, 0.0).unwrap();
        let high = Vco::new(192_000.0, 1_000.0, 0.0).unwrap();
        assert_eq!(low.sine_table.len(), SINE_TABLE_LENGTH);
        assert_eq!(high.sine_table.len(), SINE_TABLE_LENGTH);
    }
}
