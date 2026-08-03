use alloc::{vec, vec::Vec};
use core::{
    f64::consts::TAU,
    ops::{Add, Mul, Sub},
};

use crate::DspError;

/// A complex value in rectangular form.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// The additive identity.
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };

    /// Constructs a complex value from its parts.
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// Constructs a complex value with a zero imaginary part.
    pub const fn real(re: f64) -> Self {
        Self { re, im: 0.0 }
    }

    /// Returns the complex conjugate.
    pub const fn conjugate(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }

    /// Returns the squared magnitude, avoiding a square root.
    pub const fn norm_squared(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    /// Returns the magnitude.
    pub fn magnitude(self) -> f64 {
        libm::sqrt(self.norm_squared())
    }

    /// Returns the argument in radians over `-PI..=PI`.
    pub fn argument(self) -> f64 {
        libm::atan2(self.im, self.re)
    }
}

impl Add for Complex {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.re + rhs.re, self.im + rhs.im)
    }
}

impl Sub for Complex {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.re - rhs.re, self.im - rhs.im)
    }
}

impl Mul for Complex {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.re * rhs.re - self.im * rhs.im,
            self.re * rhs.im + self.im * rhs.re,
        )
    }
}

impl Mul<f64> for Complex {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.re * rhs, self.im * rhs)
    }
}

/// Transform direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FftDirection {
    /// Uses the `exp(-i*2*PI*k*n/N)` kernel without scaling.
    Forward,
    /// Uses the `exp(+i*2*PI*k*n/N)` kernel and scales the result by `1/N`.
    Inverse,
}

/// A radix-2 in-place complex transform with precomputed tables.
///
/// The plan owns its twiddle factors and bit-reversal permutation, so applying
/// it does not allocate.
#[derive(Clone, Debug)]
pub struct Fft {
    length: usize,
    twiddles: Vec<Complex>,
    reversal: Vec<usize>,
}

impl Fft {
    /// Creates a plan for a power-of-two length of at least two.
    pub fn new(length: usize) -> Result<Self, DspError> {
        if length < 2 || !length.is_power_of_two() {
            return Err(DspError::InvalidTransformLength);
        }
        let mut twiddles = Vec::with_capacity(length / 2);
        for index in 0..length / 2 {
            let angle = -TAU * index as f64 / length as f64;
            twiddles.push(Complex::new(libm::cos(angle), libm::sin(angle)));
        }
        let bits = length.trailing_zeros();
        let reversal = (0..length)
            .map(|index| index.reverse_bits() >> (usize::BITS - bits))
            .collect();
        Ok(Self {
            length,
            twiddles,
            reversal,
        })
    }

    /// Returns the transform length.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Returns whether the transform length is zero, which never happens.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Transforms `buffer` in place.
    pub fn transform(
        &self,
        buffer: &mut [Complex],
        direction: FftDirection,
    ) -> Result<(), DspError> {
        if buffer.len() != self.length {
            return Err(DspError::InvalidBufferLength);
        }
        for (index, &target) in self.reversal.iter().enumerate() {
            if index < target {
                buffer.swap(index, target);
            }
        }
        let mut half = 1;
        while half < self.length {
            // Twiddles are stored for the largest stage, so a stride selects
            // the subset a smaller stage needs.
            let stride = self.length / (half * 2);
            for start in (0..self.length).step_by(half * 2) {
                for offset in 0..half {
                    let twiddle = self.twiddles[offset * stride];
                    let twiddle = match direction {
                        FftDirection::Forward => twiddle,
                        FftDirection::Inverse => twiddle.conjugate(),
                    };
                    let even = buffer[start + offset];
                    let odd = buffer[start + offset + half] * twiddle;
                    buffer[start + offset] = even + odd;
                    buffer[start + offset + half] = even - odd;
                }
            }
            half *= 2;
        }
        if direction == FftDirection::Inverse {
            let scale = 1.0 / self.length as f64;
            for value in buffer {
                *value = *value * scale;
            }
        }
        Ok(())
    }
}

/// An analysis window applied before a real-input transform.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SpectrumWindow {
    /// No weighting.
    #[default]
    Rectangular,
    /// Periodic Hann window, matching MMSSTV's spectrum display.
    Hann,
}

impl SpectrumWindow {
    fn weight(self, index: usize, length: usize) -> f64 {
        match self {
            Self::Rectangular => 1.0,
            Self::Hann => 0.5 - 0.5 * libm::cos(TAU * index as f64 / length as f64),
        }
    }
}

/// A windowed real-input spectrum analyzer with an owned working buffer.
#[derive(Clone, Debug)]
pub struct RealSpectrum {
    fft: Fft,
    window: Vec<f64>,
    buffer: Vec<Complex>,
}

impl RealSpectrum {
    /// Creates an analyzer for `length` real samples.
    pub fn new(length: usize, window: SpectrumWindow) -> Result<Self, DspError> {
        let fft = Fft::new(length)?;
        Ok(Self {
            fft,
            window: (0..length)
                .map(|index| window.weight(index, length))
                .collect(),
            buffer: vec![Complex::ZERO; length],
        })
    }

    /// Returns the accepted input length.
    pub fn len(&self) -> usize {
        self.fft.len()
    }

