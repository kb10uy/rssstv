use iced::widget::image::Handle;
use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};
use rssstv_sstv::mode::Mode;

/// Display-ready raster published to the interface.
///
/// The receive worker will own the equivalent conversion once the audio
/// boundary exists; until then [`Raster::test_pattern`] stands in for a decoded
/// image.
#[derive(Clone, Debug)]
pub struct Raster {
    mode: Mode,
    size: ImageSize,
    handle: Handle,
}

impl Raster {
    pub fn from_image(mode: Mode, image: &RgbImage) -> Self {
        let size = image.size();
        let mut rgba = Vec::with_capacity(size.pixel_count() * 4);
        for pixel in image.pixels() {
            rgba.extend_from_slice(&[pixel.r, pixel.g, pixel.b, u8::MAX]);
        }
        Self {
            mode,
            size,
            handle: Handle::from_rgba(size.width() as u32, size.height() as u32, rgba),
        }
    }

    pub fn test_pattern(mode: Mode) -> Self {
        Self::from_image(mode, &test_pattern_image(mode))
    }

    pub const fn mode(&self) -> Mode {
        self.mode
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
}
