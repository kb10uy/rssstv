use std::error::Error;

mod app;
mod audio;
mod canvas;
mod i18n;
mod paths;
mod raster;
mod receive;
mod view;

fn main() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;
    app::run(paths)?;
    Ok(())
}
