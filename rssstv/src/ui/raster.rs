use std::sync::Arc;

use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions, Vec2};
use rssstv_sstv::{
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};

use crate::worker::receive::Frame;

/// Display-ready raster.
///
/// The pixels live in an [`Arc`] so handing them to the texture manager costs a
/// reference count rather than a copy, and the texture they were uploaded to is
/// kept here so a raster that has not changed is not re-uploaded every frame.
///
/// A reception replaces the pixels up to thirty times a second, so both the
/// pixel buffer and the texture are written in place rather than built again:
/// dropping the texture would have the renderer free and allocate one of these
/// on every decoded row, and the largest mode's is two megabytes.
#[derive(Clone)]
pub struct Raster {
    size: ImageSize,
    image: Arc<ColorImage>,
    texture: Option<TextureHandle>,
    /// Whether the current pixels have reached the texture.
    uploaded: bool,
}

impl core::fmt::Debug for Raster {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Raster")
            .field("size", &self.size)
            .field("uploaded", &(self.texture.is_some() && self.uploaded))
            .finish()
    }
}

impl Raster {
    pub fn from_image(image: &RgbImage) -> Self {
        let mut raster = Self {
            size: image.size(),
            image: Arc::new(ColorImage::default()),
            texture: None,
            uploaded: false,
        };
        raster.set_image(image);
        raster
    }

    /// Replaces the pixels with a decoded frame, leaving one with no area
    /// alone.
    ///
    /// Returns whether the frame was adopted.
    pub fn set_frame(&mut self, frame: &Frame) -> bool {
        let Ok(size) = ImageSize::new(frame.width as usize, frame.height as usize) else {
            return false;
        };
        self.replace(size, frame.rgba.chunks_exact(4).map(rgba_pixel));
        true
    }

    /// Replaces the pixels with a composed or generated image.
    pub fn set_image(&mut self, image: &RgbImage) {
        self.replace(
            image.size(),
            image
                .pixels()
                .iter()
                .map(|pixel| Color32::from_rgb(pixel.r, pixel.g, pixel.b)),
        );
    }

    /// Writes `pixels` over the raster, reusing the buffer they go in.
    ///
    /// The buffer is only reused when the texture manager has finished with the
    /// last one, which it has by the frame after an upload. Reusing it while it
    /// was still shared would copy the pixels being replaced, so that case
    /// starts a new one instead.
    fn replace(&mut self, size: ImageSize, pixels: impl Iterator<Item = Color32>) {
        self.size = size;
        self.uploaded = false;
        let dimensions = [size.width(), size.height()];
        let source_size = Vec2::new(size.width() as f32, size.height() as f32);
        match Arc::get_mut(&mut self.image) {
            Some(image) => {
                image.pixels.clear();
                image.pixels.extend(pixels);
                image.size = dimensions;
                image.source_size = source_size;
            }
            None => self.image = Arc::new(ColorImage::new(dimensions, pixels.collect())),
        }
    }

    /// Uploads the pixels if they have changed and returns the texture.
    ///
    /// Nearest-neighbour sampling is deliberate: an SSTV raster is inspected
    /// for per-pixel artifacts, so magnifying it must not interpolate.
    pub fn texture(&mut self, ctx: &Context) -> &TextureHandle {
        if !self.uploaded {
            match self.texture.as_mut() {
                Some(texture) => texture.set(Arc::clone(&self.image), TextureOptions::NEAREST),
                None => {
                    self.texture = Some(ctx.load_texture(
                        "raster",
                        Arc::clone(&self.image),
                        TextureOptions::NEAREST,
                    ));
                }
            }
            self.uploaded = true;
        }
        self.texture
            .as_ref()
            .expect("the raster was just uploaded to a texture")
    }

    /// Builds an all-black raster with the mode's transport geometry.
    pub fn blank(mode: Mode) -> Self {
        let size = ImageSize::new(mode.spec().width() as usize, mode.spec().height() as usize)
            .expect("mode dimensions are valid");
        Self::from_image(&RgbImage::new(size, Rgb8::default()))
    }

    pub fn test_pattern(mode: Mode) -> Self {
        Self::from_image(&test_pattern_image(mode))
    }

    pub const fn size(&self) -> ImageSize {
        self.size
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.size.width() as f32 / self.size.height() as f32
    }
}

/// Reads one pixel out of a decoded frame's bytes.
fn rgba_pixel(pixel: &[u8]) -> Color32 {
    Color32::from_rgba_unmultiplied(pixel[0], pixel[1], pixel[2], pixel[3])
}

