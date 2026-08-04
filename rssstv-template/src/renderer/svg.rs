use std::{collections::HashMap, fmt::Write};

use crate::{
    RenderSize, TemplateError,
    renderer::{
        RenderContext,
        asset::{Resource, encode_received_image, validate_png},
        variable::interpolate,
    },
    scene::{
        Anchor, Color, GroupLayer, ImageFit, ImageLayer, Layer, LayerSize, Length, Position,
        ReceivedImageLayer, Stroke, TextLayer,
    },
};

pub(super) struct SvgGenerator<'a> {
    size: RenderSize,
    context: &'a RenderContext<'a>,
    pub(super) resources: HashMap<String, Resource>,
    asset_uris: HashMap<String, String>,
    received_uri: Option<String>,
    next_resource: usize,
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
        }
    }

    pub(super) fn generate(&mut self, layers: &[Layer]) -> Result<String, TemplateError> {
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="0 0 {} {}">"#,
            self.size.width(),
            self.size.height(),
            self.size.width(),
            self.size.height()
        );
        self.write_layers(&mut svg, layers, 0.0, 0.0)?;
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
                    write_paint(svg, layer.fill, layer.stroke, self.size, None)?;
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
                    write_paint(svg, layer.fill, layer.stroke, self.size, None)?;
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
        svg.push_str("<g>");
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
        self.write_image_element(
            svg,
            layer.position,
            layer.size,
            &uri,
            &resource,
            (offset_x, offset_y),
        )
    }

    fn write_received_image(
        &mut self,
        svg: &mut String,
        layer: &ReceivedImageLayer,
        offset_x: f64,
        offset_y: f64,
    ) -> Result<(), TemplateError> {
        let (uri, resource) = self.received_resource()?;
        self.write_image_element(
            svg,
            layer.position,
            layer.size,
            &uri,
            &resource,
            (offset_x, offset_y),
        )
    }

    fn write_image_element(
        &self,
        svg: &mut String,
        position: Position,
        size: LayerSize,
        uri: &str,
        resource: &Resource,
        offset: (f64, f64),
    ) -> Result<(), TemplateError> {
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
            "<image href=\"{}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"{preserve}\"/>",
            escape_xml(uri)
        )
        .unwrap();
        Ok(())
    }

    fn write_text(
        &self,
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
        write!(
            svg,
            "<text x=\"{x}\" y=\"{y}\" text-anchor=\"{text_anchor}\" dominant-baseline=\"{baseline}\" font-family=\"{}\" font-size=\"{font_size}\" font-weight=\"{}\" font-style=\"{}\"",
            escape_xml(&layer.font.family),
            layer.font.weight,
            layer.font.style.as_svg()
        )
        .unwrap();
        write_color(svg, "fill", layer.fill);
        if let Some(stroke) = layer.stroke {
            write_stroke(svg, stroke, self.size, Some(font_size))?;
            svg.push_str(" paint-order=\"stroke fill\" stroke-linejoin=\"round\"");
        }
        write!(svg, ">{}</text>", escape_xml(&text)).unwrap();
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

fn write_paint(
    svg: &mut String,
    fill: Option<Color>,
    stroke: Option<Stroke>,
    size: RenderSize,
    font_size: Option<f64>,
) -> Result<(), TemplateError> {
    match fill {
        Some(color) => write_color(svg, "fill", color),
        None => svg.push_str(" fill=\"none\""),
    }
    if let Some(stroke) = stroke {
        write_stroke(svg, stroke, size, font_size)?;
    }
    Ok(())
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
