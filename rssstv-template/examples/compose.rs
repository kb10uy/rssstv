use std::{
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};
use rssstv_template::{
    AssetError, AssetProvider, EncodedAsset, RenderContext, RenderSize, Renderer, Template,
    Variables, composite,
};

struct CwdAssetProvider {
    cwd: PathBuf,
}

impl AssetProvider for CwdAssetProvider {
    fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
        let path = self.cwd.join(reference);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(EncodedAsset::png(bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(AssetError::new(format!(
                "failed to read {}: {error}",
                path.display()
            ))),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let [template_path, background_path, output_path] = env::args()
        .skip(1)
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_: Vec<String>| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: compose <template.kdl> <background-image> <output-image>",
            )
        })?;

    let template = Template::parse(&fs::read_to_string(template_path)?)?;
    let background = load_rgb_image(Path::new(&background_path))?;
    let size = RenderSize::new(
        u32::try_from(background.size().width())?,
        u32::try_from(background.size().height())?,
    )?;
    let received_image = RgbImage::new(background.size(), Rgb8::new(255, 255, 255));

    let assets = CwdAssetProvider {
        cwd: env::current_dir()?,
    };
    let variables = Variables::new();
    let mut context = RenderContext::new(&variables, &assets);
    context.received_image = Some(&received_image);

    let mut renderer = Renderer::new();
    renderer.load_system_fonts();
    let overlay = renderer.render(&template, size, &context)?;
    let composed = composite(&background, &overlay)?;
    save_rgb_image(&composed, Path::new(&output_path))?;
    Ok(())
}

fn load_rgb_image(path: &Path) -> Result<RgbImage, Box<dyn Error>> {
    let decoded = image::open(path)?.to_rgb8();
    let size = ImageSize::new(decoded.width() as usize, decoded.height() as usize)?;
    let pixels = decoded
        .pixels()
        .map(|pixel| Rgb8::new(pixel[0], pixel[1], pixel[2]))
        .collect();
    Ok(RgbImage::from_pixels(size, pixels)?)
}

fn save_rgb_image(image: &RgbImage, path: &Path) -> Result<(), Box<dyn Error>> {
    let width = u32::try_from(image.size().width())?;
    let height = u32::try_from(image.size().height())?;
    let mut bytes = Vec::with_capacity(image.pixels().len() * 3);
    for pixel in image.pixels() {
        bytes.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    let output = image::RgbImage::from_raw(width, height, bytes).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "composed image dimensions do not match its pixels",
        )
    })?;
    output.save(path)?;
    Ok(())
}
