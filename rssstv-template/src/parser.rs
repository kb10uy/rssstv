use std::collections::HashSet;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use crate::{
    TemplateError,
    scene::{
        Anchor, Clip, Color, EllipseLayer, Font, FontStyle, Gradient, GradientKind, GradientStop,
        GroupLayer, ImageFit, ImageLayer, Layer, LayerSize, Length, LineLayer, Paint, Position,
        ReceivedImageLayer, RectangleLayer, Stroke, Template, TextLayer,
    },
};

/// The multiple of the font size between the baselines of wrapped lines.
const DEFAULT_LEADING: f64 = 1.2;

/// What a `position` node is allowed to say beyond its coordinates.
#[derive(Clone, Copy, Eq, PartialEq)]
enum PositionKind {
    /// A layer's own placement: it anchors and it rotates.
    Anchored,
    /// A group's offset: it rotates, but there is no box to anchor.
    Offset,
    /// A line endpoint: coordinates alone.
    Endpoint,
}

/// What a `size` node is allowed to say beyond its extent.
#[derive(Clone, Copy, Eq, PartialEq)]
enum SizeKind {
    /// An image, which may derive one dimension from its own aspect ratio and
    /// may round its corners.
    Image,
    /// A rectangle, which requires both dimensions and may round its corners.
    Rectangle,
    /// A shape whose outline is not a box, so there is no corner to round.
    Shape,
}

impl SizeKind {
    const fn requires_both(self) -> bool {
        !matches!(self, Self::Image)
    }

    const fn allows_radius(self) -> bool {
        !matches!(self, Self::Shape)
    }
}

impl Template {
    /// Parses a KDL v2 template document.
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        let document = KdlDocument::parse_v2(source)?;
        Ok(Self {
            layers: parse_layers(document.nodes())?,
        })
    }
}

fn parse_layers(nodes: &[KdlNode]) -> Result<Vec<Layer>, TemplateError> {
    nodes.iter().map(parse_layer).collect()
}

fn parse_layer(node: &KdlNode) -> Result<Layer, TemplateError> {
    reject_node_type(node)?;
    match node.name().value() {
        "image" => parse_image(node).map(Layer::Image),
        "rximage" => parse_received_image(node).map(Layer::ReceivedImage),
        "text" => parse_text(node).map(Layer::Text),
        "rect" => parse_rectangle(node).map(Layer::Rectangle),
        "ellipse" => parse_ellipse(node).map(Layer::Ellipse),
        "line" => parse_line(node).map(Layer::Line),
        "group" => parse_group(node).map(Layer::Group),
        name => schema(format!("unknown layer `{name}`")),
    }
}

fn parse_image(node: &KdlNode) -> Result<ImageLayer, TemplateError> {
    let reference = one_string_argument(node)?.to_owned();
    if !crate::renderer::reference_is_confined(&reference) {
        return schema(format!(
            "image reference `{reference}` reaches outside the template directory"
        ));
    }
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size", "clip"])?;
    let position = parse_position(
        required_unique_child(children, "position")?,
        PositionKind::Anchored,
    )?;
    let size = parse_size(required_unique_child(children, "size")?, SizeKind::Image)?;
    Ok(ImageLayer {
        reference,
        position,
        clip: parse_image_clip(children, size)?,
        size,
    })
}

fn parse_received_image(node: &KdlNode) -> Result<ReceivedImageLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size", "clip"])?;
    let size = parse_size(required_unique_child(children, "size")?, SizeKind::Image)?;
    Ok(ReceivedImageLayer {
        position: parse_position(
            required_unique_child(children, "position")?,
            PositionKind::Anchored,
        )?,
        clip: parse_image_clip(children, size)?,
        size,
    })
}

fn parse_image_clip(
    children: &KdlDocument,
    size: LayerSize,
) -> Result<Option<Clip>, TemplateError> {
    let clip = optional_unique_child(children, "clip")?
        .map(parse_clip)
        .transpose()?;
    if clip.is_some() && size.radius.is_some() {
        return schema("a clipped layer has no corner to round");
    }
    Ok(clip)
}

