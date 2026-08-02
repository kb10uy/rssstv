use iced::widget::image::Handle;
use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};
use rssstv_sstv::mode::Mode;

use crate::receive::Frame;

/// Display-ready raster.
#[derive(Clone, Debug)]
pub struct Raster {
    size: ImageSize,
    handle: Handle,
}

impl Raster {
    pub fn from_image(image: &RgbImage) -> Self {
        let size = image.size();
        let mut rgba = Vec::with_capacity(size.pixel_count() * 4);
        for pixel in image.pixels() {
            rgba.extend_from_slice(&[pixel.r, pixel.g, pixel.b, u8::MAX]);
        }
        Self {
            size,
            handle: Handle::from_rgba(size.width() as u32, size.height() as u32, rgba),
        }
    }

    /// Wraps a decoded frame without copying its pixels.
    pub fn from_frame(frame: Frame) -> Option<Self> {
        let size = ImageSize::new(frame.width as usize, frame.height as usize).ok()?;
        Some(Self {
            size,
            handle: Handle::from_rgba(frame.width, frame.height, frame.rgba),
        })
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

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.size.width() as f32 / self.size.height() as f32
    }
}

fn test_pattern_image(mode: Mode) -> RgbImage {
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
    fn frames_become_rasters_of_the_same_size() {
        let frame = Frame {
            width: 4,
            height: 2,
            rgba: vec![0; 4 * 2 * 4],
        };
        let raster = Raster::from_frame(frame).unwrap();
        assert_eq!(raster.size().width(), 4);
        assert_eq!(raster.size().height(), 2);
    }

    #[test]
    fn degenerate_frames_are_rejected() {
        assert!(
            Raster::from_frame(Frame {
                width: 0,
                height: 0,
                rgba: Vec::new(),
            })
            .is_none()
        );
    }
}
