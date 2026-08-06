//! Streaming conversion of timed SSTV tones to normalized PCM samples.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

use rssstv_dsp::oscillator::Vco;
use rssstv_sstv::{signal::TimedTone, time::PICOS_PER_SECOND};
use thiserror::Error;

/// Errors produced while configuring or processing a tone stream.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ModulatorError {
    /// The sample rate is too low to construct an oscillator.
    #[error("sample rate must be at least 2 Hz")]
    InvalidSampleRate,
    /// A tone frequency is at or above the Nyquist frequency.
    #[error("tone frequency must be below the Nyquist frequency")]
    InvalidFrequency,
    /// A tone deadline is not later than the preceding deadline.
    #[error("tone deadlines must be strictly increasing")]
    NonIncreasingDeadline,
    /// A deadline could not be converted to an absolute sample endpoint.
    #[error("tone deadline exceeds the supported sample range")]
    DeadlineOverflow,
}

/// A pull-based, phase-continuous timed-tone PCM modulator.
pub struct Modulator<I>
where
    I: Iterator<Item = TimedTone>,
{
    tones: I,
    oscillator: Vco,
    sample_rate_hz: u32,
    sample_position: u128,
    tone_endpoint: u128,
    previous_deadline_picos: u64,
    current_frequency_hz: u32,
    has_tone: bool,
    exhausted: bool,
}