/// Builds the color-bar and gray-ramp pattern at a mode's geometry.
///
/// Shared with the transmit composition, which shows it in place of a received
/// image until a reception has produced one.
pub(crate) fn test_pattern_image(mode: Mode) -> RgbImage {
    const BARS: [Rgb8; 8] = [
        Rgb8::new(255, 255, 255),
        Rgb8::new(255, 255, 0),
        Rgb8::new(0, 255, 255),
        Rgb8::new(0, 255, 0),
        Rgb8::new(255, 0, 255),
        Rgb8::new(255, 0, 0),
        Rgb8::new(0, 0, 255),
        Rgb8::new(0, 0, 0),
    ];

    let width = mode.spec().width() as usize;
    let height = mode.spec().height() as usize;
    let size = ImageSize::new(width, height).expect("mode dimensions are valid");
    let bar_split = height * 2 / 3;
    let mut pixels = Vec::with_capacity(size.pixel_count());
    for y in 0..height {
        for x in 0..width {
            pixels.push(if y < bar_split {
                BARS[x * BARS.len() / width]
            } else {
                let ramp = (x * 256 / width) as u8;
                Rgb8::new(ramp, ramp, ramp)
            });
        }
    }
    RgbImage::from_pixels(size, pixels).expect("generated pixel count matches the image size")
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Mode::Robot36)]
    #[case(Mode::Pd120)]
    #[case(Mode::Scottie1)]
    fn test_pattern_matches_mode_geometry(#[case] mode: Mode) {
        let raster = Raster::test_pattern(mode);
        assert_eq!(raster.size().width(), mode.spec().width() as usize);
        assert_eq!(raster.size().height(), mode.spec().height() as usize);
        assert!(raster.aspect_ratio() > 1.0);
    }

    #[test]
    fn blank_rasters_match_mode_geometry() {
        let raster = Raster::blank(Mode::Scottie2);
        assert_eq!(
            raster.size().width(),
            Mode::Scottie2.spec().width() as usize
        );
        assert_eq!(
            raster.size().height(),
            Mode::Scottie2.spec().height() as usize
        );
    }

    #[test]
    fn frames_resize_the_raster_they_replace() {
        let mut raster = Raster::blank(Mode::Robot36);
        assert!(raster.set_frame(&Frame {
            width: 4,
            height: 2,
            rgba: vec![0; 4 * 2 * 4],
        }));
        assert_eq!(raster.size().width(), 4);
        assert_eq!(raster.size().height(), 2);
    }

    /// A frame with no area says nothing, so what is on the canvas stays.
    #[test]
    fn degenerate_frames_are_rejected() {
        let mut raster = Raster::blank(Mode::Robot36);
        assert!(!raster.set_frame(&Frame {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        }));
        assert_eq!(raster.size().width(), Mode::Robot36.spec().width() as usize);
    }

    /// A reception replaces the raster up to thirty times a second, and every
    /// new texture is one the renderer has to allocate and the old one is one
    /// it has to free. The pixels go into the texture already there instead.
    #[test]
    fn a_new_frame_is_written_into_the_texture_it_replaces() {
        let context = Context::default();
        let mut raster = Raster::blank(Mode::Robot36);
        let first = raster.texture(&context).id();

        assert!(raster.set_frame(&Frame {
            width: Mode::Robot36.spec().width().into(),
            height: Mode::Robot36.spec().height().into(),
            rgba: vec![
                7;
                Mode::Robot36.spec().width() as usize
                    * Mode::Robot36.spec().height() as usize
                    * 4
            ],
        }));

        assert_eq!(raster.texture(&context).id(), first);
    }

    /// The pixel buffer is written in place while nothing else holds it, which
    /// is what keeps a reception from allocating one per decoded row.
    #[test]
    fn replacing_pixels_reuses_the_buffer() {
        let mut raster = Raster::blank(Mode::Robot36);
        let before = raster.image.pixels.as_ptr();
        raster.set_frame(&Frame {
            width: Mode::Robot36.spec().width().into(),
            height: Mode::Robot36.spec().height().into(),
            rgba: vec![
                7;
                Mode::Robot36.spec().width() as usize
                    * Mode::Robot36.spec().height() as usize
                    * 4
            ],
        });
        assert_eq!(raster.image.pixels.as_ptr(), before);
        assert_eq!(
            raster.image.pixels[0],
            Color32::from_rgba_unmultiplied(7, 7, 7, 7)
        );
    }
}
