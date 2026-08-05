use std::{collections::HashMap, fmt::Write};

use crate::{
    RenderSize, TemplateError,
    renderer::{
        RenderContext,
        asset::{Resource, encode_received_image, validate_png},
        variable::interpolate,
    },
    scene::{
        Anchor, Clip, Color, Gradient, GradientKind, GroupLayer, ImageFit, ImageLayer, Layer,
        LayerSize, Length, Paint, Position, ReceivedImageLayer, Stroke, TextLayer,
    },
};

/// Where one image layer lands, independent of which layer supplied it.
#[derive(Clone, Copy)]
struct ImagePlacement {
    position: Position,
    size: LayerSize,
    clip: Option<Clip>,
    offset: (f64, f64),
}

pub(super) struct SvgGenerator<'a> {
    size: RenderSize,
    context: &'a RenderContext<'a>,
    pub(super) resources: HashMap<String, Resource>,
    asset_uris: HashMap<String, String>,
    received_uri: Option<String>,
    next_resource: usize,
    definitions: String,
    next_gradient: usize,
    next_clip: usize,
}

impl<'a> SvgGenerator<'a> {
    pub(super) fn new(size: RenderSize, context: &'a RenderContext<'a>) -> Self {
        Self {
            size,
            context,
            resources: HashMap::new(),
            asset_uris: HashMap::new(),
            received_uri: None,
            next_resource: 0,
            definitions: String::new(),
            next_gradient: 0,
            next_clip: 0,
        }
    }

