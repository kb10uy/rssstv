use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};

use crate::TemplateError;

/// Validated non-zero raster dimensions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RenderSize {
    width: u32,
    height: u32,
}

impl RenderSize {
    /// Validates and constructs render dimensions.
    pub fn new(width: u32, height: u32) -> Result<Self, TemplateError> {
        if width == 0 || height == 0 {
            return Err(TemplateError::InvalidDimensions(
                "width and height must be non-zero".into(),
            ));
        }
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| TemplateError::InvalidDimensions("image size overflow".into()))?;
        if bytes > isize::MAX as u64 {
            return Err(TemplateError::InvalidDimensions(
                "image exceeds the address space".into(),
            ));
        }
        Ok(Self { width, height })
    }

    /// Returns the width in pixels.
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Returns the height in pixels.
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Returns the number of pixels.
    pub const fn pixel_count(self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// An eight-bit straight-alpha RGBA pixel.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgba8 {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

impl Rgba8 {
    /// Constructs an RGBA pixel.
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

/// An owned row-major straight-alpha RGBA image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaImage {
    size: RenderSize,
    pixels: Vec<Rgba8>,
}

impl RgbaImage {
    /// Allocates an image initialized to `fill`.
    pub fn new(size: RenderSize, fill: Rgba8) -> Self {
        Self {
            size,
            pixels: vec![fill; size.pixel_count()],
        }
    }

    /// Validates and takes ownership of row-major pixels.
    pub fn from_pixels(size: RenderSize, pixels: Vec<Rgba8>) -> Result<Self, TemplateError> {
        if pixels.len() != size.pixel_count() {
            return Err(TemplateError::InvalidDimensions(
                "pixel count does not match dimensions".into(),
            ));
        }
        Ok(Self { size, pixels })
    }

    /// Returns the image dimensions.
    pub const fn size(&self) -> RenderSize {
        self.size
    }

    /// Returns the row-major pixels.
    pub fn pixels(&self) -> &[Rgba8] {
        &self.pixels
    }
}

/// Alpha-composites a straight-alpha overlay over an equally sized RGB image.
pub fn composite(background: &RgbImage, overlay: &RgbaImage) -> Result<RgbImage, TemplateError> {
    if background.size().width() != overlay.size.width as usize
        || background.size().height() != overlay.size.height as usize
    {
        return Err(TemplateError::ImageSizeMismatch);
    }

    let pixels = background
        .pixels()
        .iter()
        .zip(&overlay.pixels)
        .map(|(background, foreground)| {
            let alpha = u32::from(foreground.a);
            let inverse = 255 - alpha;
            Rgb8::new(
                blend(foreground.r, background.r, alpha, inverse),
                blend(foreground.g, background.g, alpha, inverse),
                blend(foreground.b, background.b, alpha, inverse),
            )
        })
        .collect();
    let size = ImageSize::new(overlay.size.width as usize, overlay.size.height as usize)?;
    Ok(RgbImage::from_pixels(size, pixels)?)
}

fn blend(foreground: u8, background: u8, alpha: u32, inverse: u32) -> u8 {
    ((u32::from(foreground) * alpha + u32::from(background) * inverse + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composites_straight_alpha_with_defined_rounding() {
        let rgb_size = ImageSize::new(3, 1).unwrap();
        let background = RgbImage::new(rgb_size, Rgb8::new(10, 20, 30));
        let rgba_size = RenderSize::new(3, 1).unwrap();
        let overlay = RgbaImage::from_pixels(
            rgba_size,
            vec![
                Rgba8::new(200, 100, 50, 0),
                Rgba8::new(200, 100, 50, 128),
                Rgba8::new(200, 100, 50, 255),
            ],
        )
        .unwrap();

        let result = composite(&background, &overlay).unwrap();
        assert_eq!(result.get(0, 0), Some(Rgb8::new(10, 20, 30)));
        assert_eq!(result.get(1, 0), Some(Rgb8::new(105, 60, 40)));
        assert_eq!(result.get(2, 0), Some(Rgb8::new(200, 100, 50)));
    }
}
