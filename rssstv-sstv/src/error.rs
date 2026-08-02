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
    /// The image dimensions do not match the mode's transport dimensions.
    #[error("transmit image size mismatch: expected {expected:?}, got {actual:?}")]
    TxImageSizeMismatch {
        /// Required transport dimensions.
        expected: ImageSize,
        /// Supplied image dimensions.
        actual: ImageSize,
    },
}