    pub(super) fn generate(&mut self, layers: &[Layer]) -> Result<String, TemplateError> {
        let mut body = String::new();
        self.write_layers(&mut body, layers, 0.0, 0.0)?;
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
            self.size.width(),
            self.size.height(),
            self.size.width(),
            self.size.height()
        );
        if !self.definitions.is_empty() {
            write!(svg, "<defs>{}</defs>", self.definitions).unwrap();
        }
        svg.push_str(&body);
        svg.push_str("</svg>");
        Ok(svg)
    }

    fn write_layers(
        &mut self,
        svg: &mut String,
        layers: &[Layer],
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        for layer in layers {
            match layer {
                Layer::Image(layer) => self.write_asset(svg, layer, offset_x, offset_y)?,
                Layer::ReceivedImage(layer) => {
                    self.write_received_image(svg, layer, offset_x, offset_y)?
                }
                Layer::Text(layer) => self.write_text(svg, layer, offset_x, offset_y)?,
                Layer::Rectangle(layer) => {
                    let (x, y, width, height) =
                        self.box_geometry(layer.position, layer.size, offset_x, offset_y, None)?;
                    write!(
                        svg,
                        "<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\""
                    )
                    .unwrap();
                    if let Some(radius) = layer.size.radius {
                        let radius = resolve_length(radius, self.size, None)?;
                        write!(svg, " rx=\"{radius}\" ry=\"{radius}\"").unwrap();
                    }
                    self.write_paint(svg, layer.fill.as_ref(), layer.stroke, None)?;
                    write_rotation(svg, layer.position, offset_x, offset_y, self.size, None)?;
                    svg.push_str("/>");
                }
                Layer::Ellipse(layer) => {
                    let (x, y, width, height) =
                        self.box_geometry(layer.position, layer.size, offset_x, offset_y, None)?;
                    write!(
                        svg,
                        "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"",
                        x + width / 2.0,
                        y + height / 2.0,
                        width / 2.0,
                        height / 2.0
                    )
                    .unwrap();
                    self.write_paint(svg, layer.fill.as_ref(), layer.stroke, None)?;
                    write_rotation(svg, layer.position, offset_x, offset_y, self.size, None)?;
                    svg.push_str("/>");
                }
                Layer::Line(layer) => {
                    let x1 = offset_x + resolve_length(layer.start.x, self.size, None)?;
                    let y1 = offset_y + resolve_length(layer.start.y, self.size, None)?;
                    let x2 = offset_x + resolve_length(layer.end.x, self.size, None)?;
                    let y2 = offset_y + resolve_length(layer.end.y, self.size, None)?;
                    write!(
                        svg,
                        "<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" fill=\"none\""
                    )
                    .unwrap();
                    write_stroke(svg, layer.stroke, self.size, None)?;
                    svg.push_str("/>");
                }
                Layer::Group(layer) => self.write_group(svg, layer, offset_x, offset_y)?,
            }
        }
        Ok(())
    }

    fn write_group(
        &mut self,
        svg: &mut String,
        group: &GroupLayer,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        let (group_x, group_y) = if let Some(position) = group.position {
            (
                resolve_length(position.x, self.size, None)?,
                resolve_length(position.y, self.size, None)?,
            )
        } else {
            (0.0, 0.0)
        };
        svg.push_str("<g");
        if let Some(position) = group.position {
            write_rotation(svg, position, offset_x, offset_y, self.size, None)?;
        }
        svg.push('>');
        self.write_layers(svg, &group.layers, offset_x + group_x, offset_y + group_y)?;
        svg.push_str("</g>");
        Ok(())
    }

    fn write_asset(
        &mut self,
        svg: &mut String,
        layer: &ImageLayer,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        let (uri, resource) = self.asset_resource(&layer.reference)?;
        let placement = ImagePlacement {
            position: layer.position,
            size: layer.size,
            clip: layer.clip,
            offset: (offset_x, offset_y),
        };
        self.write_image_element(svg, placement, &uri, &resource)
    }

    fn write_received_image(
        &mut self,
        svg: &mut String,
        layer: &ReceivedImageLayer,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        let (uri, resource) = self.received_resource()?;
        let placement = ImagePlacement {
            position: layer.position,
            size: layer.size,
            clip: layer.clip,
            offset: (offset_x, offset_y),
        };
        self.write_image_element(svg, placement, &uri, &resource)
    }

    fn write_image_element(
        &mut self,
        svg: &mut String,
        placement: ImagePlacement,
        uri: &str,
        resource: &Resource,
    ) -> Result<(), TemplateError> {
        let ImagePlacement {
            position,
            size,
            clip,
            offset,
        } = placement;
        let intrinsic = Some((f64::from(resource.width), f64::from(resource.height)));
        let (x, y, width, height) =
            self.box_geometry(position, size, offset.0, offset.1, intrinsic)?;
        let preserve = match size.fit {
            ImageFit::Stretch => "none",
            ImageFit::Cover => "xMidYMid slice",
            ImageFit::Contain | ImageFit::Preserve => "xMidYMid meet",
        };
        write!(
            svg,
            "<image href=\"{}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"{preserve}\"",
            escape_xml(uri)
        )
        .unwrap();
        let radius = size
            .radius
            .map(|radius| resolve_length(radius, self.size, None))
            .transpose()?;
        if let Some(id) = self.define_clip(clip, radius, (x, y, width, height)) {
            write!(svg, " clip-path=\"url(#{id})\"").unwrap();
        }
        write_rotation(svg, position, offset.0, offset.1, self.size, None)?;
        svg.push_str("/>");
        Ok(())
    }

    fn define_clip(
        &mut self,
        clip: Option<Clip>,
        radius: Option<f64>,
        box_geometry: (f64, f64, f64, f64),
    ) -> Option<String> {
        let (x, y, width, height) = box_geometry;
        let id = format!("clip{}", self.next_clip);
        match (clip, radius) {
            (Some(Clip::Circle), _) => write!(
                self.definitions,
                "<clipPath id=\"{id}\"><circle cx=\"{}\" cy=\"{}\" r=\"{}\"/></clipPath>",
                x + width / 2.0,
                y + height / 2.0,
                width.min(height) / 2.0
            )
            .unwrap(),
            (Some(Clip::Ellipse), _) => write!(
                self.definitions,
                "<clipPath id=\"{id}\"><ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"/></clipPath>",
                x + width / 2.0,
                y + height / 2.0,
                width / 2.0,
                height / 2.0
            )
            .unwrap(),
            (None, Some(radius)) => write!(
                self.definitions,
                "<clipPath id=\"{id}\"><rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" rx=\"{radius}\" ry=\"{radius}\"/></clipPath>"
            )
            .unwrap(),
            (None, None) => return None,
        }
        self.next_clip += 1;
        Some(id)
    }

    fn write_text(
        &mut self,
        svg: &mut String,
        layer: &TextLayer,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        let font_size = resolve_length(layer.font.size, self.size, None)?;
        let x = offset_x + resolve_length(layer.position.x, self.size, Some(font_size))?;
        let y = offset_y + resolve_length(layer.position.y, self.size, Some(font_size))?;
        let (text_anchor, baseline) = text_anchor(layer.position.anchor);
        let text = interpolate(&layer.text, self.context.variables)?;
        let lines: Vec<&str> = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect();
        let leading = layer.font.leading * font_size;
        let block = leading * (lines.len() - 1) as f64;
        let first = y - block * block_shift(layer.position.anchor);
        write!(
            svg,
            "<text x=\"{x}\" y=\"{first}\" text-anchor=\"{text_anchor}\" dominant-baseline=\"{baseline}\" font-family=\"{}\" font-size=\"{font_size}\" font-weight=\"{}\" font-style=\"{}\"",
            escape_xml(&layer.font.family),
            layer.font.weight,
            layer.font.style.as_svg()
        )
        .unwrap();
        self.write_fill(svg, Some(&layer.fill));
        if let Some(stroke) = layer.stroke {
            write_stroke(svg, stroke, self.size, Some(font_size))?;
            svg.push_str(" paint-order=\"stroke fill\" stroke-linejoin=\"round\"");
        }
        write_rotation(
            svg,
            layer.position,
            offset_x,
            offset_y,
            self.size,
            Some(font_size),
        )?;
        svg.push('>');
        match lines.as_slice() {
            [line] => svg.push_str(&escape_xml(line)),
            lines => {
                for (index, line) in lines.iter().enumerate() {
                    write!(svg, "<tspan x=\"{x}\"").unwrap();
                    if index > 0 {
                        write!(svg, " dy=\"{leading}\"").unwrap();
                    }
                    write!(svg, ">{}</tspan>", escape_xml(line)).unwrap();
                }
            }
        }
        svg.push_str("</text>");
        Ok(())
    }

    fn box_geometry(
        &self,
        position: Position,
        size: LayerSize,
        offset_x: f64,
        offset_y: f64,
        intrinsic: Option<(f64, f64)>,
    ) -> Result<(f64, f64, f64, f64), TemplateError> {
        let width = size
            .width
            .map(|length| resolve_length(length, self.size, None))
            .transpose()?;
        let height = size
            .height
            .map(|length| resolve_length(length, self.size, None))
            .transpose()?;
        let (width, height) = match (width, height, intrinsic) {
            (Some(width), Some(height), _) => (width, height),
            (Some(width), None, Some((intrinsic_width, intrinsic_height))) => {
                (width, width * intrinsic_height / intrinsic_width)
            }
            (None, Some(height), Some((intrinsic_width, intrinsic_height))) => {
                (height * intrinsic_width / intrinsic_height, height)
            }
            _ => {
                return Err(TemplateError::Schema(
                    "layer dimensions cannot be resolved".into(),
                ));
            }
        };
        if width <= 0.0 || height <= 0.0 {
            return Err(TemplateError::Schema(
                "rendered layer dimensions must be positive".into(),
            ));
        }
        let mut x = offset_x + resolve_length(position.x, self.size, None)?;
        let mut y = offset_y + resolve_length(position.y, self.size, None)?;
        let (anchor_x, anchor_y) = anchor_offset(position.anchor, width, height);
        x -= anchor_x;
        y -= anchor_y;
        Ok((x, y, width, height))
    }

    fn asset_resource(&mut self, reference: &str) -> Result<(String, Resource), TemplateError> {
        if let Some(uri) = self.asset_uris.get(reference) {
            return Ok((uri.clone(), self.resources[uri].clone()));
        }
        let asset = self
            .context
            .assets
            .load(reference)
            .map_err(|source| TemplateError::Asset {
                reference: reference.to_owned(),
                source,
            })?
            .ok_or_else(|| TemplateError::MissingAsset(reference.to_owned()))?;
        let resource = validate_png(asset.png)?;
        let uri = self.insert_resource("asset", resource.clone());
        self.asset_uris.insert(reference.to_owned(), uri.clone());
        Ok((uri, resource))
    }

    fn received_resource(&mut self) -> Result<(String, Resource), TemplateError> {
        if let Some(uri) = &self.received_uri {
            return Ok((uri.clone(), self.resources[uri].clone()));
        }
        let image = self
            .context
            .received_image
            .ok_or(TemplateError::MissingReceivedImage)?;
        let resource = encode_received_image(image)?;
        let uri = self.insert_resource("rximage", resource.clone());
        self.received_uri = Some(uri.clone());
        Ok((uri, resource))
    }

    fn write_paint(
        &mut self,
        svg: &mut String,
        fill: Option<&Paint>,
        stroke: Option<Stroke>,
        font_size: Option<f64>,
    ) -> Result<(), TemplateError> {
        self.write_fill(svg, fill);
        if let Some(stroke) = stroke {
            write_stroke(svg, stroke, self.size, font_size)?;
        }
        Ok(())
    }

    fn write_fill(&mut self, svg: &mut String, fill: Option<&Paint>) {
        match fill {
            Some(Paint::Solid(color)) => write_color(svg, "fill", *color),
            Some(Paint::Gradient(gradient)) => {
                let id = self.define_gradient(gradient);
                write!(svg, " fill=\"url(#{id})\"").unwrap();
            }
            None => svg.push_str(" fill=\"none\""),
        }
    }

    fn define_gradient(&mut self, gradient: &Gradient) -> String {
        let id = format!("gradient{}", self.next_gradient);
        self.next_gradient += 1;
        let element = match gradient.kind {
            GradientKind::Linear { angle } => {
                let radians = angle.to_radians();
                let (x, y) = (radians.cos() / 2.0, radians.sin() / 2.0);
                write!(
                    self.definitions,
                    "<linearGradient id=\"{id}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
                    unit_coordinate(0.5 - x),
                    unit_coordinate(0.5 - y),
                    unit_coordinate(0.5 + x),
                    unit_coordinate(0.5 + y)
                )
                .unwrap();
                "linearGradient"
            }
            GradientKind::Radial => {
                write!(
                    self.definitions,
                    "<radialGradient id=\"{id}\" cx=\"0.5\" cy=\"0.5\" r=\"0.5\">"
                )
                .unwrap();
                "radialGradient"
            }
        };
        for stop in &gradient.stops {
            write!(
                self.definitions,
                "<stop offset=\"{}\" stop-color=\"#{:02x}{:02x}{:02x}\"",
                stop.offset, stop.color.r, stop.color.g, stop.color.b
            )
            .unwrap();
            if stop.color.a != 255 {
                write!(
                    self.definitions,
                    " stop-opacity=\"{}\"",
                    f64::from(stop.color.a) / 255.0
                )
                .unwrap();
            }
            self.definitions.push_str("/>");
        }
        write!(self.definitions, "</{element}>").unwrap();
        id
    }

    fn insert_resource(&mut self, kind: &str, resource: Resource) -> String {
        let uri = format!("rssstv-{kind}:{}", self.next_resource);
        self.next_resource += 1;
        self.resources.insert(uri.clone(), resource);
        uri
    }
}

