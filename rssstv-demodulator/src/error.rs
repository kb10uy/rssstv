use thiserror::Error;

/// Failure while configuring or processing the receive front end.
#[derive(Debug, Error)]
pub enum DemodulatorError {
    /// The physical sample rate cannot represent the SSTV receive band.
    #[error("sample rate {0} Hz is too low for SSTV")]
    SampleRateTooLow(u32),
    /// A PCM sample was not finite.
    #[error("PCM sample {index} is not finite")]
    NonFiniteSample {
        /// Zero-based absolute PCM sample position.
        index: u64,
    },
    /// Processing was requested after the stream was finished.
    #[error("demodulator stream is already finished")]
    AlreadyFinished,
    /// The absolute PCM sample position overflowed.
    #[error("PCM sample position overflow")]
    SamplePositionOverflow,
    /// A DSP processor rejected its configuration.
    #[error(transparent)]
    Dsp(#[from] rssstv_dsp::DspError),
    /// No complete supported conventional VIS sequence was found.
    #[error("no supported conventional VIS code was detected")]
    VisNotDetected,
}