fn parse_text(node: &KdlNode) -> Result<TextLayer, TemplateError> {
    let text = one_string_argument(node)?.to_owned();
    let children = required_children(node)?;
    validate_child_names(children, &["position", "font", "fill", "stroke"])?;
    let font_node = required_unique_child(children, "font")?;
    validate_properties(
        font_node,
        &["family", "size", "weight", "style", "leading"],
        0,
    )?;
    let family = required_string(font_node, "family")?.to_owned();
    if family.is_empty() {
        return schema("font family must not be empty");
    }
    let weight = required_integer(font_node, "weight")?;
    let weight = u16::try_from(weight)
        .ok()
        .filter(|weight| (1..=1000).contains(weight))
        .ok_or_else(|| TemplateError::Schema("font weight must be between 1 and 1000".into()))?;
    let style = font_node
        .get("style")
        .map(|_| required_string(font_node, "style").and_then(parse_font_style))
        .transpose()?
        .unwrap_or_default();
    let leading = optional_number(font_node, "leading")?.unwrap_or(DEFAULT_LEADING);
    if leading < 0.0 {
        return schema("font leading must not be negative");
    }
    let fill = parse_fill(required_unique_child(children, "fill")?)?;

    Ok(TextLayer {
        text,
        position: parse_position(
            required_unique_child(children, "position")?,
            PositionKind::Anchored,
        )?,
        font: Font {
            family,
            size: required_length(font_node, "size")?,
            weight,
            style,
            leading,
        },
        fill,
        stroke: optional_unique_child(children, "stroke")?
            .map(parse_stroke)
            .transpose()?,
    })
}

fn parse_rectangle(node: &KdlNode) -> Result<RectangleLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size", "fill", "stroke"])?;
    let fill = optional_unique_child(children, "fill")?
        .map(parse_fill)
        .transpose()?;
    let stroke = optional_unique_child(children, "stroke")?
        .map(parse_stroke)
        .transpose()?;
    require_paint(fill.as_ref(), stroke)?;
    Ok(RectangleLayer {
        position: parse_position(
            required_unique_child(children, "position")?,
            PositionKind::Anchored,
        )?,
        size: parse_size(
            required_unique_child(children, "size")?,
            SizeKind::Rectangle,
        )?,
        fill,
        stroke,
    })
}

fn parse_ellipse(node: &KdlNode) -> Result<EllipseLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size", "fill", "stroke"])?;
    let fill = optional_unique_child(children, "fill")?
        .map(parse_fill)
        .transpose()?;
    let stroke = optional_unique_child(children, "stroke")?
        .map(parse_stroke)
        .transpose()?;
    require_paint(fill.as_ref(), stroke)?;
    Ok(EllipseLayer {
        position: parse_position(
            required_unique_child(children, "position")?,
            PositionKind::Anchored,
        )?,
        size: parse_size(required_unique_child(children, "size")?, SizeKind::Shape)?,
        fill,
        stroke,
    })
}

fn parse_line(node: &KdlNode) -> Result<LineLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["start", "end", "stroke"])?;
    Ok(LineLayer {
        start: parse_position(
            required_unique_child(children, "start")?,
            PositionKind::Endpoint,
        )?,
        end: parse_position(
            required_unique_child(children, "end")?,
            PositionKind::Endpoint,
        )?,
        stroke: parse_stroke(required_unique_child(children, "stroke")?)?,
    })
}

fn parse_group(node: &KdlNode) -> Result<GroupLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    let mut position = None;
    let mut layers = Vec::new();
    for child in children.nodes() {
        if child.name().value() == "position" {
            if position.is_some() {
                return schema("group contains duplicate `position` nodes");
            }
            position = Some(parse_position(child, PositionKind::Offset)?);
        } else {
            layers.push(parse_layer(child)?);
        }
    }
    if layers.is_empty() {
        return schema("group must contain at least one layer");
    }
    Ok(GroupLayer { position, layers })
}