fn resolve_length(
    length: Length,
    size: RenderSize,
    font_size: Option<f64>,
) -> Result<f64, TemplateError> {
    let value = match length {
        Length::FrameWidth(percent) => f64::from(size.width()) * percent / 100.0,
        Length::FrameHeight(percent) => f64::from(size.height()) * percent / 100.0,
        Length::Em(multiplier) => {
            font_size
                .ok_or_else(|| TemplateError::Schema("(em) requires a text font size".into()))?
                * multiplier
        }
    };
    if !value.is_finite() {
        return Err(TemplateError::InvalidDimensions(
            "resolved geometry is not finite".into(),
        ));
    }
    Ok(value)
}

fn unit_coordinate(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// How far a block of wrapped lines moves up so the block, rather than its
/// first line, sits where the anchor asked for.
fn block_shift(anchor: Anchor) -> f64 {
    match anchor {
        Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => 0.0,
        Anchor::Center => 0.5,
        Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => 1.0,
    }
}

fn write_rotation(
    svg: &mut String,
    position: Position,
    offset_x: f64,
    offset_y: f64,
    size: RenderSize,
    font_size: Option<f64>,
) -> Result<(), TemplateError> {
    if position.rotation == 0.0 {
        return Ok(());
    }
    let x = offset_x + resolve_length(position.x, size, font_size)?;
    let y = offset_y + resolve_length(position.y, size, font_size)?;
    write!(svg, " transform=\"rotate({} {x} {y})\"", position.rotation).unwrap();
    Ok(())
}

fn anchor_offset(anchor: Anchor, width: f64, height: f64) -> (f64, f64) {
    match anchor {
        Anchor::TopLeft => (0.0, 0.0),
        Anchor::TopCenter => (width / 2.0, 0.0),
        Anchor::TopRight => (width, 0.0),
        Anchor::Center => (width / 2.0, height / 2.0),
        Anchor::BottomLeft => (0.0, height),
        Anchor::BottomCenter => (width / 2.0, height),
        Anchor::BottomRight => (width, height),
    }
}

fn text_anchor(anchor: Anchor) -> (&'static str, &'static str) {
    match anchor {
        Anchor::TopLeft => ("start", "hanging"),
        Anchor::TopCenter => ("middle", "hanging"),
        Anchor::TopRight => ("end", "hanging"),
        Anchor::Center => ("middle", "central"),
        Anchor::BottomLeft => ("start", "text-after-edge"),
        Anchor::BottomCenter => ("middle", "text-after-edge"),
        Anchor::BottomRight => ("end", "text-after-edge"),
    }
}

fn write_stroke(
    svg: &mut String,
    stroke: Stroke,
    size: RenderSize,
    font_size: Option<f64>,
) -> Result<(), TemplateError> {
    write_color(svg, "stroke", stroke.color);
    write!(
        svg,
        " stroke-width=\"{}\"",
        resolve_length(stroke.width, size, font_size)?
    )
    .unwrap();
    Ok(())
}

fn write_color(svg: &mut String, attribute: &str, color: Color) {
    write!(
        svg,
        " {attribute}=\"#{:02x}{:02x}{:02x}\"",
        color.r, color.g, color.b
    )
    .unwrap();
    if color.a != 255 {
        write!(
            svg,
            " {attribute}-opacity=\"{}\"",
            f64::from(color.a) / 255.0
        )
        .unwrap();
    }
}

fn escape_xml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
