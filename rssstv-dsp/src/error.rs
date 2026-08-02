use thiserror::Error;

/// An invalid DSP design or processor configuration.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DspError {
    /// Filter attenuation is negative or not finite.
    #[error("attenuation must be finite and non-negative")]
    InvalidAttenuation,
    /// Resonator bandwidth is negative or not finite.
    #[error("bandwidth must be finite and non-negative")]
    InvalidBandwidth,
    /// A buffer length does not match the length required by the processor.
    #[error("buffer length does not match the required length")]
    InvalidBufferLength,
    /// A coefficient array is empty, non-finite, or has an unexpected length.
    #[error("coefficient count does not match the filter order")]
    InvalidCoefficientCount,
    /// A frequency is non-finite or outside its valid Nyquist interval.
    #[error("frequency must be finite and within the Nyquist interval")]
    InvalidFrequency,
    /// Filter gain is not finite.
    #[error("gain must be finite")]
    InvalidGain,
    /// Filter order is zero or unsupported by the selected design.
    #[error("filter order is invalid")]
    InvalidOrder,
    /// Chebyshev passband ripple is non-positive or not finite.
    #[error("passband ripple must be finite and positive")]
    InvalidRipple,
    /// Sample rate is non-positive or not finite.
    #[error("sample rate must be finite and positive")]
    InvalidSampleRate,
    /// A transform length is not a power of two of at least two.
    #[error("transform length must be a power of two of at least two")]
    InvalidTransformLength,
}
