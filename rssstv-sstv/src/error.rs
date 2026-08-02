use thiserror::Error;

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
}
