use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DspError {
    InvalidAttenuation,
    InvalidBandwidth,
    InvalidCoefficientCount,
    InvalidFrequency,
    InvalidGain,
    InvalidOrder,
    InvalidRipple,
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
