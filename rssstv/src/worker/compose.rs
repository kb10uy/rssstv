use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use image::imageops::FilterType;
use rssstv_sstv::{
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};
use rssstv_template::{
    AssetError, AssetProvider, EncodedAsset, RenderContext, RenderSize, Renderer, Template,
    VariableValue, Variables, composite,
};

#[derive(Clone, Debug)]
pub struct ComposeRequest {
    pub generation: u64,
    pub template_path: PathBuf,
    pub background_path: PathBuf,
    pub assets_dir: PathBuf,
    pub mode: Mode,
    /// The image `rximage` layers show, when a reception has produced one.
    pub received_image: Option<Arc<RgbImage>>,
    pub station_callsign: String,
    pub contact_callsign: String,
    pub report: String,
    pub number: String,
}

#[derive(Debug)]
pub struct ComposeResult {
    pub generation: u64,
    pub frame: Result<Arc<RgbImage>, String>,
}

struct ComposeControl {
    request: Mutex<Option<ComposeRequest>>,
    wake: Condvar,
    stop: AtomicBool,
}

pub struct Composer {
    control: Arc<ComposeControl>,
    result: Arc<Mutex<Option<ComposeResult>>>,
    thread: Option<JoinHandle<()>>,
}

impl Composer {
    pub fn spawn() -> Self {
        let control = Arc::new(ComposeControl {
            request: Mutex::new(None),
            wake: Condvar::new(),
            stop: AtomicBool::new(false),
        });
        let result = Arc::new(Mutex::new(None));
        let worker_control = Arc::clone(&control);
        let worker_result = Arc::clone(&result);
        let thread = thread::spawn(move || compose_loop(worker_control, worker_result));
        Self {
            control,
            result,
            thread: Some(thread),
        }
    }

    pub fn request(&self, request: ComposeRequest) {
        if let Ok(mut pending) = self.control.request.lock() {
            *pending = Some(request);
            self.control.wake.notify_one();
        }
    }

    pub fn latest(&self) -> Option<ComposeResult> {
        self.result.lock().ok()?.take()
    }
}

