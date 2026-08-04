use std::sync::Arc;

use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{self, ImageKind},
};
use rssstv_sstv::image::RgbImage;

use crate::{
    AssetError, RenderSize, Rgba8, RgbaImage, TemplateError,
    renderer::{asset::validate_fonts, svg::SvgGenerator},
    scene::{Template, Variables},
};

mod asset;
mod svg;
mod variable;

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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::ImageEncoder;
    use rssstv_sstv::image::{ImageSize, Rgb8};

    use super::*;

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
