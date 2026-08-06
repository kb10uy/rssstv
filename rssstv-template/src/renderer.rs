use std::{
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{Arc, OnceLock},
};

use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg::{self, ImageKind},
};
use rssstv_sstv::image::RgbImage;

use crate::{
    AssetError, RenderSize, Rgba8, RgbaImage, TemplateError,
    renderer::{
        asset::{AssetFormat, validate_fonts},
        svg::SvgGenerator,
    },
    scene::{Template, Variables},
};

mod asset;
mod svg;
pub(crate) mod variable;

pub use variable::valid_variable_name;

/// Encoded image bytes returned by an [`AssetProvider`].
///
/// The provider hands over the file as it found it. Which format those bytes
/// are in is read from the bytes themselves, so a provider does not have to
/// trust, or even look at, the name it resolved.
#[derive(Clone, Debug)]
pub struct EncodedAsset {
    data: Arc<Vec<u8>>,
    validated: Arc<OnceLock<asset::Resource>>,
}

impl EncodedAsset {
    /// Wraps the encoded bytes of a PNG, JPEG, BMP, or WebP image.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            data: Arc::new(bytes),
            validated: Arc::new(OnceLock::new()),
        }
    }
}

/// Resolves image references without granting the renderer filesystem access.
pub trait AssetProvider: Send + Sync {
    /// Loads one image reference, returning `None` when it does not exist.
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

/// Whether a reference stays inside the directory it is resolved against.
///
/// Only plain relative steps qualify. A root, a drive prefix, or a parent
/// step names a file outside the directory the template was carried in, and
/// a template is not trusted that far.
pub(crate) fn reference_is_confined(reference: &str) -> bool {
    Path::new(reference)
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

/// An asset provider that reads references from one directory.
///
/// A reference is joined to the directory as it was written, so a template
/// carried alongside its images resolves them without being told where it
/// lives. A reference that would leave the directory is refused.
#[derive(Clone, Debug)]
pub struct FileAssetProvider {
    base: PathBuf,
}

impl FileAssetProvider {
    /// Resolves references against `base`.
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }
}

impl AssetProvider for FileAssetProvider {
    fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
        if !reference_is_confined(reference) {
            return Err(AssetError::new(format!(
                "asset reference `{reference}` reaches outside the asset directory"
            )));
        }
        let path = self.base.join(reference);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(EncodedAsset::new(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AssetError::new(format!(
                "failed to read {}: {error}",
                path.display()
            ))),
        }
    }
}

