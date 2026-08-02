use crate::DspError;
use crate::iir::{IirFilter, IirLowPassDesign, IirResponse};
use crate::oscillator::Vco;

/// Peak-to-peak amplitude the input automatic gain control aims for.
const AGC_TARGET_SPAN: f64 = 5.0;
/// Smallest measured peak-to-peak span used by the gain calculation.
///
/// MMSSTV seeds its cycle extremes at plus and minus one full-scale unit, which
/// bounds the gain applied to near-silent input. This constant is the same
/// guard expressed for input normalized to `-1.0..=1.0`.
const AGC_MIN_SPAN: f64 = 1.0e-4;
/// Saturation applied to the loop filter output, in control units.
const CONTROL_LIMIT: f64 = 1.5;

/// Parameters for a multiplier phase-locked frequency discriminator.
///
/// Defaults follow MMSSTV's ordinary image band: a 1500-2300 Hz range, a
/// first-order 1500 Hz loop filter, and a third-order 900 Hz output filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PllDesign {
    /// Sampling frequency in hertz.
    pub sample_rate_hz: f64,
    /// Lowest tracked frequency in hertz.
    pub lower_frequency_hz: f64,
    /// Highest tracked frequency in hertz.
    pub upper_frequency_hz: f64,
    /// Loop low-pass filter order.
    pub loop_order: usize,
    /// Loop low-pass cutoff in hertz.
    pub loop_cutoff_hz: f64,
    /// Output low-pass filter order.
    pub output_order: usize,
    /// Output low-pass cutoff in hertz.
    pub output_cutoff_hz: f64,
}

impl PllDesign {
    /// Returns MMSSTV's wide-band image discriminator design.
    pub const fn wide(sample_rate_hz: f64) -> Self {
        Self {
            sample_rate_hz,
            lower_frequency_hz: 1_500.0,
            upper_frequency_hz: 2_300.0,
            loop_order: 1,
            loop_cutoff_hz: 1_500.0,
            output_order: 3,
            output_cutoff_hz: 900.0,
        }
    }

    /// Returns MMSSTV's narrow-band image discriminator design.
    pub const fn narrow(sample_rate_hz: f64) -> Self {
        Self {
            lower_frequency_hz: 2_044.0,
            upper_frequency_hz: 2_300.0,
            ..Self::wide(sample_rate_hz)
        }
    }
}

/// A multiplier phase-locked loop used as an FM discriminator.
///
/// The oscillator control is derived from the previous sample's phase error, so
/// the loop carries MMSSTV's one-sample delay. [`Pll::process_sample`] reports
/// the estimated input frequency in hertz.
#[derive(Clone, Debug)]
pub struct Pll {
    vco: Vco,
    loop_filter: IirFilter,
    output_filter: IirFilter,
    free_frequency_hz: f64,
    shift_hz: f64,
    phase_error: f64,
    control: f64,
    vco_output: f64,
    cycle_maximum: f64,
    cycle_minimum: f64,
    previous_sample: f64,
    previous_gain: f64,
    gain: f64,
}

impl Pll {
    /// Designs and creates a discriminator.
    pub fn new(design: PllDesign) -> Result<Self, DspError> {
        if design.lower_frequency_hz >= design.upper_frequency_hz {
            return Err(DspError::InvalidFrequency);
        }
        let free_frequency_hz = (design.lower_frequency_hz + design.upper_frequency_hz) * 0.5;
        let shift_hz = design.upper_frequency_hz - design.lower_frequency_hz;
        let low_pass = |order, cutoff_hz| {
            IirFilter::from_low_pass(IirLowPassDesign {
                order,
                sample_rate_hz: design.sample_rate_hz,
                cutoff_hz,
                response: IirResponse::Butterworth,
            })
        };
        Ok(Self {
            // A negative control gain matches MMSSTV: a rising control lowers
            // the oscillator, so a rising output corresponds to a rising input.
            vco: Vco::new(design.sample_rate_hz, free_frequency_hz, -shift_hz)?,
            loop_filter: low_pass(design.loop_order, design.loop_cutoff_hz)?,
            output_filter: low_pass(design.output_order, design.output_cutoff_hz)?,
            free_frequency_hz,
            shift_hz,
            phase_error: 0.0,
            control: 0.0,
            vco_output: 0.0,
            cycle_maximum: 0.0,
            cycle_minimum: 0.0,
            previous_sample: 0.0,
            previous_gain: 0.0,
            gain: AGC_TARGET_SPAN / AGC_MIN_SPAN,
        })
    }

