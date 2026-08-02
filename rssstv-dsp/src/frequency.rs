use crate::DspError;

#[derive(Clone, Debug)]
/// Measures instantaneous frequency from interpolated zero crossings.
///
/// Both crossing polarities are used, producing at most one measurement per
/// half-cycle. Smoothing and application-specific range limiting are left to
/// the caller.
pub struct ZeroCrossingFrequency {
    sample_rate_hz: f64,
    previous_sample: Option<f64>,
    sample_index: u64,
    previous_crossing: Option<f64>,
}

impl ZeroCrossingFrequency {
    /// Creates a frequency meter for the given sampling frequency.
    pub fn new(sample_rate_hz: f64) -> Result<Self, DspError> {
        if !sample_rate_hz.is_finite() || sample_rate_hz <= 0.0 {
            return Err(DspError::InvalidSampleRate);
        }
        Ok(Self {
            sample_rate_hz,
            previous_sample: None,
            sample_index: 0,
            previous_crossing: None,
        })
    }

    /// Accepts one sample and returns hertz when a complete half-cycle is available.
    pub fn process_sample(&mut self, sample: f64) -> Option<f64> {
        let previous_sample = self.previous_sample.replace(sample);
        // Both polarities are measured, yielding one estimate per half-cycle.
        let crossed = previous_sample.is_some_and(|previous| {
            (sample >= 0.0 && previous < 0.0) || (sample < 0.0 && previous >= 0.0)
        });
        let frequency = if let Some(previous_sample) = previous_sample.filter(|_| crossed) {
            // Linear interpolation estimates the fractional sample where the
            // segment between the previous and current values crosses zero.
            let fraction = sample / (sample - previous_sample);
            let crossing = self.sample_index as f64 - fraction;
            let frequency = self.previous_crossing.and_then(|previous| {
                let half_cycle_samples = crossing - previous;
                // Reject sub-sample half-cycles, which cannot be represented
                // without aliasing at this sample rate.
                (half_cycle_samples >= 1.0)
                    .then_some(self.sample_rate_hz * 0.5 / half_cycle_samples)
            });
            self.previous_crossing = Some(crossing);
            frequency
        } else {
            None
        };
        self.sample_index = self.sample_index.wrapping_add(1);
        frequency
    }

    /// Discards sample and crossing history.
    pub fn reset(&mut self) {
        self.previous_sample = None;
        self.sample_index = 0;
        self.previous_crossing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::f64::consts::TAU;
    use rstest::rstest;

    #[rstest]
    #[case(1_200.0)]
    #[case(1_900.0)]
    #[case(2_300.0)]
    fn measures_sstv_tones(#[case] frequency_hz: f64) {
        let sample_rate_hz = 48_000.0;
        let mut counter = ZeroCrossingFrequency::new(sample_rate_hz).unwrap();
        let mut measurements = alloc::vec::Vec::new();
        for index in 0..48_000 {
            let sample = libm::sin(TAU * frequency_hz * index as f64 / sample_rate_hz + 0.37);
            if let Some(measured) = counter.process_sample(sample) {
                measurements.push(measured);
            }
        }
        let average = measurements.iter().sum::<f64>() / measurements.len() as f64;
        assert!((average - frequency_hz).abs() < 0.05);
    }

    #[test]
    fn interpolates_crossing_between_samples() {
        let mut counter = ZeroCrossingFrequency::new(8_000.0).unwrap();
        assert_eq!(counter.process_sample(-1.0), None);
        assert_eq!(counter.process_sample(1.0), None);
        assert_eq!(counter.process_sample(-1.0), Some(4_000.0));
    }

    #[test]
    fn reset_discards_previous_crossing() {
        let mut counter = ZeroCrossingFrequency::new(8_000.0).unwrap();
        counter.process_sample(-1.0);
        counter.process_sample(1.0);
        counter.reset();
        assert_eq!(counter.process_sample(-1.0), None);
        assert_eq!(counter.process_sample(1.0), None);
    }
}
