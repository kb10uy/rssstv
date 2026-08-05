use std::{
    collections::BTreeMap,
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
use jiff::{Zoned, tz::TimeZone};
use rssstv_sstv::{
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};
use rssstv_template::{
    AssetError, AssetProvider, EncodedAsset, RenderContext, RenderSize, Renderer, Template,
    VariableValue, Variables, composite,
};

use crate::worker::rig::Reading;

#[derive(Clone, Debug)]
pub struct ComposeRequest {
    pub generation: u64,
    pub template_path: PathBuf,
    pub background_path: PathBuf,
    pub assets_dir: PathBuf,
    pub mode: Mode,
    /// The image `rximage` layers show.
    pub received_image: Arc<RgbImage>,
    /// When that image was adopted, which `${rx.timestamp...}` reads.
    pub received_at: Zoned,
    pub station_callsign: String,
    pub station_qth: String,
    pub station_grid: String,
    pub contact_callsign: String,
    pub report: String,
    pub number: String,
    pub report_received: String,
    /// Names the operator defined, offered as `${custom.<name>}`.
    pub custom: BTreeMap<String, String>,
    /// What the rig is tuned to, when rig control has read it.
    pub radio: Option<Reading>,
}

#[derive(Debug)]
pub struct ComposeResult {
    pub generation: u64,
    pub frame: Result<Arc<RgbImage>, String>,
    /// Whether the template that produced this frame shows the time.
    ///
    /// Such a frame is only true for as long as the clock agrees with it, so
    /// the interface composes again as the minute changes rather than leaving
    /// a stale time on the transmit tab.
    pub uses_timestamps: bool,
    /// Whether the template that produced this frame prints what the rig is
    /// tuned to, for the same reason and with the same consequence.
    pub uses_radio: bool,
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
        let thread = thread::Builder::new()
            .name("rssstv-compose".to_owned())
            .spawn(move || compose_loop(worker_control, worker_result))
            .expect("the composition thread should start");
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
        let composed = compose_frame(&request, &mut renderer, &mut backgrounds);
        let watched = composed
            .as_ref()
            .map_or(Watched::default(), |(_, watched)| *watched);
        let frame = composed.map(|(frame, _)| Arc::new(frame));
        if let Ok(mut output) = result.lock() {
            *output = Some(ComposeResult {
                generation,
                frame,
                uses_timestamps: watched.timestamps,
                uses_radio: watched.radio,
            });
        }
    }
}

/// What a composed frame stops being true with.
///
/// A frame that prints the clock or the frequency is only right for as long as
/// those agree with it, so the interface is told which of them it has to watch
/// rather than composing again on every change of either.
#[derive(Clone, Copy, Debug, Default)]
struct Watched {
    timestamps: bool,
    radio: bool,
}

fn compose_frame(
    request: &ComposeRequest,
    renderer: &mut Renderer,
    backgrounds: &mut BackgroundCache,
) -> Result<(RgbImage, Watched), String> {
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
    let watched = Watched {
        timestamps: template.uses_timestamps(&variables),
        radio: template.uses_radio(),
    };
    let mut context = RenderContext::new(&variables, &assets);
    context.received_image = Some(&request.received_image);
    let overlay = renderer
        .render(&template, size, &context)
        .map_err(|error| error.to_string())?;
    let frame = composite(background, &overlay).map_err(|error| error.to_string())?;
    Ok((frame, watched))
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

/// What `${radio.frequency}` and `${radio.band}` say until rig control speaks.
///
/// A fixed pair rather than an absent one: a template that prints the
/// frequency has to compose to something on the transmit tab before there is a
/// radio to ask, and a missing variable would refuse to render at all.
const PLACEHOLDER_FREQUENCY_MHZ: f64 = 7.178;
const PLACEHOLDER_BAND: &str = "40m";

/// What the radio variables say, from the rig when there is one to ask.
///
/// A rig tuned between the bands has no band to name. The variable is left
/// empty rather than filled with the placeholder: the frequency beside it is
/// real, and a band that contradicts it would be worse than none.
fn radio(request: &ComposeRequest) -> (f64, String) {
    match &request.radio {
        Some(reading) => (
            reading.frequency_hz as f64 / 1_000_000.0,
            reading.band.clone().unwrap_or_default(),
        ),
        None => (PLACEHOLDER_FREQUENCY_MHZ, PLACEHOLDER_BAND.to_owned()),
    }
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
        ("report.received", &request.report_received),
    ] {
        variables.insert(name, VariableValue::Text(value.clone()));
    }
    let (frequency_mhz, band) = radio(request);
    variables.insert("radio.band", VariableValue::Text(band));
    variables.insert(
        "application.version",
        VariableValue::Text(env!("CARGO_PKG_VERSION").to_owned()),
    );
    variables.insert("radio.frequency", VariableValue::Decimal(frequency_mhz));
    // An operator-defined name cannot collide with a built-in one: it only
    // ever appears under `custom.`, and the interface refuses a name no
    // template could reference.
    for (name, value) in &request.custom {
        variables.insert(format!("custom.{name}"), VariableValue::Text(value.clone()));
    }
    insert_timestamp(&mut variables, "tx.timestamp", &Zoned::now());
    insert_timestamp(&mut variables, "rx.timestamp", &request.received_at);
    variables
}