impl<I> Modulator<I>
where
    I: Iterator<Item = TimedTone>,
{
    /// Creates a modulator that consumes `tones` at the given sample rate.
    pub fn new(tones: I, sample_rate_hz: u32) -> Result<Self, ModulatorError> {
        if sample_rate_hz < 2 {
            return Err(ModulatorError::InvalidSampleRate);
        }

        let oscillator = Vco::new(f64::from(sample_rate_hz), 0.0, 0.0)
            .map_err(|_| ModulatorError::InvalidSampleRate)?;
        Ok(Self {
            tones,
            oscillator,
            sample_rate_hz,
            sample_position: 0,
            tone_endpoint: 0,
            previous_deadline_picos: 0,
            current_frequency_hz: 0,
            has_tone: false,
            exhausted: false,
        })
    }

    /// Fills as much of `output` as the remaining tone stream permits.
    ///
    /// The returned count is the initialized prefix length. Once the stream is
    /// exhausted, subsequent calls return zero, including for nonempty buffers.
    pub fn process(&mut self, output: &mut [f32]) -> Result<usize, ModulatorError> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.exhausted {
            return Ok(0);
        }

        let mut written = 0;
        while written < output.len() {
            if !self.has_tone || self.sample_position >= self.tone_endpoint {
                if !self.load_next_tone()? {
                    break;
                }
                if self.sample_position >= self.tone_endpoint {
                    continue;
                }
            }

            let remaining = self.tone_endpoint - self.sample_position;
            let count = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(output.len() - written);
            let block = &mut output[written..written + count];
            if self.current_frequency_hz == 0 {
                block.fill(0.0);
            } else {
                for sample in block {
                    *sample = self
                        .oscillator
                        .process_sample(0.0)
                        .map_err(|_| ModulatorError::InvalidFrequency)?
                        as f32;
                }
            }
            self.sample_position = self
                .sample_position
                .checked_add(count as u128)
                .ok_or(ModulatorError::DeadlineOverflow)?;
            written += count;
        }
        Ok(written)
    }

    fn load_next_tone(&mut self) -> Result<bool, ModulatorError> {
        let Some(tone) = self.tones.next() else {
            self.has_tone = false;
            self.exhausted = true;
            return Ok(false);
        };

        let deadline_picos = tone.until().as_picos();
        if deadline_picos <= self.previous_deadline_picos {
            return Err(ModulatorError::NonIncreasingDeadline);
        }
        let frequency_hz = tone.frequency().as_hz();
        if u64::from(frequency_hz) * 2 >= u64::from(self.sample_rate_hz) && frequency_hz != 0 {
            return Err(ModulatorError::InvalidFrequency);
        }

        let scaled = u128::from(deadline_picos)
            .checked_mul(u128::from(self.sample_rate_hz))
            .ok_or(ModulatorError::DeadlineOverflow)?;
        self.tone_endpoint = scaled
            .checked_add(PICOS_PER_SECOND - 1)
            .ok_or(ModulatorError::DeadlineOverflow)?
            / PICOS_PER_SECOND;
        if frequency_hz != 0 {
            self.oscillator
                .set_free_frequency(f64::from(frequency_hz))
                .map_err(|_| ModulatorError::InvalidFrequency)?;
        }
        self.previous_deadline_picos = deadline_picos;
        self.current_frequency_hz = frequency_hz;
        self.has_tone = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::{vec, vec::Vec};
    use rssstv_sstv::{
        signal::{Frequency, TxComponent},
        time::TxInstant,
    };
    use rstest::rstest;

    fn tone(frequency_hz: u32, until_picos: u64) -> TimedTone {
        TimedTone::new(
            TxComponent::Luminance,
            Frequency::from_hz(frequency_hz),
            TxInstant::from_picos(until_picos),
        )
    }

    fn render(tones: &[TimedTone], chunk_size: usize) -> Vec<f32> {
        let mut modulator = Modulator::new(tones.iter().copied(), 8_000).unwrap();
        let mut result = Vec::new();
        loop {
            let mut block = vec![f32::NAN; chunk_size];
            let count = modulator.process(&mut block).unwrap();
            result.extend_from_slice(&block[..count]);
            if count == 0 {
                return result;
            }
        }
    }

    #[rstest]
    #[case(1)]
    #[case(3)]
    #[case(17)]
    fn output_is_independent_of_chunk_size(#[case] chunk_size: usize) {
        let tones = [
            tone(1_000, 1_500_000_000),
            tone(1_500, 3_125_000_000),
            tone(0, 4_250_000_000),
            tone(500, 6_000_000_000),
        ];
        assert_eq!(render(&tones, chunk_size), render(&tones, 64));
    }

    #[test]
    fn absolute_deadlines_use_ceil_without_duration_rounding_drift() {
        let tones = [
            tone(0, 125_000_000),
            tone(1_000, 250_000_001),
            tone(0, 375_000_001),
        ];
        let output = render(&tones, 16);
        assert_eq!(output.len(), 4);
        assert_eq!(output[0], 0.0);
        assert_ne!(output[1], 0.0);
        assert_ne!(output[2], 0.0);
        assert_eq!(output[3], 0.0);
    }

    #[test]
    fn generated_samples_match_a_phase_continuous_vco() {
        let tones = [tone(1_000, 500_000_000), tone(1_500, 1_000_000_000)];
        let actual = render(&tones, 8);
        let mut expected_vco = Vco::new(8_000.0, 1_000.0, 0.0).unwrap();
        let mut expected = Vec::new();
        for _ in 0..4 {
            expected.push(expected_vco.process_sample(0.0).unwrap() as f32);
        }
        expected_vco.set_free_frequency(1_500.0).unwrap();
        for _ in 0..4 {
            expected.push(expected_vco.process_sample(0.0).unwrap() as f32);
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn silence_is_exact_and_does_not_advance_phase() {
        let with_silence = [
            tone(1_000, 250_000_000),
            tone(0, 750_000_000),
            tone(1_000, 1_000_000_000),
        ];
        let output = render(&with_silence, 16);
        assert_eq!(&output[2..6], &[0.0; 4]);

        let uninterrupted_phase = render(&[tone(1_000, 500_000_000)], 16);
        assert_eq!(&output[..2], &uninterrupted_phase[..2]);
        assert_eq!(&output[6..], &uninterrupted_phase[2..]);
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    fn invalid_sample_rate_is_rejected(#[case] sample_rate_hz: u32) {
        assert!(matches!(
            Modulator::new(core::iter::empty(), sample_rate_hz),
            Err(ModulatorError::InvalidSampleRate)
        ));
    }

    #[rstest]
    #[case(4_000)]
    #[case(4_001)]
    fn nyquist_and_higher_frequencies_are_rejected(#[case] frequency_hz: u32) {
        let mut modulator = Modulator::new([tone(frequency_hz, 1)].into_iter(), 8_000).unwrap();
        assert_eq!(
            modulator.process(&mut [0.0]),
            Err(ModulatorError::InvalidFrequency)
        );
    }

    #[test]
    fn non_increasing_deadlines_are_rejected() {
        let tones = [tone(1_000, 1_000_000_000), tone(1_000, 1_000_000_000)];
        let mut modulator = Modulator::new(tones.into_iter(), 8_000).unwrap();
        let mut output = [0.0; 9];
        assert_eq!(
            modulator.process(&mut output),
            Err(ModulatorError::NonIncreasingDeadline)
        );
    }

    #[test]
    fn empty_input_is_immediately_exhausted() {
        let mut modulator = Modulator::new(core::iter::empty(), 8_000).unwrap();
        assert_eq!(modulator.process(&mut [1.0; 4]), Ok(0));
        assert_eq!(modulator.process(&mut [1.0]), Ok(0));
    }
}
