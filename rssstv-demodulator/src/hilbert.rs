use std::f64::consts::{PI, TAU};

use rssstv_dsp::{
    filter::{Iir, IirLowPassDesign, IirResponse},
    transform::HilbertTransformer,
};

pub(crate) struct HilbertDiscriminator {
    transformer: HilbertTransformer,
    sample_rate_hz: f64,
    phase_history: [f64; 4],
    phase_history_len: usize,
    next_phase: usize,
    phase_lag: usize,
    output_filter: Iir,
    held_frequency: f64,
}

impl HilbertDiscriminator {
    pub(crate) fn new(sample_rate_hz: f64) -> Result<Self, rssstv_dsp::DspError> {
        let (order, phase_lag) = if sample_rate_hz < 16_000.0 {
            (12, 1)
        } else if sample_rate_hz < 40_000.0 {
            (24, 2)
        } else {
            (48, 4)
        };
        let upper_frequency_hz = sample_rate_hz * 0.5 - 100.0;
        Ok(Self {
            transformer: HilbertTransformer::new(sample_rate_hz, order, 100.0, upper_frequency_hz)?,
            sample_rate_hz,
            phase_history: [0.0; 4],
            phase_history_len: 0,
            next_phase: 0,
            phase_lag,
            output_filter: Iir::from_low_pass(IirLowPassDesign {
                order: 3,
                sample_rate_hz,
                cutoff_hz: 1_800.0_f64.min(sample_rate_hz * 0.45),
                response: IirResponse::Butterworth,
            })?,
            held_frequency: 1_900.0,
        })
    }

    pub(crate) fn process(&mut self, sample: f64) -> f64 {
        let analytic = self.transformer.process_sample(sample);
        let magnitude = analytic.in_phase.hypot(analytic.quadrature);
        let phase = analytic.quadrature.atan2(analytic.in_phase);
        let previous = (self.phase_history_len == self.phase_lag)
            .then_some(self.phase_history[self.next_phase]);
        self.phase_history[self.next_phase] = phase;
        self.next_phase = (self.next_phase + 1) % self.phase_lag;
        self.phase_history_len = (self.phase_history_len + 1).min(self.phase_lag);
        if let Some(previous) = previous
            && magnitude > 1.0e-8
        {
            let delta = (phase - previous + PI).rem_euclid(TAU) - PI;
            self.held_frequency = (delta.abs() * self.sample_rate_hz
                / (TAU * self.phase_lag as f64))
                .clamp(0.0, 3_000.0);
        }
        self.output_filter.process_sample(self.held_frequency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(8_000, 1)]
    #[case(11_025, 1)]
    #[case(16_000, 2)]
    #[case(22_050, 2)]
    #[case(40_000, 4)]
    #[case(48_000, 4)]
    fn hilbert_phase_lag_tracks_sample_rate(#[case] rate: u32, #[case] expected: usize) {
        let discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        assert_eq!(discriminator.phase_lag, expected);
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(16_000)]
    #[case(22_050)]
    #[case(40_000)]
    #[case(48_000)]
    fn hilbert_phase_lag_preserves_frequency_scale(#[case] rate: u32) {
        let expected = 1_900.0;
        let mut discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        for sample in 0..rate / 5 {
            let estimate = discriminator.process(phase.sin());
            phase = (phase + TAU * expected / f64::from(rate)).rem_euclid(TAU);
            if sample >= rate / 10 {
                sum += estimate;
            }
        }
        let estimate = sum / f64::from(rate / 10);
        assert!(
            (estimate - expected).abs() < 2.0,
            "{rate} Hz produced {estimate} Hz"
        );
    }

    #[rstest]
    #[case(1_500.0)]
    #[case(1_550.0)]
    #[case(1_900.0)]
    #[case(2_300.0)]
    fn hilbert_image_tones_have_low_residual_ripple(#[case] frequency: f64) {
        let rate = 48_000;
        let mut discriminator = HilbertDiscriminator::new(f64::from(rate)).unwrap();
        let mut phase = 0.0_f64;
        let mut sum = 0.0;
        let mut squared = 0.0;
        let mut count = 0.0;
        for sample in 0..rate / 2 {
            let estimate = discriminator.process(phase.sin());
            phase = (phase + TAU * frequency / f64::from(rate)).rem_euclid(TAU);
            if sample >= rate / 4 {
                sum += estimate;
                squared += estimate * estimate;
                count += 1.0;
            }
        }
        let mean = sum / count;
        let standard_deviation = (squared / count - mean * mean).sqrt();
        assert!(
            standard_deviation < 6.0,
            "{frequency} Hz residual ripple was {standard_deviation} Hz"
        );
    }
}
