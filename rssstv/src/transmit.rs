use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use image::imageops::FilterType;
use rssstv_audio::PlaybackWriter;
use rssstv_fskid::FskId;
use rssstv_modulator::Modulator;
use rssstv_sstv::{
    TransmissionEncoder,
    image::{ImageSize, Rgb8, RgbImage},
    mode::Mode,
};
use rssstv_template::{
    AssetError, AssetProvider, EncodedAsset, RenderContext, RenderSize, Renderer, Template,
    VariableValue, Variables, composite,
};

const PCM_BLOCK_SIZE: usize = 1_024;

#[derive(Clone, Debug)]
pub struct ComposeRequest {
    pub generation: u64,
    pub template_path: PathBuf,
    pub background_path: PathBuf,
    pub assets_dir: PathBuf,
    pub mode: Mode,
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
    context.received_image = Some(&background);
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxPhase {
    #[default]
    Idle,
    Priming,
    Producing,
    Draining,
    Complete,
    Cancelled,
    Failed,
}

impl TxPhase {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Priming | Self::Producing | Self::Draining)
    }
}

/// Where the image raster sits inside the transmission, in output samples.
///
/// The interface reports progress by row, so it needs the window the rows
/// occupy rather than the length of the audio around them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterTiming {
    pub start_samples: u64,
    pub samples: u64,
    pub rows: usize,
}

impl RasterTiming {
    /// Maps an audible sample position onto the row being transmitted.
    ///
    /// A position before the window is still in the leader and one past it is
    /// in the station identification; neither carries a row.
    pub fn progress_at(self, played_samples: u64) -> TxProgress {
        if self.rows == 0 || self.samples == 0 || played_samples < self.start_samples {
            return TxProgress::Leader;
        }
        let elapsed = played_samples - self.start_samples;
        if elapsed >= self.samples {
            return TxProgress::Identifying;
        }
        let rows = u128::from(elapsed) * self.rows as u128 / u128::from(self.samples);
        TxProgress::Scanning {
            rows: rows as usize,
            total: self.rows,
        }
    }
}

/// How far the audible transmission has advanced through the image raster.
///
/// The leader and the station identifier frame the raster and carry no rows,
/// so they are named rather than folded into a fraction that would sit pinned
/// at either end.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxProgress {
    /// Nothing is being transmitted.
    #[default]
    Idle,
    /// The VOX sequence, VIS framing, or leading segments are being sent.
    Leader,
    /// Image rows are being sent.
    Scanning { rows: usize, total: usize },
    /// The raster is finished; the footer and station identifier are being sent.
    Identifying,
    /// The whole transmission was sent.
    Complete,
}

