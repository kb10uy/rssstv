use iced::mouse::Cursor;
use iced::widget::canvas::{self, Cache, Geometry, Path, Stroke};
use iced::widget::image::FilterMethod;
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

use crate::raster::Raster;

const VIEWPORT: Color = Color::from_rgb(0.11, 0.11, 0.12);
const PENDING: Color = Color::from_rgb(0.06, 0.06, 0.07);
const SCAN_LINE: Color = Color::from_rgb(0.58, 0.77, 0.99);

/// Main image view.
///
/// The raster is only one of several things drawn here, so this is a canvas
/// rather than an image widget: the undecoded region, the scan boundary, and
/// later overlays share the raster's coordinate space.
#[derive(Debug)]
pub struct ImageCanvas<'a> {
    cache: &'a Cache,
    raster: &'a Raster,
    decoded_fraction: f32,
}

impl<'a> ImageCanvas<'a> {
    pub fn new(cache: &'a Cache, raster: &'a Raster, decoded_fraction: f32) -> Self {
        Self {
            cache,
            raster,
            decoded_fraction: decoded_fraction.clamp(0.0, 1.0),
        }
    }
}

impl<Message> canvas::Program<Message> for ImageCanvas<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let geometry = self.cache.draw(renderer, bounds.size(), |frame| {
            let area = frame.size();
            frame.fill_rectangle(Point::ORIGIN, area, VIEWPORT);

            let frame_rect = letterbox(area, self.raster.aspect_ratio());
            frame.draw_image(
                frame_rect,
                canvas::Image::new(self.raster.handle().clone())
                    .filter_method(FilterMethod::Nearest)
                    .snap(true),
            );

            if self.decoded_fraction >= 1.0 {
                return;
            }
            let decoded_height = frame_rect.height * self.decoded_fraction;
            let boundary = frame_rect.y + decoded_height;
            frame.fill_rectangle(
                Point::new(frame_rect.x, boundary),
                Size::new(frame_rect.width, frame_rect.height - decoded_height),
                PENDING,
            );
            frame.stroke(
                &Path::line(
                    Point::new(frame_rect.x, boundary),
                    Point::new(frame_rect.x + frame_rect.width, boundary),
                ),
                Stroke::default().with_color(SCAN_LINE).with_width(2.0),
            );
        });
        vec![geometry]
    }
}

/// Centers `aspect_ratio` inside `area` without distorting it.
fn letterbox(area: Size, aspect_ratio: f32) -> Rectangle {
    if area.width <= 0.0 || area.height <= 0.0 || aspect_ratio <= 0.0 {
        return Rectangle::new(Point::ORIGIN, Size::ZERO);
    }
    let size = if area.width / area.height > aspect_ratio {
        Size::new(area.height * aspect_ratio, area.height)
    } else {
        Size::new(area.width, area.width / aspect_ratio)
    };
    Rectangle::new(
        Point::new(
            (area.width - size.width) / 2.0,
            (area.height - size.height) / 2.0,
        ),
        size,
    )
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(
        Size::new(400.0, 400.0),
        2.0,
        Rectangle::new(Point::new(0.0, 100.0), Size::new(400.0, 200.0))
    )]
    #[case(
        Size::new(400.0, 100.0),
        2.0,
        Rectangle::new(Point::new(100.0, 0.0), Size::new(200.0, 100.0))
    )]
    #[case(
        Size::new(400.0, 200.0),
        2.0,
        Rectangle::new(Point::ORIGIN, Size::new(400.0, 200.0))
    )]
    fn letterbox_centers_without_distortion(
        #[case] area: Size,
        #[case] aspect_ratio: f32,
        #[case] expected: Rectangle,
    ) {
        let result = letterbox(area, aspect_ratio);
        assert_eq!(result, expected);
        assert!((result.width / result.height - aspect_ratio).abs() < f32::EPSILON);
    }

    #[test]
    fn degenerate_areas_produce_an_empty_frame() {
        assert_eq!(letterbox(Size::ZERO, 2.0).size(), Size::ZERO);
        assert_eq!(letterbox(Size::new(100.0, 100.0), 0.0).size(), Size::ZERO);
    }
}