    /// Returns the center of the tracked range in hertz.
    pub fn free_frequency_hz(&self) -> f64 {
        self.free_frequency_hz
    }

    /// Returns the width of the tracked range in hertz.
    pub fn shift_hz(&self) -> f64 {
        self.shift_hz
    }

    /// Returns the most recent unfiltered phase-detector output.
    pub fn phase_error(&self) -> f64 {
        self.phase_error
    }

    /// Returns the most recent saturated oscillator control value.
    pub fn control(&self) -> f64 {
        self.control
    }

    /// Returns the most recent oscillator sample.
    pub fn vco_output(&self) -> f64 {
        self.vco_output
    }

    /// Retunes the tracked range while preserving loop and filter state.
    pub fn set_frequency_range(
        &mut self,
        lower_frequency_hz: f64,
        upper_frequency_hz: f64,
    ) -> Result<(), DspError> {
        if lower_frequency_hz >= upper_frequency_hz {
            return Err(DspError::InvalidFrequency);
        }
        let free_frequency_hz = (lower_frequency_hz + upper_frequency_hz) * 0.5;
        let shift_hz = upper_frequency_hz - lower_frequency_hz;
        self.vco.set_free_frequency(free_frequency_hz)?;
        self.vco.set_control_gain(-shift_hz)?;
        self.free_frequency_hz = free_frequency_hz;
        self.shift_hz = shift_hz;
        Ok(())
    }

    /// Accepts one sample in `-1.0..=1.0` and returns the estimated frequency in hertz.
    pub fn process_sample(&mut self, sample: f64) -> Result<f64, DspError> {
        let normalized = sample * self.update_gain(sample);
        self.control = self
            .loop_filter
            .process_sample(self.phase_error)
            .clamp(-CONTROL_LIMIT, CONTROL_LIMIT);
        self.vco_output = self.vco.process_sample(self.control)?;
        self.phase_error = self.vco_output * normalized;
        let filtered = self.output_filter.process_sample(self.control);
        Ok(self.free_frequency_hz - filtered * self.shift_hz)
    }

    /// Clears loop, filter, and gain state without changing the design.
    pub fn reset(&mut self) {
        self.loop_filter.reset();
        self.output_filter.reset();
        self.vco.reset_phase();
        self.phase_error = 0.0;
        self.control = 0.0;
        self.vco_output = 0.0;
        self.cycle_maximum = 0.0;
        self.cycle_minimum = 0.0;
        self.previous_sample = 0.0;
        self.previous_gain = 0.0;
        self.gain = AGC_TARGET_SPAN / AGC_MIN_SPAN;
    }

