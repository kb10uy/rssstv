use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::SystemTime,
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
    /// The image `rximage` layers show.
    pub received_image: Arc<RgbImage>,
    pub station_callsign: String,
    pub station_qth: String,
    pub station_grid: String,
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
    let mut backgrounds = BackgroundCache::default();
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
        let frame = compose_frame(&request, &mut renderer, &mut backgrounds).map(Arc::new);
        if let Ok(mut output) = result.lock() {
            *output = Some(ComposeResult { generation, frame });
        }
    }
}

fn compose_frame(
    request: &ComposeRequest,
    renderer: &mut Renderer,
    backgrounds: &mut BackgroundCache,
) -> Result<RgbImage, String> {
    let source = fs::read_to_string(&request.template_path)
        .map_err(|error| format!("{}: {error}", request.template_path.display()))?;
    let template = Template::parse(&source).map_err(|error| error.to_string())?;
    let background = backgrounds.prepare(&request.background_path, request.mode)?;
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
    context.received_image = Some(&request.received_image);
    let overlay = renderer
        .render(&template, size, &context)
        .map_err(|error| error.to_string())?;
    composite(background, &overlay).map_err(|error| error.to_string())
}

/// The prepared background the last composition used.
///
/// Decoding a background and resizing it to the mode is by far the slowest
/// part of a composition, and most compositions repeat one: a callsign edit, a
/// new reception, or another template all leave the picture alone. The file is
/// measured again on every request, so one replaced on disk is still picked
/// up.
#[derive(Debug, Default)]
struct BackgroundCache {
    prepared: Option<(BackgroundKey, RgbImage)>,
}

impl BackgroundCache {
    fn prepare(&mut self, path: &Path, mode: Mode) -> Result<&RgbImage, String> {
        let key = BackgroundKey::of(path, mode);
        // A file the filesystem will not describe cannot be told apart from
        // the one already held, so it is prepared again rather than assumed
        // unchanged.
        let reusable = key.stamp.is_some()
            && self
                .prepared
                .as_ref()
                .is_some_and(|(prepared, _)| *prepared == key);
        if !reusable {
            self.prepared = Some((key, load_background(path, mode)?));
        }
        Ok(&self
            .prepared
            .as_ref()
            .expect("the background was just prepared")
            .1)
    }
}

/// What a prepared background was prepared from.
#[derive(Debug, Eq, PartialEq)]
struct BackgroundKey {
    path: PathBuf,
    /// The mode decides the dimensions the background was cropped to.
    mode: Mode,
    /// The modification time and length of the file as it was read, or `None`
    /// when the filesystem would not say.
    stamp: Option<(SystemTime, u64)>,
}

impl BackgroundKey {
    fn of(path: &Path, mode: Mode) -> Self {
        let stamp = fs::metadata(path)
            .and_then(|metadata| Ok((metadata.modified()?, metadata.len())))
            .ok();
        Self {
            path: path.to_owned(),
            mode,
            stamp,
        }
    }
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
        ("station.callsign", &request.station_callsign),
        ("station.qth", &request.station_qth),
        ("station.grid", &request.station_grid),
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
    use std::sync::atomic::AtomicUsize;

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
            received_image: Arc::new(RgbImage::new(
                ImageSize::new(1, 1).unwrap(),
                Rgb8::default(),
            )),
            station_callsign: "JA1ABC".to_owned(),
            station_qth: "Chiyoda, Tokyo".to_owned(),
            station_grid: "PM95uq".to_owned(),
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
        assert_eq!(
            variables.get("station.grid"),
            Some(&VariableValue::Text("PM95uq".to_owned()))
        );
    }

    /// Composing again for a new template or callsign must not pay for the
    /// background a second time. Preparing one allocates its pixels, and the
    /// replacement is allocated before the previous image is dropped, so the
    /// same buffer coming back is proof that nothing was decoded again.
    #[test]
    fn a_prepared_background_is_reused_while_the_file_is_unchanged() {
        let directory = TestDirectory::new();
        let path = directory.0.join("background.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let mut cache = BackgroundCache::default();

        let first = cache
            .prepare(&path, Mode::Robot36)
            .unwrap()
            .pixels()
            .as_ptr();
        let second = cache
            .prepare(&path, Mode::Robot36)
            .unwrap()
            .pixels()
            .as_ptr();

        assert_eq!(first, second);
    }

    /// A background that is no longer there is reported rather than composed
    /// from what the cache happens to still hold.
    #[test]
    fn a_removed_background_file_is_an_error() {
        let directory = TestDirectory::new();
        let path = directory.0.join("background.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let mut cache = BackgroundCache::default();

        cache.prepare(&path, Mode::Robot36).unwrap();
        fs::remove_file(&path).unwrap();

        assert!(cache.prepare(&path, Mode::Robot36).is_err());
    }

    #[test]
    fn a_replaced_background_file_is_prepared_again() {
        let directory = TestDirectory::new();
        let path = directory.0.join("background.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let mut cache = BackgroundCache::default();

        let first = cache.prepare(&path, Mode::Robot36).unwrap().clone();
        // A different geometry gives the file a different length, so the
        // change is visible whatever the filesystem's timestamp resolution is.
        image::RgbImage::from_pixel(16, 12, image::Rgb([200, 100, 50]))
            .save(&path)
            .unwrap();

        assert_ne!(cache.prepare(&path, Mode::Robot36).unwrap(), &first);
    }

    /// The prepared image is cropped to the mode, so the mode is part of what
    /// the cache is keyed on.
    #[test]
    fn another_mode_prepares_the_background_again() {
        let directory = TestDirectory::new();
        let path = directory.0.join("background.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let mut cache = BackgroundCache::default();

        cache.prepare(&path, Mode::Robot36).unwrap();
        let prepared = cache.prepare(&path, Mode::Pd120).unwrap();

        assert_eq!(prepared.size().width(), Mode::Pd120.spec().width() as usize);
    }

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let index = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rssstv-compose-{}-{index}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
}
