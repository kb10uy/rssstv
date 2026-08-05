use std::{io::Cursor, sync::Arc};

use image::ImageEncoder;
use resvg::usvg;
use rssstv_sstv::image::RgbImage;

use crate::{AssetError, EncodedAsset, TemplateError, scene::Layer};

#[derive(Clone, Debug)]
pub(super) struct Resource {
    pub(super) data: Arc<Vec<u8>>,
    pub(super) format: AssetFormat,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// An encoded image format the SVG backend can embed as it stands.
#[derive(Clone, Copy, Debug)]
pub(super) enum AssetFormat {
    Png,
    Jpeg,
    WebP,
}

pub(super) fn validate_fonts(
    layers: &[Layer],
    database: &usvg::fontdb::Database,
) -> Result<(), TemplateError> {
    for layer in layers {
        match layer {
            Layer::Text(text) => {
                let available = database.faces().any(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(&text.font.family))
                });
                if !available {
                    return Err(TemplateError::MissingFont(text.font.family.clone()));
                }
            }
            Layer::Group(group) => validate_fonts(&group.layers, database)?,
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn validate_asset(
    asset: &EncodedAsset,
    reference: &str,
) -> Result<Resource, TemplateError> {
    if let Some(resource) = asset.validated.get() {
        return Ok(resource.clone());
    }
    let resource = decode_asset(Arc::clone(&asset.data), reference)?;
    let _ = asset.validated.set(resource.clone());
    Ok(resource)
}

fn decode_asset(data: Arc<Vec<u8>>, reference: &str) -> Result<Resource, TemplateError> {
    let reader = image::ImageReader::new(Cursor::new(data.as_slice()))
        .with_guessed_format()
        .map_err(|error| TemplateError::Asset {
            reference: reference.to_owned(),
            source: AssetError::new(error.to_string()),
        })?;
    let format = match reader.format() {
        Some(image::ImageFormat::Png) => AssetFormat::Png,
        Some(image::ImageFormat::Jpeg) => AssetFormat::Jpeg,
        Some(image::ImageFormat::WebP) => AssetFormat::WebP,
        Some(image::ImageFormat::Bmp) => return transcode_to_png(reader, reference),
        _ => return Err(TemplateError::UnsupportedAsset(reference.to_owned())),
    };
    let (width, height) = reader.into_dimensions()?;
    Ok(Resource {
        data,
        format,
        width,
        height,
    })
}

fn transcode_to_png(
    reader: image::ImageReader<Cursor<&[u8]>>,
    reference: &str,
) -> Result<Resource, TemplateError> {
    let decoded = reader.decode().map_err(|error| TemplateError::Asset {
        reference: reference.to_owned(),
        source: AssetError::new(error.to_string()),
    })?;
    let rgba = decoded.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut png)).write_image(
        rgba.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(Resource {
        data: Arc::new(png),
        format: AssetFormat::Png,
        width,
        height,
    })
}

pub(super) fn encode_received_image(image: &RgbImage) -> Result<Resource, TemplateError> {
    let width = u32::try_from(image.size().width()).map_err(|_| {
        TemplateError::InvalidDimensions("received image width exceeds PNG limits".into())
    })?;
    let height = u32::try_from(image.size().height()).map_err(|_| {
        TemplateError::InvalidDimensions("received image height exceeds PNG limits".into())
    })?;
    let rgb = image.to_rgb_bytes();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut png)).write_image(
        &rgb,
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(Resource {
        data: Arc::new(png),
        format: AssetFormat::Png,
        width,
        height,
    })
}
