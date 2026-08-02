//! Portable transmit-image templates for RSSSTV.
//!
//! This crate parses KDL templates and renders them as straight-alpha RGBA
//! overlays. Background preparation and SSTV encoding remain separate.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod error;
mod image;
mod parser;
mod renderer;
mod scene;

pub use error::{AssetError, TemplateError};
pub use image::{RenderSize, Rgba8, RgbaImage, composite};
pub use renderer::{AssetProvider, EmptyAssetProvider, EncodedAsset, RenderContext, Renderer};
pub use scene::{
    Anchor, Color, EllipseLayer, GroupLayer, ImageFit, ImageLayer, Layer, Length, LineLayer,
    ReceivedImageLayer, RectangleLayer, Template, TextLayer, VariableValue, Variables,
};