impl TxProgress {
    /// Returns the transmitted fraction of the raster in `0.0..=1.0`.
    pub fn fraction(self) -> f32 {
        match self {
            Self::Idle | Self::Leader => 0.0,
            Self::Scanning { rows, total } if total > 0 => {
                (rows as f32 / total as f32).clamp(0.0, 1.0)
            }
            Self::Scanning { .. } => 0.0,
            Self::Identifying | Self::Complete => 1.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TxSnapshot {
    pub phase: TxPhase,
    pub generated_samples: u64,
    pub total_samples: u64,
    pub raster: RasterTiming,
    pub error: Option<String>,
}

pub struct TxWorker {
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TxWorker {
    pub fn spawn(
        writer: PlaybackWriter,
        mode: Mode,
        frame: Arc<RgbImage>,
        station_id: FskId,
    ) -> Self {
        let snapshot = Arc::new(Mutex::new(TxSnapshot {
            phase: TxPhase::Priming,
            ..TxSnapshot::default()
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_snapshot = Arc::clone(&snapshot);
        let worker_cancel = Arc::clone(&cancel);
        let thread = thread::spawn(move || {
            transmit_loop(
                writer,
                mode,
                frame,
                station_id,
                worker_snapshot,
                worker_cancel,
            )
        });
        Self {
            snapshot,
            cancel,
            thread: Some(thread),
        }
    }

    pub fn latest(&self) -> TxSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| TxSnapshot {
                phase: TxPhase::Failed,
                error: Some("transmit worker state is unavailable".to_owned()),
                ..TxSnapshot::default()
            })
    }
}

impl Drop for TxWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for TxWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxWorker")
            .field("snapshot", &self.latest())
            .finish_non_exhaustive()
    }
}

fn transmit_loop(
    mut writer: PlaybackWriter,
    mode: Mode,
    frame: Arc<RgbImage>,
    station_id: FskId,
    snapshot: Arc<Mutex<TxSnapshot>>,
    cancel: Arc<AtomicBool>,
) {
    let transmission = match TransmissionEncoder::new(mode, (*frame).clone(), station_id) {
        Ok(transmission) => transmission,
        Err(error) => return fail(&snapshot, error.to_string()),
    };
    let sample_rate_hz = writer.sample_rate_hz();
    let total_samples = samples_for(transmission.duration().as_picos(), sample_rate_hz);
    let raster = RasterTiming {
        start_samples: samples_for(transmission.raster_start().as_picos(), sample_rate_hz),
        samples: samples_for(transmission.raster_duration().as_picos(), sample_rate_hz),
        rows: usize::from(mode.spec().active_rows()),
    };
    update(&snapshot, |state| {
        state.total_samples = total_samples;
        state.raster = raster;
    });
    let mut modulator = match Modulator::new(transmission, sample_rate_hz) {
        Ok(modulator) => modulator,
        Err(error) => return fail(&snapshot, error.to_string()),
    };
    let prefill = writer
        .vacant()
        .min((sample_rate_hz as usize / 4).max(1))
        .min(total_samples as usize);
    let mut block = [0.0_f32; PCM_BLOCK_SIZE];
    let mut count = 0;
    let mut offset = 0;
    let mut primed = false;
    loop {
        if cancel.load(Ordering::Acquire) || writer.is_cancelled() {
            update(&snapshot, |state| state.phase = TxPhase::Cancelled);
            return;
        }
        if offset == count {
            match modulator.process(&mut block) {
                Ok(0) => {
                    writer.finish();
                    update(&snapshot, |state| state.phase = TxPhase::Draining);
                    return;
                }
                Ok(next) => {
                    count = next;
                    offset = 0;
                }
                Err(error) => return fail(&snapshot, error.to_string()),
            }
        }
        let written = writer.write(&block[offset..count]);
        if written == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        offset += written;
        update(&snapshot, |state| {
            state.generated_samples += written as u64;
            if !primed && state.generated_samples >= prefill as u64 {
                state.phase = TxPhase::Producing;
                primed = true;
            }
        });
    }
}

fn samples_for(duration_picos: u64, sample_rate_hz: u32) -> u64 {
    let scaled = u128::from(duration_picos) * u128::from(sample_rate_hz);
    u64::try_from(scaled.div_ceil(1_000_000_000_000)).expect("SSTV transmission fits u64")
}

fn update(snapshot: &Mutex<TxSnapshot>, update: impl FnOnce(&mut TxSnapshot)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

fn fail(snapshot: &Mutex<TxSnapshot>, error: String) {
    update(snapshot, |state| {
        state.phase = TxPhase::Failed;
        state.error = Some(error);
    });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rssstv_audio::synthetic_playback;
    use rstest::rstest;

    use super::*;

    /// The window a mode's rows occupy is narrower than the transmission, so
    /// the same played position means different things at either edge.
    #[rstest]
    #[case(0, TxProgress::Leader)]
    #[case(99, TxProgress::Leader)]
    #[case(100, TxProgress::Scanning { rows: 0, total: 40 })]
    #[case(105, TxProgress::Scanning { rows: 10, total: 40 })]
    #[case(119, TxProgress::Scanning { rows: 38, total: 40 })]
    #[case(120, TxProgress::Identifying)]
    #[case(400, TxProgress::Identifying)]
    fn played_samples_map_onto_the_row_being_transmitted(
        #[case] played_samples: u64,
        #[case] expected: TxProgress,
    ) {
        let raster = RasterTiming {
            start_samples: 100,
            samples: 20,
            rows: 40,
        };
        assert_eq!(raster.progress_at(played_samples), expected);
    }

    #[test]
    fn an_unknown_raster_window_reports_the_leader_rather_than_a_row() {
        assert_eq!(
            RasterTiming::default().progress_at(1_000),
            TxProgress::Leader
        );
    }

    #[rstest]
    #[case(TxProgress::Leader, 0.0)]
    #[case(TxProgress::Scanning { rows: 62, total: 124 }, 0.5)]
    #[case(TxProgress::Scanning { rows: 0, total: 0 }, 0.0)]
    #[case(TxProgress::Identifying, 1.0)]
    #[case(TxProgress::Complete, 1.0)]
    fn progress_maps_to_a_transmitted_fraction(
        #[case] progress: TxProgress,
        #[case] expected: f32,
    ) {
        assert_eq!(progress.fraction(), expected);
    }

    #[test]
    fn transmit_worker_streams_a_complete_bounded_transmission() {
        let mode = Mode::Robot36;
        let size = ImageSize::new(mode.spec().width().into(), mode.spec().height().into()).unwrap();
        let frame = Arc::new(RgbImage::new(size, Rgb8::new(80, 140, 200)));
        let (writer, mut reader) = synthetic_playback(8_000, 512).unwrap();
        let worker = TxWorker::spawn(writer, mode, frame, FskId::new("N0CALL").unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut received = 0_u64;
        let mut block = [0.0_f32; 512];
        loop {
            let snapshot = worker.latest();
            if snapshot.phase == TxPhase::Producing || snapshot.phase == TxPhase::Draining {
                received += reader.read(&mut block) as u64;
            }
            if snapshot.phase == TxPhase::Draining && reader.is_complete() {
                assert_eq!(received, snapshot.total_samples);
                assert_eq!(snapshot.generated_samples, snapshot.total_samples);
                let raster = snapshot.raster;
                assert_eq!(raster.rows, usize::from(mode.spec().active_rows()));
                // The leader precedes the window and the identifier follows it.
                assert!(raster.start_samples > 0);
                assert!(raster.start_samples + raster.samples < snapshot.total_samples);
                break;
            }
            assert!(snapshot.phase != TxPhase::Failed, "{:?}", snapshot.error);
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
    }

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
