use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use crate::ui::{
    colors::{PENDING, SCAN_LINE, VIEWPORT},
    raster::Raster,
};

/// Draws the main image view into the space `ui` has left.
///
/// The raster is only one of several things drawn here, so this paints
/// directly rather than using an image widget: the undecoded region, the scan
/// boundary, and later overlays share the raster's coordinate space.
/// Returns the area it claimed, so callers can overlay it.
pub fn image_view(ui: &mut Ui, raster: &mut Raster, decoded_fraction: f32) -> Rect {
    let decoded_fraction = decoded_fraction.clamp(0.0, 1.0);
    let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(area, 0.0, VIEWPORT);

    let rect = letterbox(area, raster.aspect_ratio());
    if rect.is_negative() || rect.width() <= 0.0 {
        return area;
    }
    let decoded_height = rect.height() * decoded_fraction;
    let boundary = rect.top() + decoded_height;

    if decoded_fraction < 1.0 {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.left(), boundary), rect.max),
            0.0,
            PENDING,
        );
    }

    // The undecoded region is excluded by clipping the raster rather than by
    // covering it, so the boundary stays exact at any viewport scale.
    let texture = raster.texture(ui.ctx()).id();
    ui.painter_at(Rect::from_min_max(
        rect.min,
        Pos2::new(rect.right(), boundary),
    ))
    .image(
        texture,
        rect,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );

    if decoded_fraction < 1.0 {
        ui.painter().hline(
            rect.left()..=rect.right(),
            boundary,
            Stroke::new(2.0, SCAN_LINE),
        );
    }
    area
}

/// Centers `aspect_ratio` inside `area` without distorting it.
fn letterbox(area: Rect, aspect_ratio: f32) -> Rect {
    if area.width() <= 0.0 || area.height() <= 0.0 || aspect_ratio <= 0.0 {
        return Rect::from_min_size(area.min, Vec2::ZERO);
    }
    let size = if area.width() / area.height() > aspect_ratio {
        Vec2::new(area.height() * aspect_ratio, area.height())
    } else {
        Vec2::new(area.width(), area.width() / aspect_ratio)
    };
    Rect::from_min_size(area.min + (area.size() - size) / 2.0, size)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn area(width: f32, height: f32) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height))
    }

    #[rstest]
    #[case(
        area(400.0, 400.0),
        2.0,
        Rect::from_min_size(Pos2::new(0.0, 100.0), Vec2::new(400.0, 200.0))
    )]
    #[case(
        area(400.0, 100.0),
        2.0,
        Rect::from_min_size(Pos2::new(100.0, 0.0), Vec2::new(200.0, 100.0))
    )]
    #[case(
        area(400.0, 200.0),
        2.0,
        Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))
    )]
    fn letterbox_centers_without_distortion(
        #[case] area: Rect,
        #[case] aspect_ratio: f32,
        #[case] expected: Rect,
    ) {
        let result = letterbox(area, aspect_ratio);
        assert_eq!(result, expected);
        assert!((result.width() / result.height() - aspect_ratio).abs() < f32::EPSILON);
    }

    #[test]
    fn letterbox_is_offset_by_the_area_origin() {
        let offset = Rect::from_min_size(Pos2::new(30.0, 70.0), Vec2::new(400.0, 400.0));
        assert_eq!(
            letterbox(offset, 2.0),
            Rect::from_min_size(Pos2::new(30.0, 170.0), Vec2::new(400.0, 200.0))
        );
    }

    #[test]
    fn degenerate_areas_produce_an_empty_frame() {
        assert_eq!(letterbox(area(0.0, 0.0), 2.0).size(), Vec2::ZERO);
        assert_eq!(letterbox(area(100.0, 100.0), 0.0).size(), Vec2::ZERO);
    }
}