impl Drop for Composer {
    fn drop(&mut self) {
        self.control.stop.store(true, Ordering::Release);
        self.control.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for Composer {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Composer").finish_non_exhaustive()
    }
}

fn compose_loop(control: Arc<ComposeControl>, result: Arc<Mutex<Option<ComposeResult>>>) {
    let mut renderer = Renderer::new();
    renderer.load_system_fonts();
    loop {
        let request = {
            let Ok(mut pending) = control.request.lock() else {
                return;
            };
            while pending.is_none() && !control.stop.load(Ordering::Acquire) {
                let Ok(next) = control.wake.wait(pending) else {
                    return;
                };
                pending = next;
            }
            if control.stop.load(Ordering::Acquire) {
                return;
            }
            pending.take()
        };
        let Some(request) = request else {
            continue;
        };
        let generation = request.generation;
        let frame = compose_frame(&request, &mut renderer).map(Arc::new);
        if let Ok(mut output) = result.lock() {
            *output = Some(ComposeResult { generation, frame });
        }
    }
}

fn compose_frame(request: &ComposeRequest, renderer: &mut Renderer) -> Result<RgbImage, String> {
    let source = fs::read_to_string(&request.template_path)
        .map_err(|error| format!("{}: {error}", request.template_path.display()))?;
    let template = Template::parse(&source).map_err(|error| error.to_string())?;
    let background = load_background(&request.background_path, request.mode)?;
    let size = RenderSize::new(
        u32::try_from(background.size().width()).map_err(|error| error.to_string())?,
        u32::try_from(background.size().height()).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let assets = FileAssets {
        template_dir: request
            .template_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        assets_dir: request.assets_dir.clone(),
    };
    let variables = variables(request);
    let mut context = RenderContext::new(&variables, &assets);
    // Before the first reception worth keeping there is no received image, and
    // a template built around one would otherwise refuse to render at all. The
    // prepared background stands in until a reception replaces it, so the
    // preview shows the layout instead of an error.
    context.received_image = Some(request.received_image.as_deref().unwrap_or(&background));
    let overlay = renderer
        .render(&template, size, &context)
        .map_err(|error| error.to_string())?;
    composite(&background, &overlay).map_err(|error| error.to_string())
}

fn load_background(path: &Path, mode: Mode) -> Result<RgbImage, String> {
    let decoded = image::open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?
        .to_rgb8();
    let prepared = cover_image(
        &decoded,
        u32::from(mode.spec().width()),
        u32::from(mode.spec().height()),
    );
    let size = ImageSize::new(prepared.width() as usize, prepared.height() as usize)
        .map_err(|error| error.to_string())?;
    let pixels = prepared
        .pixels()
        .map(|pixel| Rgb8::new(pixel[0], pixel[1], pixel[2]))
        .collect();
    RgbImage::from_pixels(size, pixels).map_err(|error| error.to_string())
}

fn cover_image(source: &image::RgbImage, width: u32, height: u32) -> image::RgbImage {
    let source_width = source.width();
    let source_height = source.height();
    let (resized_width, resized_height) = if u64::from(width) * u64::from(source_height)
        >= u64::from(height) * u64::from(source_width)
    {
        (
            width,
            u32::try_from(
                (u64::from(source_height) * u64::from(width)).div_ceil(u64::from(source_width)),
            )
            .expect("resized height fits u32"),
        )
    } else {
        (
            u32::try_from(
                (u64::from(source_width) * u64::from(height)).div_ceil(u64::from(source_height)),
            )
            .expect("resized width fits u32"),
            height,
        )
    };
    let resized =
        image::imageops::resize(source, resized_width, resized_height, FilterType::Lanczos3);
    image::imageops::crop_imm(
        &resized,
        (resized_width - width) / 2,
        (resized_height - height) / 2,
        width,
        height,
    )
    .to_image()
}

fn variables(request: &ComposeRequest) -> Variables {
    let mut variables = Variables::new();
    for (name, value) in [
        ("mycall", &request.station_callsign),
        ("station.callsign", &request.station_callsign),
        ("contact.callsign", &request.contact_callsign),
        ("report.sent", &request.report),
        ("report.number", &request.number),
    ] {
        variables.insert(name, VariableValue::Text(value.clone()));
    }
    variables
}

struct FileAssets {
    template_dir: PathBuf,
    assets_dir: PathBuf,
}

impl AssetProvider for FileAssets {
    fn load(&self, reference: &str) -> Result<Option<EncodedAsset>, AssetError> {
        let local = self.template_dir.join(reference);
        let shared = Path::new(reference).strip_prefix("assets").map_or_else(
            |_| self.assets_dir.join(reference),
            |path| self.assets_dir.join(path),
        );
        for path in [local, shared] {
            match fs::read(&path) {
                Ok(bytes) => return Ok(Some(EncodedAsset::png(bytes))),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(AssetError::new(format!(
                        "failed to read {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_resize_matches_mode_dimensions() {
        let source = image::RgbImage::from_fn(4, 2, |x, _| image::Rgb([x as u8, 0, 0]));
        let result = cover_image(&source, 2, 2);
        assert_eq!(result.dimensions(), (2, 2));
        assert!(result[(0, 0)][0] > 0);
        assert!(result[(1, 0)][0] < 3);
    }

    #[test]
    fn template_variables_include_station_and_contact_fields() {
        let request = ComposeRequest {
            generation: 1,
            template_path: PathBuf::new(),
            background_path: PathBuf::new(),
            assets_dir: PathBuf::new(),
            mode: Mode::Robot36,
            received_image: None,
            station_callsign: "JA1ABC".to_owned(),
            contact_callsign: "N0CALL".to_owned(),
            report: "595".to_owned(),
            number: "001".to_owned(),
        };
        let variables = variables(&request);
        assert_eq!(
            variables.get("station.callsign"),
            Some(&VariableValue::Text("JA1ABC".to_owned()))
        );
        assert_eq!(
            variables.get("contact.callsign"),
            Some(&VariableValue::Text("N0CALL".to_owned()))
        );
    }
}