    /// Recalculates the input gain once per input cycle.
    fn update_gain(&mut self, sample: f64) -> f64 {
        self.cycle_maximum = self.cycle_maximum.max(sample);
        self.cycle_minimum = self.cycle_minimum.min(sample);
        if sample >= 0.0 && self.previous_sample < 0.0 {
            let span = (self.cycle_maximum - self.cycle_minimum).max(AGC_MIN_SPAN);
            let measured = AGC_TARGET_SPAN / span;
            // Averaging with the previous cycle keeps a single loud or quiet
            // cycle from stepping the loop gain.
            self.gain = (self.previous_gain + measured) * 0.5;
            self.previous_gain = measured;
            self.cycle_maximum = 0.0;
            self.cycle_minimum = 0.0;
        }
        self.previous_sample = sample;
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::f64::consts::TAU;

    use rstest::rstest;

    use super::*;

    const SAMPLE_RATE: f64 = 11_025.0;

    fn settled_estimate(pll: &mut Pll, frequency_hz: f64, phase: f64, count: usize) -> f64 {
        let mut estimates = Vec::new();
        for index in 0..count {
            let sample = libm::sin(TAU * frequency_hz * index as f64 / SAMPLE_RATE + phase);
            let estimate = pll.process_sample(sample).unwrap();
            if index >= count / 2 {
                estimates.push(estimate);
            }
        }
        estimates.iter().sum::<f64>() / estimates.len() as f64
    }

    #[rstest]
    #[case(1_500.0)]
    #[case(1_900.0)]
    #[case(2_300.0)]
    fn locks_onto_tones_across_the_image_band(#[case] frequency_hz: f64) {
        let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
        let estimate = settled_estimate(&mut pll, frequency_hz, 0.31, 8_192);
        assert!(
            (estimate - frequency_hz).abs() < 10.0,
            "{frequency_hz} estimated as {estimate}"
        );
    }

    #[test]
    fn amplitude_does_not_change_the_reported_frequency() {
        let quiet = {
            let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
            let mut estimate = 0.0;
            for index in 0..8_192 {
                let sample = 0.02 * libm::sin(TAU * 2_100.0 * index as f64 / SAMPLE_RATE);
                estimate = pll.process_sample(sample).unwrap();
            }
            estimate
        };
        let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
        let loud = settled_estimate(&mut pll, 2_100.0, 0.0, 8_192);
        assert!((quiet - loud).abs() < 20.0, "quiet={quiet} loud={loud}");
    }

    #[test]
    fn tracks_a_frequency_step_within_one_line() {
        let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
        let mut phase = 0.0;
        let mut estimate = 0.0;
        for index in 0..4_096 {
            let frequency_hz = if index < 2_048 { 1_600.0 } else { 2_200.0 };
            phase += TAU * frequency_hz / SAMPLE_RATE;
            estimate = pll.process_sample(libm::sin(phase)).unwrap();
        }
        assert!((estimate - 2_200.0).abs() < 25.0, "estimate={estimate}");
    }

    #[test]
    fn reset_reproduces_the_initial_response() {
        let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
        let expected = settled_estimate(&mut pll, 1_700.0, 0.0, 512);
        settled_estimate(&mut pll, 2_300.0, 1.0, 512);
        pll.reset();
        assert_eq!(settled_estimate(&mut pll, 1_700.0, 0.0, 512), expected);
    }

    #[test]
    fn narrow_range_is_centered_on_the_narrow_band() {
        let pll = Pll::new(PllDesign::narrow(SAMPLE_RATE)).unwrap();
        assert_eq!(pll.free_frequency_hz(), 2_172.0);
        assert_eq!(pll.shift_hz(), 256.0);
    }

    #[test]
    fn retuning_preserves_state_and_invalid_ranges_are_rejected() {
        let mut pll = Pll::new(PllDesign::wide(SAMPLE_RATE)).unwrap();
        settled_estimate(&mut pll, 1_900.0, 0.0, 512);
        let control = pll.control();
        pll.set_frequency_range(2_044.0, 2_300.0).unwrap();
        assert_eq!(pll.control(), control);
        assert_eq!(pll.free_frequency_hz(), 2_172.0);
        assert_eq!(
            pll.set_frequency_range(2_300.0, 2_044.0),
            Err(DspError::InvalidFrequency)
        );
    }

    #[rstest]
    #[case(PllDesign { lower_frequency_hz: 2_300.0, ..PllDesign::wide(SAMPLE_RATE) }, DspError::InvalidFrequency)]
    #[case(PllDesign { sample_rate_hz: 0.0, ..PllDesign::wide(SAMPLE_RATE) }, DspError::InvalidSampleRate)]
    #[case(PllDesign { loop_order: 0, ..PllDesign::wide(SAMPLE_RATE) }, DspError::InvalidOrder)]
    #[case(PllDesign { output_cutoff_hz: SAMPLE_RATE, ..PllDesign::wide(SAMPLE_RATE) }, DspError::InvalidFrequency)]
    fn invalid_designs_are_typed(#[case] design: PllDesign, #[case] expected: DspError) {
        assert_eq!(Pll::new(design).err(), Some(expected));
    }
}