/// Caller-owned values and images used for one render.
pub struct RenderContext<'a> {
    /// Values available to `${...}` text interpolation.
    pub variables: &'a Variables,
    /// Final received image used by `rximage`, when available.
    pub received_image: Option<&'a RgbImage>,
    /// Resolver for image references used by `image` layers.
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
                .map(|resource| match resource.format {
                    AssetFormat::Png => ImageKind::PNG(resource.data),
                    AssetFormat::Jpeg => ImageKind::JPEG(resource.data),
                    AssetFormat::WebP => ImageKind::WEBP(resource.data),
                })
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

    use crate::VariableValue;

    use super::*;

    #[test]
    fn a_file_provider_refuses_references_that_leave_its_directory() {
        let provider = FileAssetProvider::new(std::env::temp_dir());
        for reference in ["../secret.png", "/etc/secret.png", "a/../../secret.png"] {
            assert!(
                provider.load(reference).is_err(),
                "`{reference}` should have been refused"
            );
        }
        assert!(matches!(provider.load("missing-asset.png"), Ok(None)));
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
    fn paints_text_from_a_defined_gradient() {
        let template = Template::parse(
            r##"
text "CQ SSTV" {
    position x=(fw)0 y=(fh)0
    font family="Monaspace Argon" size=(fh)25 weight=700
    fill gradient="linear" angle=90 {
        stop offset=0 color="#00ffff"
        stop offset=1 color="#00ff0080"
    }
}
"##,
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let mut generator = SvgGenerator::new(RenderSize::new(320, 256).unwrap(), &context);
        let svg = generator.generate(template.layers()).unwrap();

        assert!(svg.contains(
            "<defs><linearGradient id=\"gradient0\" x1=\"0.5\" y1=\"0\" x2=\"0.5\" y2=\"1\">"
        ));
        assert!(svg.contains("<stop offset=\"0\" stop-color=\"#00ffff\"/>"));
        assert!(svg.contains("stop-color=\"#00ff00\" stop-opacity=\""));
        assert!(svg.contains("fill=\"url(#gradient0)\""));
    }

    #[test]
    fn wraps_text_at_newlines_under_one_gradient() {
        let template = Template::parse(
            r##"
text "AAA\nBBB\nCCC" {
    position x=(fw)0 y=(fh)50 anchor="center"
    font family="Monaspace Argon" size=(fh)10 weight=400 leading=1.5
    fill color="#ffffff"
}
"##,
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let mut generator = SvgGenerator::new(RenderSize::new(100, 100).unwrap(), &context);
        let svg = generator.generate(template.layers()).unwrap();

        assert!(svg.contains("<tspan x=\"0\">AAA</tspan>"));
        assert!(svg.contains("<tspan x=\"0\" dy=\"15\">BBB</tspan>"));
        assert!(svg.contains("<tspan x=\"0\" dy=\"15\">CCC</tspan>"));
        assert!(svg.contains("y=\"35\""));
    }

    #[test]
    fn keeps_one_line_free_of_wrapping_markup() {
        let template = Template::parse(
            "text \"CQ\" { position x=(fw)0 y=(fh)50; font family=\"Monaspace Argon\" size=(fh)10 weight=400; fill color=\"#ffffff\"; }",
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let mut generator = SvgGenerator::new(RenderSize::new(100, 100).unwrap(), &context);
        let svg = generator.generate(template.layers()).unwrap();

        assert!(!svg.contains("tspan"));
        assert!(svg.contains(">CQ</text>"));
        assert!(svg.contains("y=\"50\""));
    }

    #[test]
    fn clips_rounds_and_rotates_layers() {
        let template = Template::parse(
            r##"
rximage {
    position x=(fw)0 y=(fh)0
    size width=(fw)50 height=(fh)100 fit="stretch"
    clip shape="circle"
}
rect {
    position x=(fw)50 y=(fh)0
    size width=(fw)50 height=(fh)50 radius=(fw)10
    fill color="#ff0000"
}
rect {
    position x=(fw)75 y=(fh)75 anchor="center" rotate=90
    size width=(fw)30 height=(fh)4
    fill color="#0000ff"
}
"##,
        )
        .unwrap();
        let received = RgbImage::new(ImageSize::new(2, 2).unwrap(), Rgb8::new(0, 255, 0));
        let variables = Variables::new();
        let mut context = RenderContext::new(&variables, &EmptyAssetProvider);
        context.received_image = Some(&received);
        let image = Renderer::new()
            .render(&template, RenderSize::new(100, 100).unwrap(), &context)
            .unwrap();
        let pixel = |x: usize, y: usize| image.pixels()[y * 100 + x];

        assert_eq!(pixel(1, 1), Rgba8::default());
        assert_eq!(pixel(25, 50), Rgba8::new(0, 255, 0, 255));

        assert_eq!(pixel(51, 1), Rgba8::default());
        assert_eq!(pixel(75, 25), Rgba8::new(255, 0, 0, 255));

        assert_eq!(pixel(75, 65), Rgba8::new(0, 0, 255, 255));
        assert_eq!(pixel(65, 75), Rgba8::default());
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
    fn fills_shapes_with_linear_and_radial_gradients() {
        let template = Template::parse(
            r##"
rect {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)50
    fill gradient="linear" {
        stop offset=0 color="#ff0000"
        stop offset=1 color="#0000ff"
    }
}
rect {
    position x=(fw)0 y=(fh)50
    size width=(fw)100 height=(fh)50
    fill gradient="radial" {
        stop offset=0 color="#00ff00"
        stop offset=1 color="#000000"
    }
}
"##,
        )
        .unwrap();
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &EmptyAssetProvider);
        let image = Renderer::new()
            .render(&template, RenderSize::new(64, 64).unwrap(), &context)
            .unwrap();
        let pixel = |x: usize, y: usize| image.pixels()[y * 64 + x];

        assert!(pixel(0, 16).r > 240 && pixel(0, 16).b < 16);
        assert!(pixel(63, 16).b > 240 && pixel(63, 16).r < 16);
        assert!(pixel(32, 16).r > 100 && pixel(32, 16).b > 100);

        assert!(pixel(32, 48).g > 240);
        assert!(pixel(0, 48).g < 16);
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
            Ok((reference == "logo.png").then(|| EncodedAsset::new(self.0.clone())))
        }
    }

    struct AnyAsset(Vec<u8>);

    impl AssetProvider for AnyAsset {
        fn load(&self, _reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
            Ok(Some(EncodedAsset::new(self.0.clone())))
        }
    }

    struct ReusableAsset(EncodedAsset);

    impl AssetProvider for ReusableAsset {
        fn load(&self, _reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
            Ok(Some(self.0.clone()))
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
    fn renders_a_caller_resolved_jpeg_asset() {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(Cursor::new(&mut jpeg))
            .write_image(&[100, 150, 200], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let assets = AnyAsset(jpeg);
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &assets);
        let template = Template::parse(
            "image \"logo.jpg\" { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100 fit=\"stretch\"; }",
        )
        .unwrap();
        let image = Renderer::new()
            .render(&template, RenderSize::new(2, 2).unwrap(), &context)
            .unwrap();

        assert!(image.pixels().iter().all(|pixel| {
            pixel.r.abs_diff(100) <= 8
                && pixel.g.abs_diff(150) <= 8
                && pixel.b.abs_diff(200) <= 8
                && pixel.a == 255
        }));
    }

    #[test]
    fn transcodes_a_caller_resolved_bmp_asset_to_png() {
        let mut bmp = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut Cursor::new(&mut bmp))
            .write_image(&[100, 150, 200], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let assets = AnyAsset(bmp);
        let variables = Variables::new();
        let context = RenderContext::new(&variables, &assets);
        let template = Template::parse(
            "image \"logo.bmp\" { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100 fit=\"stretch\"; }",
        )
        .unwrap();
        let image = Renderer::new()
            .render(&template, RenderSize::new(2, 2).unwrap(), &context)
            .unwrap();

        assert!(
            image
                .pixels()
                .iter()
                .all(|pixel| *pixel == Rgba8::new(100, 150, 200, 255))
        );
    }

    #[test]
    fn reuses_a_validated_asset_across_variable_only_renders() {
        let mut bmp = Vec::new();
        image::codecs::bmp::BmpEncoder::new(&mut Cursor::new(&mut bmp))
            .write_image(&[100, 150, 200], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        let assets = ReusableAsset(EncodedAsset::new(bmp));
        let template = Template::parse(
            "image \"logo.bmp\" { position x=(fw)0 y=(fh)0; size width=(fw)100 height=(fh)100 fit=\"stretch\"; }",
        )
        .unwrap();
        let renderer = Renderer::new();
        let first_variables = Variables::new();
        renderer
            .render(
                &template,
                RenderSize::new(2, 2).unwrap(),
                &RenderContext::new(&first_variables, &assets),
            )
            .unwrap();
        let first = assets.0.validated.get().unwrap().data.as_ptr();

        let mut second_variables = Variables::new();
        second_variables.insert("contact.callsign", VariableValue::Text("N0CALL".into()));
        renderer
            .render(
                &template,
                RenderSize::new(2, 2).unwrap(),
                &RenderContext::new(&second_variables, &assets),
            )
            .unwrap();

        assert_eq!(assets.0.validated.get().unwrap().data.as_ptr(), first);
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