fn parse_position(node: &KdlNode, kind: PositionKind) -> Result<Position, TemplateError> {
    let allowed = match kind {
        PositionKind::Anchored => &["x", "y", "anchor", "rotate"][..],
        PositionKind::Offset => &["x", "y", "rotate"][..],
        PositionKind::Endpoint => &["x", "y"][..],
    };
    validate_properties(node, allowed, 0)?;
    let anchor = node
        .get("anchor")
        .map(|_| required_string(node, "anchor").and_then(parse_anchor))
        .transpose()?
        .unwrap_or_default();
    Ok(Position {
        x: required_coordinate(node, "x")?,
        y: required_coordinate(node, "y")?,
        anchor,
        rotation: optional_number(node, "rotate")?.unwrap_or(0.0),
    })
}

fn parse_size(node: &KdlNode, kind: SizeKind) -> Result<LayerSize, TemplateError> {
    validate_properties(node, &["width", "height", "fit", "aspect", "radius"], 0)?;
    let width = optional_length(node, "width")?;
    let height = optional_length(node, "height")?;
    if width.is_none() && height.is_none() {
        return schema("size requires width or height");
    }
    if kind.requires_both() && (width.is_none() || height.is_none()) {
        return schema("shape size requires both width and height");
    }
    if node.get("fit").is_some() && node.get("aspect").is_some() {
        return schema("size cannot specify both `fit` and `aspect`");
    }
    let fit = node
        .get("fit")
        .map(|_| required_string(node, "fit").and_then(parse_fit))
        .or_else(|| {
            node.get("aspect")
                .map(|_| required_string(node, "aspect").and_then(parse_fit))
        })
        .transpose()?
        .unwrap_or_default();
    let radius = optional_length(node, "radius")?;
    if radius.is_some() && !kind.allows_radius() {
        return schema("this layer has no corner to round");
    }
    Ok(LayerSize {
        width,
        height,
        fit,
        radius,
    })
}

fn parse_clip(node: &KdlNode) -> Result<Clip, TemplateError> {
    validate_properties(node, &["shape"], 0)?;
    match required_string(node, "shape")? {
        "circle" => Ok(Clip::Circle),
        "ellipse" => Ok(Clip::Ellipse),
        value => schema(format!("unknown clip shape `{value}`")),
    }
}

fn parse_fill(node: &KdlNode) -> Result<Paint, TemplateError> {
    validate_properties(node, &["color", "gradient", "angle"], 0)?;
    if node.get("gradient").is_none() {
        if node.get("angle").is_some() {
            return schema("`angle` requires `gradient`");
        }
        if node.children().is_some() {
            return schema("solid fill must not contain stops");
        }
        return parse_color(required_string(node, "color")?).map(Paint::Solid);
    }
    if node.get("color").is_some() {
        return schema("fill cannot specify both `color` and `gradient`");
    }
    let kind = match required_string(node, "gradient")? {
        "linear" => GradientKind::Linear {
            angle: optional_number(node, "angle")?.unwrap_or(0.0),
        },
        "radial" => {
            if node.get("angle").is_some() {
                return schema("a radial gradient has no `angle`");
            }
            GradientKind::Radial
        }
        value => return schema(format!("unknown gradient `{value}`")),
    };
    Ok(Paint::Gradient(Gradient {
        kind,
        stops: parse_stops(required_children(node)?)?,
    }))
}

fn parse_stops(document: &KdlDocument) -> Result<Vec<GradientStop>, TemplateError> {
    validate_child_names(document, &["stop"])?;
    let mut stops: Vec<GradientStop> = Vec::new();
    for node in document.nodes() {
        validate_properties(node, &["offset", "color"], 0)?;
        let offset = required_number(node, "offset")?;
        if !(0.0..=1.0).contains(&offset) {
            return schema("stop offset must be between 0 and 1");
        }
        if stops.last().is_some_and(|last| offset < last.offset) {
            return schema("stop offsets must not decrease");
        }
        stops.push(GradientStop {
            offset,
            color: parse_color(required_string(node, "color")?)?,
        });
    }
    if stops.len() < 2 {
        return schema("a gradient requires at least two stops");
    }
    Ok(stops)
}

fn parse_stroke(node: &KdlNode) -> Result<Stroke, TemplateError> {
    validate_properties(node, &["color", "width"], 0)?;
    Ok(Stroke {
        color: parse_color(required_string(node, "color")?)?,
        width: required_length(node, "width")?,
    })
}

