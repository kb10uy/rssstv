use std::collections::HashMap;
use std::fmt::Write;
use std::io::Cursor;
use std::sync::Arc;

use image::{GenericImageView, ImageEncoder};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, ImageKind};
use rssstv_sstv::image::RgbImage;

use crate::scene::{
    Anchor, Color, GroupLayer, ImageFit, ImageLayer, Layer, LayerSize, Length, Position,
    ReceivedImageLayer, Stroke, Template, TextLayer, Variables,
};
use crate::{AssetError, RenderSize, Rgba8, RgbaImage, TemplateError};

/// PNG bytes returned by an [`AssetProvider`].
#[derive(Clone, Debug)]
pub struct EncodedAsset {
    png: Arc<Vec<u8>>,
}

impl EncodedAsset {
    /// Wraps PNG-encoded bytes.
    pub fn png(bytes: Vec<u8>) -> Self {
        Self {
            png: Arc::new(bytes),
        }
    }
}

/// Resolves image references without granting the renderer filesystem access.
pub trait AssetProvider: Send + Sync {
    /// Loads one PNG reference, returning `None` when it does not exist.
    fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError>;
}

/// An asset provider that contains no images.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyAssetProvider;

impl AssetProvider for EmptyAssetProvider {
    fn load(&self, _reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
        Ok(None)
    }
}

/// Caller-owned values and images used for one render.
pub struct RenderContext<'a> {
    /// Values available to `${...}` text interpolation.
    pub variables: &'a Variables,
    /// Final received image used by `rximage`, when available.
    pub received_image: Option<&'a RgbImage>,
    /// Resolver for PNG references used by `image` layers.
    pub assets: &'a dyn AssetProvider,
}

impl<'a> RenderContext<'a> {
    /// Constructs a context without a received image.
    pub const fn new(variables: &'a Variables, assets: &'a dyn AssetProvider) -> Self {
        Self {
            variables,
            received_image: None,
            assets,
        }
    }
}

/// A reusable SVG-backed template renderer and font database.
#[derive(Clone, Debug)]
pub struct Renderer {
    fontdb: Arc<usvg::fontdb::Database>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    /// Constructs a renderer with an empty font database.
    pub fn new() -> Self {
        Self {
            fontdb: Arc::new(usvg::fontdb::Database::new()),
        }
    }

    /// Registers all faces contained in an in-memory font file.
    pub fn load_font_data(&mut self, data: Vec<u8>) -> Result<(), TemplateError> {
        let database = Arc::make_mut(&mut self.fontdb);
        let previous = database.len();
        database.load_font_data(data);
        if database.len() == previous {
            return Err(TemplateError::Schema(
                "font data contains no supported faces".into(),
            ));
        }
        Ok(())
    }

    /// Explicitly loads fonts from platform font directories.
    pub fn load_system_fonts(&mut self) {
        Arc::make_mut(&mut self.fontdb).load_system_fonts();
    }

    /// Renders a template to a straight-alpha RGBA overlay.
    pub fn render(
        &self,
        template: &Template,
        size: RenderSize,
        context: &RenderContext<'_>,
    ) -> Result<RgbaImage, TemplateError> {
        validate_fonts(&template.layers, &self.fontdb)?;
        let mut generator = SvgGenerator::new(size, context);
        let svg = generator.generate(&template.layers)?;
        let resources = Arc::new(generator.resources);

        let mut options = usvg::Options {
            fontdb: self.fontdb.clone(),
            ..usvg::Options::default()
        };
        options.resources_dir = None;
        options.image_href_resolver.resolve_string = Box::new(move |href, _| {
            resources
                .get(href)
                .cloned()
                .map(|resource| ImageKind::PNG(resource.png))
        });

        let tree = usvg::Tree::from_str(&svg, &options)?;
        let mut pixmap = Pixmap::new(size.width(), size.height()).ok_or_else(|| {
            TemplateError::InvalidDimensions("resvg rejected the render dimensions".into())
        })?;
        let mut target = pixmap.as_mut();
        resvg::render(&tree, Transform::identity(), &mut target);
        let bytes = pixmap.take_demultiplied();
        let pixels = bytes
            .chunks_exact(4)
            .map(|pixel| Rgba8::new(pixel[0], pixel[1], pixel[2], pixel[3]))
            .collect();
        RgbaImage::from_pixels(size, pixels)
    }
}

