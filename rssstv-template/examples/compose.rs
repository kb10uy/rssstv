use std::{env, error::Error, fs, io, path::Path};

use rssstv_sstv::image::{ImageSize, Rgb8, RgbImage};
use rssstv_template::{
    FileAssetProvider, RenderContext, RenderSize, Renderer, Template, Variables, composite,
};

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

    let assets = FileAssetProvider::new(env::current_dir()?);
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
    Ok(RgbImage::from_rgb_bytes(size, decoded.as_raw())?)
}

fn save_rgb_image(image: &RgbImage, path: &Path) -> Result<(), Box<dyn Error>> {
    let width = u32::try_from(image.size().width())?;
    let height = u32::try_from(image.size().height())?;
    let output =
        image::RgbImage::from_raw(width, height, image.to_rgb_bytes()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "composed image dimensions do not match its pixels",
            )
        })?;
    output.save(path)?;
    Ok(())
}
