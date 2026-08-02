use thiserror::Error;

use crate::image::ImageSize;
use crate::mode::Mode;

/// An invalid SSTV value or arithmetic operation.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SstvError {
    /// Image width or height is zero.
    #[error("image dimensions must be non-zero")]
    EmptyImage,
    /// Pixel storage length does not match the image dimensions.
    #[error("pixel count does not match image dimensions")]
    InvalidPixelCount,
    /// Image dimensions cannot be represented by the allocation size.
    #[error("image dimensions overflow the address space")]
    ImageSizeOverflow,
    /// Duration or deadline arithmetic overflowed.
    #[error("SSTV time arithmetic overflow")]
    TimeOverflow,
    /// The selected mode has no transmit encoder.
    #[error("transmit encoding is not implemented for {0:?}")]
    UnsupportedTxMode(Mode),
    /// The selected mode has no receive decoder.
    #[error("receive decoding is not implemented for {0:?}")]
    UnsupportedRxMode(Mode),
    /// Receive sample rates must be non-zero.
    #[error("receive sample rate must be non-zero")]
    InvalidSampleRate,
    /// Demodulated frequency and synchronization arrays have different lengths.
    #[error("demodulated frequency and synchronization lengths differ")]
    DemodulatedLengthMismatch,
    /// A demodulated value was non-finite or synchronization strength was not normalized.
    #[error("invalid demodulated value at block offset {offset}")]
    InvalidDemodulatedSample {
        /// Offset of the invalid value in the supplied block.
        offset: usize,
    },
    /// A block did not begin at the next required absolute sample.
    #[error("non-contiguous demodulated block: expected sample {expected}, got {actual}")]
    DemodulatedGap {
        /// Required first sample.
        expected: u64,
        /// Supplied first sample.
        actual: u64,
    },
    /// Absolute sample arithmetic overflowed.
    #[error("receive sample position overflow")]
    SamplePositionOverflow,
    /// No stable recurring raster synchronization was present in the acquisition window.
    #[error("stable raster synchronization was not acquired")]
    RasterNotAcquired,
    /// Immutable receive staging would exceed its configured hard limit.
    #[error("receive staging capacity exceeded (maximum {max_samples} samples)")]
    StagingCapacityExceeded {
        /// Configured maximum retained sample count.
        max_samples: usize,
    },
    /// Staged refinement was requested without enough accepted sync observations.
    #[error("insufficient staged synchronization observations")]
    InsufficientStagedSync,
    /// Staged samples do not cover every sample required by the fitted raster.
    #[error("staged receive data does not cover required sample {required_sample}")]
    InsufficientStagedData {
        /// First unavailable sample required to rebuild the image.
        required_sample: u64,
    },
    /// Staged refinement was requested while sample staging is disabled.
    #[error("receive sample staging is disabled")]
    StagingDisabled,
    /// Samples were supplied after decoding completed.
    #[error("receive decoder is already complete")]
    RxAlreadyComplete,
    /// Refinement-only samples were supplied before decoding completed.
    #[error("receive decoder is not complete")]
    RxNotComplete,
    /// The image dimensions do not match the mode's transport dimensions.
    #[error("transmit image size mismatch: expected {expected:?}, got {actual:?}")]
    TxImageSizeMismatch {
        /// Required transport dimensions.
        expected: ImageSize,
        /// Supplied image dimensions.
        actual: ImageSize,
    },
}

/// A receive processing failure with the consumed input prefix preserved.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("receive processing failed after consuming {consumed} samples: {error}")]
pub struct RxProcessError {
    consumed: usize,
    #[source]
    error: SstvError,
}

impl RxProcessError {
    pub(crate) const fn new(consumed: usize, error: SstvError) -> Self {
        Self { consumed, error }
    }

    /// Returns the consumed prefix length of the supplied block.
    pub const fn consumed(&self) -> usize {
        self.consumed
    }

    /// Returns the processing error.
    pub const fn error(&self) -> SstvError {
        self.error
    }
}