fn require_paint(fill: Option<&Paint>, stroke: Option<Stroke>) -> Result<(), TemplateError> {
    if fill.is_none() && stroke.is_none() {
        return schema("shape requires fill or stroke");
    }
    Ok(())
}

fn parse_anchor(value: &str) -> Result<Anchor, TemplateError> {
    match value {
        "top-left" => Ok(Anchor::TopLeft),
        "top-center" => Ok(Anchor::TopCenter),
        "top-right" => Ok(Anchor::TopRight),
        "center" => Ok(Anchor::Center),
        "bottom-left" => Ok(Anchor::BottomLeft),
        "bottom-center" => Ok(Anchor::BottomCenter),
        "bottom-right" => Ok(Anchor::BottomRight),
        _ => schema(format!("unknown anchor `{value}`")),
    }
}

fn parse_font_style(value: &str) -> Result<FontStyle, TemplateError> {
    match value {
        "normal" => Ok(FontStyle::Normal),
        "italic" => Ok(FontStyle::Italic),
        _ => schema(format!("unknown font style `{value}`")),
    }
}

fn parse_fit(value: &str) -> Result<ImageFit, TemplateError> {
    match value {
        "contain" => Ok(ImageFit::Contain),
        "cover" => Ok(ImageFit::Cover),
        "stretch" => Ok(ImageFit::Stretch),
        "preserve" => Ok(ImageFit::Preserve),
        _ => schema(format!("unknown image fit `{value}`")),
    }
}

fn parse_color(value: &str) -> Result<Color, TemplateError> {
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| TemplateError::Schema(format!("color `{value}` must start with #")))?;
    if hex.len() != 6 && hex.len() != 8 {
        return schema(format!("color `{value}` must be #RRGGBB or #RRGGBBAA"));
    }
    let channel = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16)
            .map_err(|_| TemplateError::Schema(format!("color `{value}` is invalid")))
    };
    Ok(Color {
        r: channel(0..2)?,
        g: channel(2..4)?,
        b: channel(4..6)?,
        a: if hex.len() == 8 { channel(6..8)? } else { 255 },
    })
}

fn required_length(node: &KdlNode, name: &str) -> Result<Length, TemplateError> {
    parse_optional_length(node, name, false)?.ok_or_else(|| {
        TemplateError::Schema(format!("`{}` requires property `{name}`", node.name()))
    })
}

fn optional_length(node: &KdlNode, name: &str) -> Result<Option<Length>, TemplateError> {
    parse_optional_length(node, name, false)
}

fn required_coordinate(node: &KdlNode, name: &str) -> Result<Length, TemplateError> {
    parse_optional_length(node, name, true)?.ok_or_else(|| {
        TemplateError::Schema(format!("`{}` requires property `{name}`", node.name()))
    })
}

fn parse_optional_length(
    node: &KdlNode,
    name: &str,
    allow_negative: bool,
) -> Result<Option<Length>, TemplateError> {
    let Some(entry) = unique_property(node, name)? else {
        return Ok(None);
    };
    let value = match entry.value() {
        KdlValue::Integer(value) => *value as f64,
        KdlValue::Float(value) => *value,
        _ => return schema(format!("property `{name}` must be a number")),
    };
    if !value.is_finite() || (!allow_negative && value < 0.0) {
        let constraint = if allow_negative {
            "finite"
        } else {
            "finite and non-negative"
        };
        return schema(format!("property `{name}` must be {constraint}"));
    }
    let unit = entry
        .ty()
        .map(|identifier| identifier.value())
        .ok_or_else(|| TemplateError::Schema(format!("property `{name}` requires a unit")))?;
    let length = match unit {
        "fw" => Length::FrameWidth(value),
        "fh" => Length::FrameHeight(value),
        "em" => Length::Em(value),
        _ => return schema(format!("unknown length unit `({unit})`")),
    };
    Ok(Some(length))
}

fn required_number(node: &KdlNode, name: &str) -> Result<f64, TemplateError> {
    optional_number(node, name)?.ok_or_else(|| {
        TemplateError::Schema(format!("`{}` requires property `{name}`", node.name()))
    })
}