    /// Returns whether the input length is zero, which never happens.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Returns the number of produced bins, `length / 2 + 1`.
    pub fn bin_count(&self) -> usize {
        self.fft.len() / 2 + 1
    }

    /// Returns the center frequency of `bin` in hertz.
    pub fn bin_frequency_hz(&self, bin: usize, sample_rate_hz: f64) -> f64 {
        sample_rate_hz * bin as f64 / self.fft.len() as f64
    }

    /// Windows and transforms `samples`, returning bins zero through Nyquist.
    pub fn transform(&mut self, samples: &[f64]) -> Result<&[Complex], DspError> {
        if samples.len() != self.fft.len() {
            return Err(DspError::InvalidBufferLength);
        }
        for ((slot, &sample), &weight) in self.buffer.iter_mut().zip(samples).zip(&self.window) {
            *slot = Complex::real(sample * weight);
        }
        self.fft
            .transform(&mut self.buffer, FftDirection::Forward)?;
        Ok(&self.buffer[..self.fft.len() / 2 + 1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn discrete_transform(input: &[Complex]) -> Vec<Complex> {
        (0..input.len())
            .map(|bin| {
                input
                    .iter()
                    .enumerate()
                    .fold(Complex::ZERO, |sum, (index, value)| {
                        let angle = -TAU * bin as f64 * index as f64 / input.len() as f64;
                        sum + *value * Complex::new(libm::cos(angle), libm::sin(angle))
                    })
            })
            .collect()
    }

    #[rstest]
    #[case(2)]
    #[case(8)]
    #[case(64)]
    fn matches_the_direct_transform(#[case] length: usize) {
        let input: Vec<_> = (0..length)
            .map(|index| {
                Complex::new(
                    libm::sin(index as f64 * 0.7) + 0.25,
                    libm::cos(index as f64),
                )
            })
            .collect();
        let expected = discrete_transform(&input);
        let mut actual = input;
        Fft::new(length)
            .unwrap()
            .transform(&mut actual, FftDirection::Forward)
            .unwrap();
        for (left, right) in actual.iter().zip(&expected) {
            assert!((left.re - right.re).abs() < 1e-10, "{left:?} {right:?}");
            assert!((left.im - right.im).abs() < 1e-10, "{left:?} {right:?}");
        }
    }

    #[test]
    fn inverse_restores_the_original_signal() {
        let fft = Fft::new(32).unwrap();
        let original: Vec<_> = (0..32)
            .map(|index| Complex::new(index as f64 * 0.125, -(index as f64) * 0.0625))
            .collect();
        let mut buffer = original.clone();
        fft.transform(&mut buffer, FftDirection::Forward).unwrap();
        fft.transform(&mut buffer, FftDirection::Inverse).unwrap();
        for (left, right) in buffer.iter().zip(&original) {
            assert!((left.re - right.re).abs() < 1e-12);
            assert!((left.im - right.im).abs() < 1e-12);
        }
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(6)]
    fn invalid_transform_lengths_are_rejected(#[case] length: usize) {
        assert_eq!(
            Fft::new(length).err(),
            Some(DspError::InvalidTransformLength)
        );
    }

    #[test]
    fn buffer_length_must_match_the_plan() {
        let mut buffer = [Complex::ZERO; 3];
        assert_eq!(
            Fft::new(4)
                .unwrap()
                .transform(&mut buffer, FftDirection::Forward),
            Err(DspError::InvalidBufferLength)
        );
    }

    #[test]
    fn real_spectrum_locates_a_bin_centered_tone() {
        let length = 1_024;
        let sample_rate_hz = 8_000.0;
        let bin = 243;
        let frequency_hz = sample_rate_hz * bin as f64 / length as f64;
        let samples: Vec<_> = (0..length)
            .map(|index| libm::sin(TAU * frequency_hz * index as f64 / sample_rate_hz))
            .collect();
        let mut spectrum = RealSpectrum::new(length, SpectrumWindow::Hann).unwrap();
        let bins = spectrum.transform(&samples).unwrap();
        assert_eq!(bins.len(), length / 2 + 1);
        let peak = bins
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.norm_squared().total_cmp(&right.1.norm_squared()))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, bin);
        assert!(
            (spectrum.bin_frequency_hz(peak, sample_rate_hz) - frequency_hz).abs() < f64::EPSILON
        );
    }

    #[test]
    fn hann_window_suppresses_leakage_from_an_off_bin_tone() {
        let length = 256;
        let frequency = 10.5;
        let samples: Vec<_> = (0..length)
            .map(|index| libm::sin(TAU * frequency * index as f64 / length as f64))
            .collect();
        let far_bin = 40;
        let leakage = |window| {
            let mut spectrum = RealSpectrum::new(length, window).unwrap();
            spectrum.transform(&samples).unwrap()[far_bin].magnitude()
        };
        assert!(leakage(SpectrumWindow::Hann) < leakage(SpectrumWindow::Rectangular) * 0.05);
    }

    #[test]
    fn real_spectrum_rejects_a_mismatched_input_length() {
        let mut spectrum = RealSpectrum::new(8, SpectrumWindow::Rectangular).unwrap();
        assert_eq!(
            spectrum.transform(&[0.0; 7]),
            Err(DspError::InvalidBufferLength)
        );
    }
}
