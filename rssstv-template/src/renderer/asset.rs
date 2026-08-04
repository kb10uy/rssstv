use std::{io::Cursor, sync::Arc};

use image::{GenericImageView, ImageEncoder};
use resvg::usvg;
use rssstv_sstv::image::RgbImage;

use crate::{TemplateError, scene::Layer};

#[derive(Clone)]
pub(super) struct Resource {
    pub(super) png: Arc<Vec<u8>>,
    pub(super) width: u32,
    pub(super) height: u32,
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

pub(super) fn validate_png(png: Arc<Vec<u8>>) -> Result<Resource, TemplateError> {
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    let (width, height) = image.dimensions();
    Ok(Resource { png, width, height })
}

pub(super) fn encode_received_image(image: &RgbImage) -> Result<Resource, TemplateError> {
    let width = u32::try_from(image.size().width()).map_err(|_| {
        TemplateError::InvalidDimensions("received image width exceeds PNG limits".into())
    })?;
    let height = u32::try_from(image.size().height()).map_err(|_| {
        TemplateError::InvalidDimensions("received image height exceeds PNG limits".into())
    })?;
    let mut rgb = Vec::with_capacity(image.pixels().len() * 3);
    for pixel in image.pixels() {
        rgb.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut png)).write_image(
        &rgb,
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(Resource {
        png: Arc::new(png),
        width,
        height,
    })
}
