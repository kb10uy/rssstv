use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// An invalid DSP design or processor configuration.
pub enum DspError {
    /// Filter attenuation is negative or not finite.
    InvalidAttenuation,
    /// Resonator bandwidth is negative or not finite.
    InvalidBandwidth,
    /// A coefficient array is empty, non-finite, or has an unexpected length.
    InvalidCoefficientCount,
    /// A frequency is non-finite or outside its valid Nyquist interval.
    InvalidFrequency,
    /// Filter gain is not finite.
    InvalidGain,
    /// Filter order is zero or unsupported by the selected design.
    InvalidOrder,
    /// Chebyshev passband ripple is non-positive or not finite.
    InvalidRipple,
    /// Sample rate is non-positive or not finite.
    InvalidSampleRate,
}

impl fmt::Display for DspError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidAttenuation => "attenuation must be finite and non-negative",
            Self::InvalidBandwidth => "bandwidth must be finite and non-negative",
            Self::InvalidCoefficientCount => "coefficient count does not match the filter order",
            Self::InvalidFrequency => "frequency must be finite and within the Nyquist interval",
            Self::InvalidGain => "gain must be finite",
            Self::InvalidOrder => "filter order is invalid",
            Self::InvalidRipple => "passband ripple must be finite and positive",
            Self::InvalidSampleRate => "sample rate must be finite and positive",
        };
        formatter.write_str(message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DspError {}