/// Offers one instant in both the zone the operator reads and the zone the
/// band works in, so a template chooses rather than converts.
fn insert_timestamp(variables: &mut Variables, name: &str, instant: &Zoned) {
    variables.insert(
        format!("{name}.local"),
        VariableValue::Timestamp(instant.clone()),
    );
    variables.insert(
        format!("{name}.utc"),
        VariableValue::Timestamp(instant.with_time_zone(TimeZone::UTC)),
    );
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
    use jiff::civil::date;

    use super::*;

    #[test]
    fn cover_resize_matches_mode_dimensions() {
        let source = image::RgbImage::from_fn(4, 2, |x, _| image::Rgb([x as u8, 0, 0]));
        let result = cover_image(&source, 2, 2);
        assert_eq!(result.dimensions(), (2, 2));
        assert!(result[(0, 0)][0] > 0);
        assert!(result[(1, 0)][0] < 3);
    }

    fn request() -> ComposeRequest {
        ComposeRequest {
            generation: 1,
            template_path: PathBuf::new(),
            background_path: PathBuf::new(),
            assets_dir: PathBuf::new(),
            mode: Mode::Robot36,
            received_image: Arc::new(RgbImage::new(
                ImageSize::new(1, 1).unwrap(),
                Rgb8::default(),
            )),
            received_at: date(2026, 8, 4)
                .at(18, 5, 0, 0)
                .in_tz("Asia/Tokyo")
                .unwrap(),
            station_callsign: "JA1ABC".to_owned(),
            station_qth: "Chiyoda, Tokyo".to_owned(),
            station_grid: "PM95uq".to_owned(),
            contact_callsign: "N0CALL".to_owned(),
            report: "595".to_owned(),
            number: "001".to_owned(),
            report_received: "579".to_owned(),
            custom: BTreeMap::from([("club".to_owned(), "JARL".to_owned())]),
            radio: None,
        }
    }

    #[test]
    fn template_variables_include_station_and_contact_fields() {
        let variables = variables(&request());
        for (name, text) in [
            ("station.callsign", "JA1ABC"),
            ("station.grid", "PM95uq"),
            ("contact.callsign", "N0CALL"),
            ("report.sent", "595"),
            ("report.received", "579"),
            ("custom.club", "JARL"),
        ] {
            assert_eq!(
                variables.get(name),
                Some(&VariableValue::Text(text.to_owned())),
                "{name} should be offered to the template"
            );
        }
    }

    /// A transmit tab has to compose before there is a rig to ask, so the
    /// radio variables stand in for one rather than being absent.
    #[test]
    fn the_radio_variables_fall_back_on_a_placeholder_without_a_rig() {
        let variables = variables(&request());
        assert_eq!(
            variables.get("radio.frequency"),
            Some(&VariableValue::Decimal(PLACEHOLDER_FREQUENCY_MHZ))
        );
        assert_eq!(
            variables.get("radio.band"),
            Some(&VariableValue::Text(PLACEHOLDER_BAND.to_owned()))
        );
    }

    #[test]
    fn the_radio_variables_follow_what_the_rig_is_tuned_to() {
        let variables = variables(&ComposeRequest {
            radio: Some(Reading {
                frequency_hz: 14_230_000,
                mode: "USB".to_owned(),
                band: Some("20m".to_owned()),
            }),
            ..request()
        });

        assert_eq!(
            variables.get("radio.frequency"),
            Some(&VariableValue::Decimal(14.23))
        );
        assert_eq!(
            variables.get("radio.band"),
            Some(&VariableValue::Text("20m".to_owned()))
        );
    }

    /// The frequency beside it is real, so a band that contradicted it would
    /// be worse than none at all.
    #[test]
    fn a_rig_tuned_between_the_bands_names_no_band() {
        let variables = variables(&ComposeRequest {
            radio: Some(Reading {
                frequency_hz: 6_000_000,
                mode: "USB".to_owned(),
                band: None,
            }),
            ..request()
        });

        assert_eq!(
            variables.get("radio.frequency"),
            Some(&VariableValue::Decimal(6.0))
        );
        assert_eq!(
            variables.get("radio.band"),
            Some(&VariableValue::Text(String::new()))
        );
    }

    /// A reception is timed once, and a template chooses the zone it prints
    /// rather than converting one itself.
    #[test]
    fn a_reception_is_offered_in_both_zones() {
        let variables = variables(&request());
        let Some(VariableValue::Timestamp(local)) = variables.get("rx.timestamp.local") else {
            panic!("the local reception time should be offered");
        };
        let Some(VariableValue::Timestamp(utc)) = variables.get("rx.timestamp.utc") else {
            panic!("the UTC reception time should be offered");
        };
        assert_eq!(local.timestamp(), utc.timestamp());
        assert_eq!(local.hour(), 18);
        assert_eq!(utc.hour(), 9);
    }

    /// Composing again for a new template or callsign must not pay for the
    /// background a second time. Preparing one allocates its pixels, and the
    /// replacement is allocated before the previous image is dropped, so the
    /// same buffer coming back is proof that nothing was decoded again.
    #[test]
    fn a_prepared_background_is_reused_while_the_file_is_unchanged() {
        let directory = crate::test_util::TempDir::new();
        let path = directory.path().join("background.png");
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
        let directory = crate::test_util::TempDir::new();
        let path = directory.path().join("background.png");
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
        let directory = crate::test_util::TempDir::new();
        let path = directory.path().join("background.png");
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
        let directory = crate::test_util::TempDir::new();
        let path = directory.path().join("background.png");
        image::RgbImage::from_pixel(8, 6, image::Rgb([10, 20, 30]))
            .save(&path)
            .unwrap();
        let mut cache = BackgroundCache::default();

        cache.prepare(&path, Mode::Robot36).unwrap();
        let prepared = cache.prepare(&path, Mode::Pd120).unwrap();

        assert_eq!(prepared.size().width(), Mode::Pd120.spec().width() as usize);
    }
}