fn optional_number(node: &KdlNode, name: &str) -> Result<Option<f64>, TemplateError> {
    let Some(entry) = unique_property(node, name)? else {
        return Ok(None);
    };
    if entry.ty().is_some() {
        return schema(format!("property `{name}` must not have a unit"));
    }
    let value = match entry.value() {
        KdlValue::Integer(value) => *value as f64,
        KdlValue::Float(value) => *value,
        _ => return schema(format!("property `{name}` must be a number")),
    };
    if !value.is_finite() {
        return schema(format!("property `{name}` must be finite"));
    }
    Ok(Some(value))
}

fn required_string<'a>(node: &'a KdlNode, name: &str) -> Result<&'a str, TemplateError> {
    unique_property(node, name)?
        .ok_or_else(|| {
            TemplateError::Schema(format!("`{}` requires property `{name}`", node.name()))
        })?
        .value()
        .as_string()
        .ok_or_else(|| TemplateError::Schema(format!("property `{name}` must be a string")))
}

fn required_integer(node: &KdlNode, name: &str) -> Result<i128, TemplateError> {
    unique_property(node, name)?
        .ok_or_else(|| {
            TemplateError::Schema(format!("`{}` requires property `{name}`", node.name()))
        })?
        .value()
        .as_integer()
        .ok_or_else(|| TemplateError::Schema(format!("property `{name}` must be an integer")))
}

fn one_string_argument(node: &KdlNode) -> Result<&str, TemplateError> {
    validate_properties(node, &[], 1)?;
    node.entries()[0].value().as_string().ok_or_else(|| {
        TemplateError::Schema(format!("`{}` argument must be a string", node.name()))
    })
}

fn no_entries(node: &KdlNode) -> Result<(), TemplateError> {
    validate_properties(node, &[], 0)
}

fn validate_properties(
    node: &KdlNode,
    allowed: &[&str],
    positional_count: usize,
) -> Result<(), TemplateError> {
    let mut seen = HashSet::new();
    let mut positional = 0;
    for entry in node.entries() {
        match entry.name() {
            Some(name) => {
                let name = name.value();
                if !allowed.contains(&name) {
                    return schema(format!("unknown property `{name}` on `{}`", node.name()));
                }
                if !seen.insert(name) {
                    return schema(format!("duplicate property `{name}` on `{}`", node.name()));
                }
            }
            None => positional += 1,
        }
    }
    if positional != positional_count {
        return schema(format!(
            "`{}` requires {positional_count} positional argument(s)",
            node.name()
        ));
    }
    if node.ty().is_some() {
        return schema(format!("node `{}` must not have a type", node.name()));
    }
    Ok(())
}

fn unique_property<'a>(
    node: &'a KdlNode,
    name: &str,
) -> Result<Option<&'a KdlEntry>, TemplateError> {
    let mut entries = node.entries().iter().filter(|entry| {
        entry
            .name()
            .is_some_and(|entry_name| entry_name.value() == name)
    });
    let first = entries.next();
    if entries.next().is_some() {
        return schema(format!("duplicate property `{name}` on `{}`", node.name()));
    }
    Ok(first)
}

fn required_children(node: &KdlNode) -> Result<&KdlDocument, TemplateError> {
    node.children()
        .ok_or_else(|| TemplateError::Schema(format!("`{}` requires a child block", node.name())))
}

fn validate_child_names(document: &KdlDocument, allowed: &[&str]) -> Result<(), TemplateError> {
    for node in document.nodes() {
        if !allowed.contains(&node.name().value()) {
            return schema(format!("unknown child `{}`", node.name()));
        }
    }
    Ok(())
}

fn required_unique_child<'a>(
    document: &'a KdlDocument,
    name: &str,
) -> Result<&'a KdlNode, TemplateError> {
    optional_unique_child(document, name)?
        .ok_or_else(|| TemplateError::Schema(format!("missing child `{name}`")))
}

