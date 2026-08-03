use alloc::{vec, vec::Vec};

use crate::SstvError;

/// An eight-bit red, green, and blue pixel.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgb8 {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb8 {
    /// Constructs an RGB pixel.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Validated non-zero image dimensions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ImageSize {
    width: usize,
    height: usize,
}

impl ImageSize {
    /// Validates and constructs image dimensions.
    pub const fn new(width: usize, height: usize) -> Result<Self, SstvError> {
        if width == 0 || height == 0 {
            return Err(SstvError::EmptyImage);
        }
        let Some(pixel_count) = width.checked_mul(height) else {
            return Err(SstvError::ImageSizeOverflow);
        };
        match pixel_count.checked_mul(core::mem::size_of::<Rgb8>()) {
            Some(bytes) if bytes <= isize::MAX as usize => {}
            _ => return Err(SstvError::ImageSizeOverflow),
        }
        Ok(Self { width, height })
    }

    /// Returns the width in pixels.
    pub const fn width(self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    pub const fn height(self) -> usize {
        self.height
    }

    /// Returns the number of pixels.
    pub const fn pixel_count(self) -> usize {
        self.width * self.height
    }
}

/// An owned, row-major RGB image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbImage {
    size: ImageSize,
    pixels: Vec<Rgb8>,
}

impl RgbImage {
    /// Allocates an image initialized to `fill`.
    pub fn new(size: ImageSize, fill: Rgb8) -> Self {
        Self {
            size,
            pixels: vec![fill; size.pixel_count()],
        }
    }

    /// Validates and takes ownership of row-major pixels.
    pub fn from_pixels(size: ImageSize, pixels: Vec<Rgb8>) -> Result<Self, SstvError> {
        if pixels.len() != size.pixel_count() {
            return Err(SstvError::InvalidPixelCount);
        }
        Ok(Self { size, pixels })
    }

    /// Returns the dimensions.
    pub const fn size(&self) -> ImageSize {
        self.size
    }

    /// Returns the row-major pixels.
    pub fn pixels(&self) -> &[Rgb8] {
        &self.pixels
    }

    /// Returns mutable row-major pixels.
    pub fn pixels_mut(&mut self) -> &mut [Rgb8] {
        &mut self.pixels
    }

    /// Returns one immutable row if `y` is in bounds.
    pub fn row(&self, y: usize) -> Option<&[Rgb8]> {
        let start = y.checked_mul(self.size.width)?;
        self.pixels.get(start..start + self.size.width)
    }

    /// Returns one mutable row if `y` is in bounds.
    pub fn row_mut(&mut self, y: usize) -> Option<&mut [Rgb8]> {
        let start = y.checked_mul(self.size.width)?;
        self.pixels.get_mut(start..start + self.size.width)
    }

    /// Returns a pixel if the coordinates are in bounds.
    pub fn get(&self, x: usize, y: usize) -> Option<Rgb8> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        Some(self.pixels[y * self.size.width + x])
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn validates_dimensions_and_storage() {
        assert_eq!(ImageSize::new(0, 1), Err(SstvError::EmptyImage));
        assert_eq!(
            ImageSize::new(usize::MAX, 2),
            Err(SstvError::ImageSizeOverflow)
        );
        assert_eq!(
            ImageSize::new(usize::MAX / 2, 2),
            Err(SstvError::ImageSizeOverflow)
        );
        let size = ImageSize::new(2, 2).unwrap();
        assert_eq!(
            RgbImage::from_pixels(size, vec![Rgb8::default(); 3]),
            Err(SstvError::InvalidPixelCount)
        );
        let image = RgbImage::new(size, Rgb8::new(1, 2, 3));
        assert_eq!(image.get(1, 1), Some(Rgb8::new(1, 2, 3)));
        assert_eq!(image.get(2, 0), None);
        assert_eq!(image.row(1), Some(&[Rgb8::new(1, 2, 3); 2][..]));
        assert_eq!(image.row(2), None);
    }
}
