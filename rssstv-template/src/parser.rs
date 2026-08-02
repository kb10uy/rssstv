use std::collections::HashSet;

use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};

use crate::TemplateError;
use crate::scene::{
    Anchor, Color, EllipseLayer, Font, GroupLayer, ImageFit, ImageLayer, Layer, LayerSize, Length,
    LineLayer, Position, ReceivedImageLayer, RectangleLayer, Stroke, Template, TextLayer,
};

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
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size"])?;
    let position = parse_position(required_unique_child(children, "position")?, true)?;
    let size = parse_size(required_unique_child(children, "size")?, false)?;
    Ok(ImageLayer {
        reference,
        position,
        size,
    })
}

fn parse_received_image(node: &KdlNode) -> Result<ReceivedImageLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["position", "size"])?;
    Ok(ReceivedImageLayer {
        position: parse_position(required_unique_child(children, "position")?, true)?,
        size: parse_size(required_unique_child(children, "size")?, false)?,
    })
}

fn parse_text(node: &KdlNode) -> Result<TextLayer, TemplateError> {
    let text = one_string_argument(node)?.to_owned();
    let children = required_children(node)?;
    validate_child_names(children, &["position", "font", "fill", "stroke"])?;
    let font_node = required_unique_child(children, "font")?;
    validate_properties(font_node, &["family", "size", "weight"], 0)?;
    let family = required_string(font_node, "family")?.to_owned();
    if family.is_empty() {
        return schema("font family must not be empty");
    }
    let weight = required_integer(font_node, "weight")?;
    let weight = u16::try_from(weight)
        .ok()
        .filter(|weight| (1..=1000).contains(weight))
        .ok_or_else(|| TemplateError::Schema("font weight must be between 1 and 1000".into()))?;
    let fill_node = required_unique_child(children, "fill")?;
    validate_properties(fill_node, &["color"], 0)?;

    Ok(TextLayer {
        text,
        position: parse_position(required_unique_child(children, "position")?, true)?,
        font: Font {
            family,
            size: required_length(font_node, "size")?,
            weight,
        },
        fill: parse_color(required_string(fill_node, "color")?)?,
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
    require_paint(fill, stroke)?;
    Ok(RectangleLayer {
        position: parse_position(required_unique_child(children, "position")?, true)?,
        size: parse_size(required_unique_child(children, "size")?, true)?,
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
    require_paint(fill, stroke)?;
    Ok(EllipseLayer {
        position: parse_position(required_unique_child(children, "position")?, true)?,
        size: parse_size(required_unique_child(children, "size")?, true)?,
        fill,
        stroke,
    })
}

fn parse_line(node: &KdlNode) -> Result<LineLayer, TemplateError> {
    no_entries(node)?;
    let children = required_children(node)?;
    validate_child_names(children, &["start", "end", "stroke"])?;
    Ok(LineLayer {
        start: parse_position(required_unique_child(children, "start")?, false)?,
        end: parse_position(required_unique_child(children, "end")?, false)?,
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
            position = Some(parse_position(child, false)?);
        } else {
            layers.push(parse_layer(child)?);
        }
    }
    if layers.is_empty() {
        return schema("group must contain at least one layer");
    }
    Ok(GroupLayer { position, layers })
}

fn parse_position(node: &KdlNode, allow_anchor: bool) -> Result<Position, TemplateError> {
    let allowed = if allow_anchor {
        &["x", "y", "anchor"][..]
    } else {
        &["x", "y"][..]
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
    })
}

fn parse_size(node: &KdlNode, require_both: bool) -> Result<LayerSize, TemplateError> {
    validate_properties(node, &["width", "height", "fit", "aspect"], 0)?;
    let width = optional_length(node, "width")?;
    let height = optional_length(node, "height")?;
    if width.is_none() && height.is_none() {
        return schema("size requires width or height");
    }
    if require_both && (width.is_none() || height.is_none()) {
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
    Ok(LayerSize { width, height, fit })
}

fn parse_fill(node: &KdlNode) -> Result<Color, TemplateError> {
    validate_properties(node, &["color"], 0)?;
    parse_color(required_string(node, "color")?)
}

fn parse_stroke(node: &KdlNode) -> Result<Stroke, TemplateError> {
    validate_properties(node, &["color", "width"], 0)?;
    Ok(Stroke {
        color: parse_color(required_string(node, "color")?)?,
        width: required_length(node, "width")?,
    })
}

fn require_paint(fill: Option<Color>, stroke: Option<Stroke>) -> Result<(), TemplateError> {
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
}