#[derive(Clone)]
struct Resource {
    png: Arc<Vec<u8>>,
    width: u32,
    height: u32,
}

struct SvgGenerator<'a> {
    size: RenderSize,
    context: &'a RenderContext<'a>,
    resources: HashMap<String, Resource>,
    asset_uris: HashMap<String, String>,
    received_uri: Option<String>,
    next_resource: usize,
}

impl<'a> SvgGenerator<'a> {
    fn new(size: RenderSize, context: &'a RenderContext<'a>) -> Self {
        Self {
            size,
            context,
            resources: HashMap::new(),
            asset_uris: HashMap::new(),
            received_uri: None,
            next_resource: 0,
        }
    }

    fn generate(&mut self, layers: &[Layer]) -> Result<String, TemplateError> {
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

fn validate_fonts(
    layers: &[Layer],
    database: &usvg::fontdb::Database,
) -> Result<(), TemplateError> {
    for layer in layers {
        match layer {
            Layer::Text(text) => {
                let available = database.faces().any(|face| {
                    face.families
                        .iter()
                        .any(|(family, _)| family.eq_ignore_ascii_case(&text.font.family))
                });
                if !available {
                    return Err(TemplateError::MissingFont(text.font.family.clone()));
                }
            }
            Layer::Group(group) => validate_fonts(&group.layers, database)?,
            _ => {}
        }
    }
    Ok(())
}

fn validate_png(png: Arc<Vec<u8>>) -> Result<Resource, TemplateError> {
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    let (width, height) = image.dimensions();
    Ok(Resource { png, width, height })
}

fn encode_received_image(image: &RgbImage) -> Result<Resource, TemplateError> {
    let width = u32::try_from(image.size().width()).map_err(|_| {
        TemplateError::InvalidDimensions("received image width exceeds PNG limits".into())
    })?;
    let height = u32::try_from(image.size().height()).map_err(|_| {
        TemplateError::InvalidDimensions("received image height exceeds PNG limits".into())
    })?;
    let mut rgb = Vec::with_capacity(image.pixels().len() * 3);
    for pixel in image.pixels() {
        rgb.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut png)).write_image(
        &rgb,
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(Resource {
        png: Arc::new(png),
        width,
        height,
    })
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

fn interpolate(source: &str, variables: &Variables) -> Result<String, TemplateError> {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(dollar) = rest.find('$') {
        output.push_str(&rest[..dollar]);
        rest = &rest[dollar + 1..];
        if let Some(after_escape) = rest.strip_prefix('$') {
            output.push('$');
            rest = after_escape;
            continue;
        }
        let Some(expression) = rest.strip_prefix('{') else {
            output.push('$');
            continue;
        };
        let Some(end) = expression.find('}') else {
            return Err(TemplateError::Schema(
                "unterminated variable interpolation".into(),
            ));
        };
        let name = &expression[..end];
        if !valid_variable_name(name) {
            return Err(TemplateError::Schema(format!(
                "invalid variable name `{name}`"
            )));
        }
        let value = variables
            .get(name)
            .ok_or_else(|| TemplateError::MissingVariable(name.to_owned()))?;
        write!(output, "{value}").unwrap();
        rest = &expression[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

fn valid_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::ImageEncoder;
    use rssstv_sstv::image::{ImageSize, Rgb8};

    use super::*;
    use crate::VariableValue;

    #[test]
    fn interpolates_values_and_escaped_dollars() {
        let mut variables = Variables::new();
        variables.insert("contact.callsign", VariableValue::Text("JA1ABC".into()));
        assert_eq!(
            interpolate("To ${contact.callsign}; $${literal}", &variables).unwrap(),
            "To JA1ABC; ${literal}"
        );
        assert!(matches!(
            interpolate("${station.callsign}", &variables),
            Err(TemplateError::MissingVariable(_))
        ));
    }

    #[test]
    fn emits_italic_svg_text() {
        let template = Template::parse(
            "text \"CQ SSTV\" { position x=(fw)0 y=(fh)0; font family=\"Monaspace Argon\" size=(fh)25 weight=700 style=\"italic\"; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let mut generator = SvgGenerator::new(RenderSize::new(320, 256).unwrap(), &context);
        let svg = generator.generate(template.layers()).unwrap();

        assert!(svg.contains("font-style=\"italic\""));
    }

    #[test]
    fn renders_shapes_as_a_transparent_overlay() {
        let template = Template::parse(
            r##"
rect {
    position x=(fw)25 y=(fh)25
    size width=(fw)50 height=(fh)50
    fill color="#ff000080"
}
ellipse {
    position x=(fw)50 y=(fh)50 anchor="center"
    size width=(fw)20 height=(fh)20
    fill color="#00ff00"
}
line {
    start x=(fw)0 y=(fh)0
    end x=(fw)100 y=(fh)100
    stroke color="#0000ff" width=(fh)1
}
group {
    position x=(fw)80 y=(fh)0
    rect {
        position x=(fw)0 y=(fh)0
        size width=(fw)20 height=(fh)20
        fill color="#ffffff"
    }
}
"##,
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let image = Renderer::new()
            .render(&template, RenderSize::new(100, 80).unwrap(), &context)
            .unwrap();

        assert_eq!(image.size(), RenderSize::new(100, 80).unwrap());
        assert_eq!(image.pixels()[70 * 100 + 10], Rgba8::default());
        assert!(image.pixels().iter().any(|pixel| pixel.a == 128));
        assert!(
            image
                .pixels()
                .iter()
                .any(|pixel| pixel.g == 255 && pixel.a == 255)
        );
        assert!(
            image
                .pixels()
                .iter()
                .any(|pixel| pixel.b == 255 && pixel.a > 0)
        );
        assert_eq!(image.pixels()[99], Rgba8::new(255, 255, 255, 255));
    }

    #[test]
    fn clips_layers_with_negative_positions_to_the_frame() {
        let template = Template::parse(
            r##"
rect {
    position x=(fw)-50 y=(fh)0
    size width=(fw)100 height=(fh)100
    fill color="#ff0000"
}
"##,
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let image = Renderer::new()
            .render(&template, RenderSize::new(10, 10).unwrap(), &context)
            .unwrap();

        assert_eq!(image.pixels()[0], Rgba8::new(255, 0, 0, 255));
        assert_eq!(image.pixels()[9], Rgba8::default());
    }

    struct SingleAsset(Vec<u8>);

    impl AssetProvider for SingleAsset {
        fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
            Ok((reference == "logo.png").then(|| EncodedAsset::png(self.0.clone())))
        }
    }

    #[test]
    fn renders_a_caller_resolved_png_asset() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(Cursor::new(&mut png))
            .write_image(&[100, 150, 200, 128], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let assets = SingleAsset(png);
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &assets);
        let template = Template::parse(
            "image \"logo.png\" { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100 fit=\"stretch\"; }",
        )
        .unwrap();
        let image = Renderer::new()
            .render(&template, RenderSize::new(2, 2).unwrap(), &context)
            .unwrap();

        assert!(
            image
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgba8::new(100, 149, 199, 128))
        );
    }

    #[test]
    fn renders_caller_provided_received_image() {
        let template = Template::parse(
            "rximage { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100 fit=\"stretch\"; }",
        )
        .unwrap();
        let received = RgbImage::new(ImageSize::new(2, 1).unwrap(), Rgb8::new(12, 34, 56));
        let variables = Variables::new();
        let mut context = RenderContext::new(&variables, &EmptyAssetProvider);
        context.received_image = Some(&received);
        let image = Renderer::new()
            .render(&template, RenderSize::new(4, 2).unwrap(), &context)
            .unwrap();
        assert!(
            image
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgba8::new(12, 34, 56, 255))
        );
    }

    #[test]
    fn requires_received_image_and_registered_fonts() {
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let rx = Template::parse(
            "rximage { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100; }",
        )
        .unwrap();
        assert!(matches!(
            Renderer::new().render(&rx, RenderSize::new(2, 2).unwrap(), &context),
            Err(TemplateError::MissingReceivedImage)
        ));

        let text = Template::parse(
            "text \"hello\" { position x=(fw)0 y=(fh)0; font family=\"Missing\" size=(fh)10 weight=400; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        assert!(matches!(
            Renderer::new().render(&text, RenderSize::new(20, 20).unwrap(), &context),
            Err(TemplateError::MissingFont(_))
        ));
    }
}