fn optional_unique_child<'a>(
    document: &'a KdlDocument,
    name: &str,
) -> Result<Option<&'a KdlNode>, TemplateError> {
    let mut nodes = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == name);
    let first = nodes.next();
    if nodes.next().is_some() {
        return schema(format!("duplicate child `{name}`"));
    }
    Ok(first)
}

fn reject_node_type(node: &KdlNode) -> Result<(), TemplateError> {
    if node.ty().is_some() {
        return schema(format!("layer `{}` must not have a type", node.name()));
    }
    Ok(())
}

fn schema<T>(message: impl Into<String>) -> Result<T, TemplateError> {
    Err(TemplateError::Schema(message.into()))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const COMPLETE_TEMPLATE: &str = r##"
image "assets/logo.png" {
    position x=(fw)95 y=(fh)5 anchor="top-right"
    size width=(fw)14 aspect="preserve"
}
rximage {
    position x=(fw)5 y=(fh)22
    size width=(fw)35 height=(fh)45 fit="cover"
}
text "To ${contact.callsign}" {
    position x=(fw)50 y=(fh)8 anchor="top-center"
    font family="Noto Sans" size=(fh)9 weight=700
    fill color="#ffffff"
    stroke color="#182030" width=(em)0.08
}
rect {
    position x=(fw)5 y=(fh)78
    size width=(fw)90 height=(fh)17
    fill color="#101820cc"
}
ellipse {
    position x=(fw)50 y=(fh)50 anchor="center"
    size width=(fw)20 height=(fh)20
    stroke color="#ffffff" width=(fh)0.5
}
line {
    start x=(fw)5 y=(fh)90
    end x=(fw)95 y=(fh)90
    stroke color="#ffffff" width=(fh)0.5
}
group {
    position x=(fw)5 y=(fh)5
    rect {
        position x=(fw)0 y=(fh)0
        size width=(fw)10 height=(fh)10
        fill color="#ffffff"
    }
}
"##;

    #[test]
    fn parses_all_initial_layers_in_document_order() {
        let template = Template::parse(COMPLETE_TEMPLATE).unwrap();
        assert_eq!(template.layers().len(), 7);
        assert!(matches!(template.layers()[0], Layer::Image(_)));
        assert!(matches!(template.layers()[1], Layer::ReceivedImage(_)));
        assert!(matches!(template.layers()[6], Layer::Group(_)));
    }

    #[test]
    fn rejects_unknown_duplicate_and_unitless_properties() {
        let unknown = Template::parse("rect mystery=1").unwrap_err();
        assert!(unknown.to_string().contains("unknown property"));

        let duplicate = Template::parse(
            "rect { position x=(fw)1 x=(fw)2 y=(fh)1; size width=(fw)1 height=(fh)1; fill color=\"#ffffff\"; }",
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate property"));

        let unitless = Template::parse(
            "rect { position x=1 y=(fh)1; size width=(fw)1 height=(fh)1; fill color=\"#ffffff\"; }",
        )
        .unwrap_err();
        assert!(unitless.to_string().contains("requires a unit"));
    }

    #[test]
    fn accepts_negative_coordinates_but_rejects_negative_sizes() {
        let template = Template::parse(
            "rect { position x=(fw)-5 y=(fh)-1.5; size width=(fw)10 height=(fh)20; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        let Layer::Rectangle(rectangle) = &template.layers()[0] else {
            panic!("expected rectangle");
        };
        assert_eq!(rectangle.position.x, Length::FrameWidth(-5.0));
        assert_eq!(rectangle.position.y, Length::FrameHeight(-1.5));

        let negative_size = Template::parse(
            "rect { position x=(fw)0 y=(fh)0; size width=(fw)-1 height=(fh)20; fill color=\"#ffffff\"; }",
        )
        .unwrap_err();
        assert!(negative_size.to_string().contains("non-negative"));
    }

    #[test]
    fn parses_linear_and_radial_gradient_fills() {
        let template = Template::parse(
            r##"
rect {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)10
    fill gradient="linear" angle=90 {
        stop offset=0 color="#00ffff"
        stop offset=1 color="#00ff0080"
    }
}
ellipse {
    position x=(fw)50 y=(fh)50 anchor="center"
    size width=(fw)20 height=(fh)20
    fill gradient="radial" {
        stop offset=0.25 color="#ffffff"
        stop offset=1 color="#000000"
    }
}
"##,
        )
        .unwrap();
        let Layer::Rectangle(rectangle) = &template.layers()[0] else {
            panic!("expected rectangle");
        };
        let Some(Paint::Gradient(linear)) = &rectangle.fill else {
            panic!("expected a gradient fill");
        };
        assert_eq!(linear.kind, GradientKind::Linear { angle: 90.0 });
        assert_eq!(linear.stops.len(), 2);
        assert_eq!(linear.stops[1].color.a, 0x80);

        let Layer::Ellipse(ellipse) = &template.layers()[1] else {
            panic!("expected ellipse");
        };
        let Some(Paint::Gradient(radial)) = &ellipse.fill else {
            panic!("expected a gradient fill");
        };
        assert_eq!(radial.kind, GradientKind::Radial);
        assert_eq!(radial.stops[0].offset, 0.25);
    }

    #[rstest]
    #[case(
        "fill color=\"#ffffff\" gradient=\"linear\" { stop offset=0 color=\"#000000\"; stop offset=1 color=\"#ffffff\"; }",
        "both `color` and `gradient`"
    )]
    #[case(
        "fill gradient=\"conic\" { stop offset=0 color=\"#000000\"; stop offset=1 color=\"#ffffff\"; }",
        "unknown gradient"
    )]
    #[case(
        "fill gradient=\"radial\" angle=90 { stop offset=0 color=\"#000000\"; stop offset=1 color=\"#ffffff\"; }",
        "radial gradient has no `angle`"
    )]
    #[case(
        "fill gradient=\"linear\" { stop offset=0 color=\"#000000\"; }",
        "at least two stops"
    )]
    #[case(
        "fill gradient=\"linear\" { stop offset=1 color=\"#000000\"; stop offset=0 color=\"#ffffff\"; }",
        "must not decrease"
    )]
    #[case(
        "fill gradient=\"linear\" { stop offset=1.5 color=\"#000000\"; stop offset=2 color=\"#ffffff\"; }",
        "between 0 and 1"
    )]
    #[case(
        "fill gradient=\"linear\" { stop offset=0 color=\"#000000\"; blend offset=1 color=\"#ffffff\"; }",
        "unknown child `blend`"
    )]
    #[case("fill color=\"#ffffff\" angle=90", "`angle` requires `gradient`")]
    #[case(
        "fill gradient=\"linear\" angle=(fw)90 { stop offset=0 color=\"#000000\"; stop offset=1 color=\"#ffffff\"; }",
        "must not have a unit"
    )]
    fn rejects_malformed_gradients(#[case] fill: &str, #[case] message: &str) {
        let error = Template::parse(&format!(
            "rect {{\nposition x=(fw)0 y=(fh)0\nsize width=(fw)1 height=(fh)1\n{fill}\n}}"
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains(message),
            "expected `{message}` in `{error}`"
        );
    }

    #[rstest]
    #[case("../secret.png")]
    #[case("/etc/secret.png")]
    #[case("photos/../../secret.png")]
    fn rejects_image_references_that_leave_the_template_directory(#[case] reference: &str) {
        let error = Template::parse(&format!(
            "image \"{reference}\" {{\nposition x=(fw)0 y=(fh)0\nsize width=(fw)1 height=(fh)1\n}}"
        ))
        .unwrap_err();
        assert!(
            error.to_string().contains("outside the template directory"),
            "expected a confinement error, got `{error}`"
        );
    }

    #[test]
    fn parses_rotation_rounding_clipping_and_leading() {
        let template = Template::parse(
            r##"
rximage {
    position x=(fw)0 y=(fh)0 rotate=-12.5
    size width=(fw)30 height=(fh)30
    clip shape="circle"
}
rect {
    position x=(fw)0 y=(fh)0
    size width=(fw)30 height=(fh)30 radius=(fh)2
    fill color="#ffffff"
}
text "one\ntwo" {
    position x=(fw)0 y=(fh)0
    font family="Noto Sans" size=(fh)9 weight=400 leading=1.5
    fill color="#ffffff"
}
group {
    position x=(fw)5 y=(fh)5 rotate=90
    rect {
        position x=(fw)0 y=(fh)0
        size width=(fw)1 height=(fh)1
        fill color="#ffffff"
    }
}
"##,
        )
        .unwrap();
        let Layer::ReceivedImage(received) = &template.layers()[0] else {
            panic!("expected rximage");
        };
        assert_eq!(received.position.rotation, -12.5);
        assert_eq!(received.clip, Some(Clip::Circle));

        let Layer::Rectangle(rectangle) = &template.layers()[1] else {
            panic!("expected rectangle");
        };
        assert_eq!(rectangle.size.radius, Some(Length::FrameHeight(2.0)));
        assert_eq!(rectangle.position.rotation, 0.0);

        let Layer::Text(text) = &template.layers()[2] else {
            panic!("expected text");
        };
        assert_eq!(text.text, "one\ntwo");
        assert_eq!(text.font.leading, 1.5);

        let Layer::Group(group) = &template.layers()[3] else {
            panic!("expected group");
        };
        assert_eq!(group.position.expect("the group is placed").rotation, 90.0);
    }

    #[rstest]
    #[case(
        "ellipse { position x=(fw)0 y=(fh)0; size width=(fw)1 height=(fh)1 radius=(fh)1; fill color=\"#ffffff\"; }",
        "no corner to round"
    )]
    #[case(
        "rximage { position x=(fw)0 y=(fh)0; size width=(fw)1 height=(fh)1 radius=(fh)1; clip shape=\"circle\"; }",
        "no corner to round"
    )]
    #[case(
        "rximage { position x=(fw)0 y=(fh)0; size width=(fw)1 height=(fh)1; clip shape=\"star\"; }",
        "unknown clip shape"
    )]
    #[case(
        "line { start x=(fw)0 y=(fh)0 rotate=90; end x=(fw)1 y=(fh)1; stroke color=\"#ffffff\" width=(fh)1; }",
        "unknown property `rotate`"
    )]
    #[case(
        "rect { position x=(fw)0 y=(fh)0 rotate=(fh)90; size width=(fw)1 height=(fh)1; fill color=\"#ffffff\"; }",
        "must not have a unit"
    )]
    #[case(
        "text \"a\" { position x=(fw)0 y=(fh)0; font family=\"Noto Sans\" size=(fh)9 weight=400 leading=-1; fill color=\"#ffffff\"; }",
        "leading must not be negative"
    )]
    #[case(
        "rect { position x=(fw)0 y=(fh)0; size width=(fw)1 height=(fh)1; clip shape=\"circle\"; fill color=\"#ffffff\"; }",
        "unknown child `clip`"
    )]
    fn rejects_misplaced_geometry(#[case] source: &str, #[case] message: &str) {
        let error = Template::parse(source).unwrap_err();
        assert!(
            error.to_string().contains(message),
            "expected `{message}` in `{error}`"
        );
    }

    #[test]
    fn parses_optional_font_style_and_rejects_unknown_values() {
        let italic = Template::parse(
            "text \"CQ SSTV\" { position x=(fw)0 y=(fh)0; font family=\"Monaspace Argon\" size=(fh)25 weight=700 style=\"italic\"; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        let Layer::Text(text) = &italic.layers()[0] else {
            panic!("expected text");
        };
        assert_eq!(text.font.style, FontStyle::Italic);

        let normal = Template::parse(
            "text \"CQ SSTV\" { position x=(fw)0 y=(fh)0; font family=\"Monaspace Argon\" size=(fh)25 weight=700; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        let Layer::Text(text) = &normal.layers()[0] else {
            panic!("expected text");
        };
        assert_eq!(text.font.style, FontStyle::Normal);

        let unknown = Template::parse(
            "text \"CQ SSTV\" { position x=(fw)0 y=(fh)0; font family=\"Monaspace Argon\" size=(fh)25 weight=700 style=\"oblique\"; fill color=\"#ffffff\"; }",
        )
        .unwrap_err();
        assert!(unknown.to_string().contains("unknown font style"));
    }
}
