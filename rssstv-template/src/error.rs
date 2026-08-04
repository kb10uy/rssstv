use thiserror::Error;

/// A failure reported by a caller-provided asset source.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct AssetError {
    message: String,
}

impl AssetError {
    /// Creates an asset error with a caller-facing message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A template parsing, evaluation, or rendering failure.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// The KDL document is not syntactically valid.
    #[error("invalid KDL template: {0}")]
    Kdl(#[from] kdl::KdlError),
    /// The document violates the RSSSTV template schema.
    #[error("invalid template: {0}")]
    Schema(String),
    /// A text expression references a value absent from the render context.
    #[error("template variable `{0}` was not provided")]
    MissingVariable(String),
    /// A text expression asks for a format the referenced value cannot take.
    #[error("cannot format template variable `{name}`: {message}")]
    VariableFormat {
        /// Referenced variable name.
        name: String,
        /// Why the format was rejected.
        message: String,
    },
    /// An `rximage` layer has no caller-provided received image.
    #[error("template contains rximage but no received image was provided")]
    MissingReceivedImage,
    /// An image asset could not be found.
    #[error("template asset `{0}` was not found")]
    MissingAsset(String),
    /// An image asset source failed.
    #[error("failed to load template asset `{reference}`: {source}")]
    Asset {
        /// Requested template reference.
        reference: String,
        /// Error returned by the provider.
        #[source]
        source: AssetError,
    },
    /// A requested font family is not registered with the renderer.
    #[error("template font family `{0}` is not available")]
    MissingFont(String),
    /// PNG encoding or decoding failed.
    #[error("template image processing failed: {0}")]
    Image(#[from] image::ImageError),
    /// SVG parsing failed.
    #[error("generated SVG is invalid: {0}")]
    Svg(#[from] resvg::usvg::Error),
    /// An allocation or raster target dimension is invalid.
    #[error("invalid render dimensions: {0}")]
    InvalidDimensions(String),
    /// The background and overlay dimensions differ.
    #[error("background and overlay dimensions differ")]
    ImageSizeMismatch,
    /// Construction of an SSTV RGB image failed.
    #[error(transparent)]
    Sstv(#[from] rssstv_sstv::SstvError),
}
