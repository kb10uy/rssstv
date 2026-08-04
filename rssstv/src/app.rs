use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use rssstv_audio::{InputDevice, OutputDevice, Playback, StreamFault};
use rssstv_fskid::FskId;
use rssstv_sstv::{
    image::RgbImage,
    mode::{Mode, Support},
};

use crate::{
    audio::AudioState,
    config::{Config, Settings, UI_SCALE_RANGE},
    i18n::{I18n, Locale},
    paths::AppPaths,
    platform::{self, Activity, Platform, reveal_directory},
    raster::Raster,
    receive::Progress,
    transmit::{ComposeRequest, Composer, TxPhase, TxProgress, TxSnapshot, TxWorker},
};

const PLAYBACK_QUEUE_SAMPLES: usize = 48_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Receive,
    Transmit,
    History,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Receive, Self::Transmit, Self::History];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Receive => "tab-receive",
            Self::Transmit => "tab-transmit",
            Self::History => "tab-history",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dsp {
    Afc,
    Lms,
    Slant,
}

impl Dsp {
    pub const ALL: [Self; 3] = [Self::Afc, Self::Lms, Self::Slant];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Afc => "dsp-afc",
            Self::Lms => "dsp-lms",
            Self::Slant => "dsp-slant",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DspFlags {
    pub afc: bool,
    pub lms: bool,
    pub slant: bool,
}

impl Default for DspFlags {
    fn default() -> Self {
        Self {
            afc: true,
            lms: false,
            slant: true,
        }
    }
}

impl DspFlags {
    pub const fn get(self, dsp: Dsp) -> bool {
        match dsp {
            Dsp::Afc => self.afc,
            Dsp::Lms => self.lms,
            Dsp::Slant => self.slant,
        }
    }

    const fn toggle(&mut self, dsp: Dsp) {
        match dsp {
            Dsp::Afc => self.afc = !self.afc,
            Dsp::Lms => self.lms = !self.lms,
            Dsp::Slant => self.slant = !self.slant,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Qso {
    pub call: String,
    pub rsv: String,
    pub number: String,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub geometry: String,
    path: PathBuf,
}

impl Entry {
    /// Builds an entry that names no real file, for tests.
    #[cfg(test)]
    pub(crate) fn sample(name: &str, geometry: &str) -> Self {
        Self {
            name: name.to_owned(),
            geometry: geometry.to_owned(),
            path: PathBuf::from(name),
        }
    }

    fn new(path: PathBuf, geometry: String) -> Self {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        Self {
            name,
            geometry,
            path,
        }
    }
}

pub struct App {
    pub tab: Tab,
    pub i18n: I18n,
    pub audio: AudioState,
    pub auto_mode: bool,
    pub rx_mode: Mode,
    pub tx_mode: Mode,
    pub rx_modes: Vec<Mode>,
    pub tx_modes: Vec<Mode>,
    pub dsp: DspFlags,
    pub auto_history: bool,
    pub qso: Qso,
    pub station_callsign: String,
    pub templates: Vec<Entry>,
    pub template: Option<usize>,
    pub stocks: Vec<Entry>,
    pub stock: Option<usize>,
    pub library_error: Option<String>,
    pub rx_raster: Raster,
    pub tx_raster: Raster,
    pub composite_raster: Raster,
    pub tx_snapshot: TxSnapshot,
    pub tx_error: Option<String>,
    /// The device fault waiting to be acknowledged, if one is.
    ///
    /// A lost device is reported in front of the interface rather than on the
    /// status line: it stops reception outright, and the operator has to
    /// choose a device before anything works again.
    pub device_fault: Option<StreamFault>,
    /// How much the whole interface is scaled.
    ///
    /// Held here rather than read from egui because it has to be restored
    /// before the first frame is laid out.
    pub ui_scale: f32,
    paths: AppPaths,
    config: Config,
    /// The settings as last written to disk.
    ///
    /// An immediate mode interface mutates state in place, so there is no
    /// message to hang a save off. Comparing against this at the end of the
    /// frame catches every change without the interface having to remember to
    /// announce one.
    saved: Settings,
    composer: Composer,
    compose_generation: u64,
    preview_frame: Option<Arc<RgbImage>>,
    prepared_frame: Option<Arc<RgbImage>>,
    playback: Option<Playback>,
    tx_worker: Option<TxWorker>,
    playback_started: bool,
    platform: Box<dyn Platform>,
    /// What the platform was last told the application is doing.
    ///
    /// Compared against each frame so a transition is reported once instead of
    /// restated on every frame.
    activity: Activity,
}

impl App {
    pub fn new(paths: AppPaths) -> Self {
        let config = Config::load(paths.config_file());
        let settings = config.settings();
        let audio = AudioState::new(
            settings.input_device.as_deref(),
            settings.output_device.as_deref(),
            settings.dsp.slant,
        );
        let mut app = Self::from_parts(audio, paths, config, &settings, platform::host());
        app.refresh_library();
        app.restore_selection(&settings);
        app.request_preview();
        app.saved = app.settings();
        app
    }

    /// Builds an interface with no host audio and no stored settings, for
    /// tests.
    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self::headless_on(Box::new(platform::InertPlatform))
    }

    /// Builds a headless interface reporting activity to `platform`.
    #[cfg(test)]
    pub(crate) fn headless_on(platform: Box<dyn Platform>) -> Self {
        Self::from_parts(
            AudioState::disconnected(),
            AppPaths::from_roots(
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
                PathBuf::new(),
            ),
            Config::detached(),
            &Settings::default(),
            platform,
        )
    }

    fn from_parts(
        audio: AudioState,
        paths: AppPaths,
        config: Config,
        settings: &Settings,
        platform: Box<dyn Platform>,
    ) -> Self {
        Self {
            tab: Tab::default(),
            i18n: I18n::new(settings.locale),
            audio,
            auto_mode: settings.auto_mode,
            rx_mode: settings.rx_mode,
            tx_mode: settings.tx_mode,
            rx_modes: modes(|mode| mode.spec().decode_support()),
            tx_modes: modes(|mode| mode.spec().encode_support()),
            dsp: settings.dsp,
            auto_history: settings.auto_history,
            qso: Qso::default(),
            station_callsign: settings.station_callsign.trim().to_ascii_uppercase(),
            templates: Vec::new(),
            template: None,
            stocks: Vec::new(),
            stock: None,
            library_error: None,
            rx_raster: Raster::blank(settings.rx_mode),
            tx_raster: Raster::test_pattern(settings.tx_mode),
            composite_raster: Raster::test_pattern(settings.tx_mode),
            tx_snapshot: TxSnapshot::default(),
            tx_error: None,
            device_fault: None,
            ui_scale: settings.ui_scale,
            paths,
            config,
            saved: settings.clone(),
            composer: Composer::spawn(),
            compose_generation: 0,
            preview_frame: None,
            prepared_frame: None,
            playback: None,
            tx_worker: None,
            playback_started: false,
            platform,
            activity: Activity::default(),
        }
    }

    pub fn title(&self) -> String {
        self.i18n.text("app-title")
    }

    /// Reselects the library entries named by `settings`.
    ///
    /// A name that no longer exists leaves the selection the library scan
    /// already made, so a deleted file does not empty the panel.
    fn restore_selection(&mut self, settings: &Settings) {
        self.template = index_of(&self.templates, settings.template.as_deref()).or(self.template);
        self.stock = index_of(&self.stocks, settings.stock.as_deref()).or(self.stock);
    }

    /// Returns the settings the interface is currently showing.
    fn settings(&self) -> Settings {
        Settings {
            locale: self.i18n.locale(),
            input_device: self
                .audio
                .device
                .as_ref()
                .map(|device| device.name().to_owned()),
            output_device: self
                .audio
                .output_device
                .as_ref()
                .map(|device| device.name().to_owned()),
            station_callsign: self.station_callsign.clone(),
            template: selected_name(&self.templates, self.template),
            stock: selected_name(&self.stocks, self.stock),
            rx_mode: self.rx_mode,
            tx_mode: self.tx_mode,
            auto_mode: self.auto_mode,
            dsp: self.dsp,
            auto_history: self.auto_history,
            ui_scale: self.ui_scale,
        }
    }

    /// Writes the settings back when anything the interface owns has changed.
    ///
    /// Called once at the end of every frame; a frame that changed nothing
    /// does not touch the disk.
    pub fn persist(&mut self) {
        let settings = self.settings();
        if settings != self.saved {
            self.config.store(&settings);
            self.saved = settings;
        }
    }

    pub fn config_error(&self) -> Option<&str> {
        self.config.error()
    }

    /// Scales the whole interface, within the range the setting allows.
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end());
    }

    /// Steps the scale, rounded so repeated steps stay on tidy values.
    pub fn zoom_by(&mut self, step: f32) {
        self.set_ui_scale(((self.ui_scale + step) * 10.0).round() / 10.0);
    }

    pub fn select_locale(&mut self, locale: Locale) {
        if locale != self.i18n.locale() {
            self.i18n = I18n::new(locale);
        }
    }

    pub fn select_device(&mut self, device: InputDevice) {
        self.audio.select(device);
    }

    pub fn select_output_device(&mut self, device: OutputDevice) {
        self.audio.select_output(device);
    }

    /// Switches to the input device with the given name, if the host has one.
    pub fn select_device_named(&mut self, name: &str) {
        if let Some(device) = self
            .audio
            .devices
            .iter()
            .find(|device| device.name() == name)
            .cloned()
        {
            self.select_device(device);
        }
    }

    pub fn select_output_device_named(&mut self, name: &str) {
        if let Some(device) = self
            .audio
            .output_devices
            .iter()
            .find(|device| device.name() == name)
            .cloned()
        {
            self.select_output_device(device);
        }
    }

    pub fn select_rx_mode(&mut self, mode: Mode) {
        if mode != self.rx_mode {
            self.rx_mode = mode;
            self.rx_raster = Raster::blank(mode);
        }
    }

    pub fn select_tx_mode(&mut self, mode: Mode) {
        if mode != self.tx_mode {
            self.tx_mode = mode;
            self.tx_raster = Raster::test_pattern(mode);
            self.composite_raster = Raster::test_pattern(mode);
            self.preview_frame = None;
            self.prepared_frame = None;
            self.request_preview();
        }
    }

    pub fn toggle_dsp(&mut self, dsp: Dsp) {
        self.dsp.toggle(dsp);
        if dsp == Dsp::Slant {
            self.audio.set_slant(self.dsp.slant);
        }
    }

    pub fn reveal_templates(&mut self) {
        self.library_error = reveal_directory(self.paths.templates_dir())
            .err()
            .map(|error| error.to_string());
    }

    /// Opens the directory holding the configuration file.
    ///
    /// The directory rather than the file itself: the application rewrites
    /// the file as settings change, and a `.toml` has no dependable handler
    /// on every platform.
    pub fn reveal_config(&mut self) {
        self.library_error = reveal_directory(self.paths.config_dir())
            .err()
            .map(|error| error.to_string());
    }

    pub fn reveal_stocks(&mut self) {
        self.library_error = reveal_directory(self.paths.stocks_dir())
            .err()
            .map(|error| error.to_string());
    }

    /// Uppercases the callsign field after an edit.
    pub fn normalize_call(&mut self) {
        if self.qso.call.chars().any(char::is_lowercase) {
            self.qso.call = self.qso.call.to_uppercase();
        }
    }

    pub fn normalize_station_callsign(&mut self) {
        let normalized = self.station_callsign.trim().to_ascii_uppercase();
        if normalized != self.station_callsign {
            self.station_callsign = normalized;
        }
        self.request_preview();
    }

    pub fn qso_changed(&mut self) {
        self.request_preview();
    }

    pub fn clear_qso(&mut self) {
        self.qso = Qso::default();
        self.request_preview();
    }

    fn refresh_library(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) = self.load_templates() {
            errors.push(error.to_string());
        }
        if let Err(error) = self.load_stocks() {
            errors.push(error.to_string());
        }
        self.library_error = (!errors.is_empty()).then(|| errors.join("; "));
    }

    pub fn refresh_templates(&mut self) {
        self.library_error = self.load_templates().err().map(|error| error.to_string());
        self.request_preview();
    }

    pub fn refresh_stocks(&mut self) {
        self.library_error = self.load_stocks().err().map(|error| error.to_string());
        self.request_preview();
    }

    fn load_templates(&mut self) -> io::Result<()> {
        let entries = template_entries(self.paths.templates_dir())?;
        replace_entries(&mut self.templates, &mut self.template, entries);
        Ok(())
    }

    fn load_stocks(&mut self) -> io::Result<()> {
        let entries = stock_entries(self.paths.stocks_dir())?;
        replace_entries(&mut self.stocks, &mut self.stock, entries);
        Ok(())
    }

    /// Adopts anything the receive worker produced since the last frame.
    pub fn poll_audio(&mut self) {
        self.poll_device_fault();
        if let Some(frame) = self.audio.poll()
            && let Some(raster) = Raster::from_frame(frame)
        {
            self.rx_raster = raster;
        }
        // A detected mode only takes over the selection while automatic
        // detection is on; otherwise it would undo the operator's choice.
        if self.auto_mode
            && let Some(mode) = self.audio.snapshot().mode
        {
            self.select_rx_mode(mode);
        }
        self.poll_transmit();
        self.report_activity();
    }

    /// Picks up a device that stopped and puts it in front of the operator.
    ///
    /// The device lists are enumerated again at the same time, so whatever the
    /// operator reaches for next describes what is attached now rather than
    /// what was attached when the application started.
    fn poll_device_fault(&mut self) {
        let Some(fault) = self.audio.take_capture_fault() else {
            return;
        };
        crate::log::note(&format!("capture stopped: {fault}"));
        self.audio.rescan();
        self.device_fault = Some(fault);
    }

    /// Opens the device again after a fault, keeping the report up if it is
    /// still not there.
    pub fn retry_device(&mut self) {
        self.audio.rescan();
        if self.audio.reopen() {
            self.device_fault = None;
        }
    }

    /// Acknowledges the fault without opening anything.
    pub fn dismiss_device_fault(&mut self) {
        self.device_fault = None;
    }

    /// Tells the platform what the application is doing, when that changes.
    ///
    /// Transmission outranks reception: the two cannot overlap on one device,
    /// and a transmission in progress is the stronger claim on the machine.
    fn report_activity(&mut self) {
        let activity = if self.tx_snapshot.phase.is_active() {
            Activity::Transmitting
        } else if matches!(
            self.audio.snapshot().progress,
            Progress::Acquiring | Progress::Decoding { .. }
        ) {
            Activity::Receiving
        } else {
            Activity::Idle
        };

        if activity != self.activity {
            self.activity = activity;
            self.platform.set_activity(activity);
        }
    }

    /// What the platform was last told the application is doing.
    #[cfg(test)]
    pub(crate) const fn activity(&self) -> Activity {
        self.activity
    }

    /// Fraction of the active tab's raster that is drawn as decoded.
    pub fn decoded_fraction(&self) -> f32 {
        match self.tab {
            Tab::Receive => self.audio.snapshot().progress.fraction(),
            Tab::Transmit => {
                if self.tx_snapshot.phase.is_active() {
                    self.tx_progress().fraction()
                } else {
                    1.0
                }
            }
            Tab::History => 1.0,
        }
    }

    pub const fn active_mode(&self) -> Mode {
        match self.tab {
            Tab::Transmit => self.tx_mode,
            Tab::Receive | Tab::History => self.rx_mode,
        }
    }

    pub const fn active_raster_mut(&mut self) -> &mut Raster {
        match self.tab {
            Tab::Transmit => &mut self.tx_raster,
            Tab::Receive | Tab::History => &mut self.rx_raster,
        }
    }

    pub fn preview_changed(&mut self) {
        self.request_preview();
    }

    fn request_preview(&mut self) {
        let (Some(template), Some(stock)) = (
            self.template.and_then(|index| self.templates.get(index)),
            self.stock.and_then(|index| self.stocks.get(index)),
        ) else {
            self.preview_frame = None;
            return;
        };
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.preview_frame = None;
        self.tx_error = None;
        self.composer.request(ComposeRequest {
            generation: self.compose_generation,
            template_path: template.path.clone(),
            background_path: stock.path.clone(),
            assets_dir: self.paths.assets_dir().to_path_buf(),
            mode: self.tx_mode,
            station_callsign: self.station_callsign.clone(),
            contact_callsign: self.qso.call.clone(),
            report: self.qso.rsv.clone(),
            number: self.qso.number.clone(),
        });
    }

    fn poll_transmit(&mut self) {
        if let Some(result) = self.composer.latest()
            && result.generation == self.compose_generation
        {
            match result.frame {
                Ok(frame) => {
                    self.composite_raster = Raster::from_image(&frame);
                    self.preview_frame = Some(frame);
                    self.tx_error = None;
                }
                Err(error) => {
                    self.preview_frame = None;
                    self.tx_error = Some(error);
                }
            }
        }

        let Some(worker) = self.tx_worker.as_ref() else {
            return;
        };
        self.tx_snapshot = worker.latest();
        if self.tx_snapshot.phase == TxPhase::Failed {
            self.tx_error = self.tx_snapshot.error.clone();
            self.stop_transmit_with(TxPhase::Failed);
            return;
        }
        let should_start = !self.playback_started
            && matches!(
                self.tx_snapshot.phase,
                TxPhase::Producing | TxPhase::Draining
            );
        if should_start {
            let result = self
                .playback
                .as_ref()
                .ok_or_else(|| "playback closed before transmission started".to_owned())
                .and_then(|playback| playback.play().map_err(|error| error.to_string()));
            match result {
                Ok(()) => self.playback_started = true,
                Err(error) => {
                    self.tx_error = Some(error);
                    self.stop_transmit_with(TxPhase::Failed);
                    return;
                }
            }
        }
        if self.playback_started
            && self
                .playback
                .as_ref()
                .is_some_and(|playback| playback.underrun_samples() > 0)
        {
            self.tx_error = Some("audio playback underrun".to_owned());
            self.stop_transmit_with(TxPhase::Failed);
            return;
        }
        if self.tx_snapshot.phase == TxPhase::Draining
            && self.playback.as_ref().is_some_and(Playback::is_complete)
        {
            self.stop_transmit_with(TxPhase::Complete);
        }
    }

    pub fn set_for_transmit(&mut self) {
        let Some(frame) = self.preview_frame.clone() else {
            return;
        };
        self.tx_raster = Raster::from_image(&frame);
        self.prepared_frame = Some(frame);
        self.tx_error = None;
    }

    pub fn can_set_for_transmit(&self) -> bool {
        self.preview_frame.is_some() && !self.tx_snapshot.phase.is_active()
    }

    pub fn can_transmit(&self) -> bool {
        self.transmit_problem().is_none() && !self.tx_snapshot.phase.is_active()
    }

    pub fn transmit_problem(&self) -> Option<String> {
        if self.prepared_frame.is_none() {
            return Some(self.i18n.text("error-no-transmit-frame"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        if let Err(error) = FskId::new(self.station_callsign.trim()) {
            return Some(self.i18n.text_with(
                "error-invalid-station-call",
                &[("error", error.to_string().into())],
            ));
        }
        None
    }

    pub fn start_transmit(&mut self) {
        if let Some(error) = self.transmit_problem() {
            self.tx_error = Some(error);
            return;
        }
        let station_id = match FskId::new(self.station_callsign.trim()) {
            Ok(station_id) => station_id,
            Err(error) => {
                self.tx_error = Some(error.to_string());
                return;
            }
        };
        let (playback, writer) = match self.audio.open_playback(PLAYBACK_QUEUE_SAMPLES) {
            Ok(playback) => playback,
            Err(error) => {
                self.tx_error = Some(error);
                return;
            }
        };
        let frame = self
            .prepared_frame
            .clone()
            .expect("transmit availability requires a prepared frame");
        self.tx_snapshot = TxSnapshot {
            phase: TxPhase::Priming,
            ..TxSnapshot::default()
        };
        self.tx_error = None;
        self.playback = Some(playback);
        self.tx_worker = Some(TxWorker::spawn(writer, self.tx_mode, frame, station_id));
        self.playback_started = false;
    }

    pub fn stop_transmit(&mut self) {
        if self.tx_snapshot.phase.is_active() {
            self.stop_transmit_with(TxPhase::Cancelled);
        }
    }

    fn stop_transmit_with(&mut self, phase: TxPhase) {
        self.playback = None;
        self.tx_worker = None;
        self.playback_started = false;
        self.tx_snapshot.phase = phase;
    }

    /// Reports which image row the transmission is currently on.
    ///
    /// The clock is the sample count the device callback has consumed, not the
    /// worker's generated count: the worker runs ahead to keep the queue full,
    /// so only the callback knows what is actually being heard. That position
    /// maps onto a row because the raster scans at a uniform rate.
    pub fn tx_progress(&self) -> TxProgress {
        if self.tx_snapshot.phase == TxPhase::Complete {
            return TxProgress::Complete;
        }
        if !self.tx_snapshot.phase.is_active() {
            return TxProgress::Idle;
        }
        let played = self.playback.as_ref().map_or(0, Playback::played_samples);
        self.tx_snapshot.raster.progress_at(played)
    }

    pub fn output_sample_rate_hz(&self) -> Option<u32> {
        self.playback.as_ref().map(Playback::sample_rate_hz)
    }
}

fn modes(support: fn(Mode) -> Support) -> Vec<Mode> {
    Mode::ALL
        .into_iter()
        .filter(|mode| support(*mode) == Support::Supported)
        .collect()
}

fn template_entries(directory: &Path) -> io::Result<Vec<Entry>> {
    directory_entries(directory, |path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("kdl"))
            .then(|| Entry::new(path.to_owned(), String::new()))
    })
}

fn stock_entries(directory: &Path) -> io::Result<Vec<Entry>> {
    directory_entries(directory, |path| {
        image::image_dimensions(path)
            .ok()
            .map(|(width, height)| Entry::new(path.to_owned(), format!("{width}×{height}")))
    })
}

fn directory_entries(
    directory: &Path,
    load: impl Fn(&Path) -> Option<Entry>,
) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_file()
            && let Some(entry) = load(&path)
        {
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(entries)
}

fn index_of(entries: &[Entry], name: Option<&str>) -> Option<usize> {
    let name = name?;
    entries.iter().position(|entry| entry.name == name)
}

fn selected_name(entries: &[Entry], selected: Option<usize>) -> Option<String> {
    entries.get(selected?).map(|entry| entry.name.clone())
}

fn replace_entries(entries: &mut Vec<Entry>, selected: &mut Option<usize>, next: Vec<Entry>) {
    let selected_path = selected.and_then(|index| entries.get(index).map(|entry| &entry.path));
    let next_selected = selected_path
        .and_then(|path| next.iter().position(|entry| entry.path == *path))
        .or_else(|| (!next.is_empty()).then_some(0));
    *entries = next;
    *selected = next_selected;
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use rstest::rstest;

    use super::*;
    use crate::receive::Snapshot;

    static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let index = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("rssstv-app-library-{}-{index}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// Builds an interface over real directories but no host audio.
    fn disconnected(paths: AppPaths, settings: &Settings) -> App {
        let config = Config::load(paths.config_file());
        let mut app = App::from_parts(
            AudioState::disconnected(),
            paths,
            config,
            settings,
            Box::new(platform::InertPlatform),
        );
        app.saved = app.settings();
        app
    }

    fn library(root: &TestDirectory) -> AppPaths {
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
            root.0.join("state"),
        );
        paths.initialize().unwrap();
        fs::write(paths.templates_dir().join("alpha.kdl"), "").unwrap();
        fs::write(paths.templates_dir().join("beta.kdl"), "").unwrap();
        image::RgbImage::new(7, 5)
            .save(paths.stocks_dir().join("first.png"))
            .unwrap();
        image::RgbImage::new(7, 5)
            .save(paths.stocks_dir().join("second.png"))
            .unwrap();
        paths
    }

    #[test]
    fn only_supported_modes_are_selectable() {
        let app = App::headless();
        assert!(!app.rx_modes.is_empty());
        assert!(!app.tx_modes.is_empty());
        assert!(
            app.rx_modes
                .iter()
                .all(|mode| mode.spec().decode_support() == Support::Supported)
        );
        assert!(
            app.tx_modes
                .iter()
                .all(|mode| mode.spec().encode_support() == Support::Supported)
        );
    }

    fn decoding(rows: usize, total: usize) -> Snapshot {
        Snapshot {
            progress: Progress::Decoding { rows, total },
            ..Snapshot::default()
        }
    }

    /// A platform that records what the interface asked it for.
    #[derive(Clone, Default)]
    struct RecordingPlatform(Rc<RefCell<Vec<Activity>>>);

    impl Platform for RecordingPlatform {
        fn set_activity(&mut self, activity: Activity) {
            self.0.borrow_mut().push(activity);
        }
    }

    /// Sleep has to be held off for the whole of a reception and released
    /// again when it ends, or a long picture is cut short by an idle timer.
    #[test]
    fn a_reception_asks_the_platform_to_stay_awake_until_it_ends() {
        let recorder = RecordingPlatform::default();
        let mut app = App::headless_on(Box::new(recorder.clone()));

        app.audio.set_snapshot(decoding(40, 100));
        app.poll_audio();
        assert_eq!(app.activity(), Activity::Receiving);

        app.audio.set_snapshot(Snapshot::default());
        app.poll_audio();
        assert_eq!(app.activity(), Activity::Idle);
        assert_eq!(
            recorder.0.take(),
            vec![Activity::Receiving, Activity::Idle],
            "each transition should be reported once"
        );
    }

    /// An unchanged activity must not be restated, or the platform is asked
    /// the same thing on every frame.
    #[test]
    fn an_unchanged_activity_is_not_reported_again() {
        let recorder = RecordingPlatform::default();
        let mut app = App::headless_on(Box::new(recorder.clone()));

        app.audio.set_snapshot(decoding(10, 100));
        app.poll_audio();
        app.audio.set_snapshot(decoding(20, 100));
        app.poll_audio();

        assert_eq!(recorder.0.take(), vec![Activity::Receiving]);
    }

    /// A transmission outranks a reception: both cannot hold the device, and
    /// the transmission is the stronger claim on the machine.
    #[rstest]
    #[case(TxPhase::Priming, Activity::Transmitting)]
    #[case(TxPhase::Producing, Activity::Transmitting)]
    #[case(TxPhase::Draining, Activity::Transmitting)]
    #[case(TxPhase::Complete, Activity::Receiving)]
    #[case(TxPhase::Cancelled, Activity::Receiving)]
    fn a_transmission_outranks_a_reception(#[case] phase: TxPhase, #[case] expected: Activity) {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.tx_snapshot = TxSnapshot {
            phase,
            ..TxSnapshot::default()
        };

        app.poll_audio();

        assert_eq!(app.activity(), expected);
    }

    #[test]
    fn switching_tabs_preserves_receive_progress() {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.tab = Tab::Transmit;
        app.tab = Tab::Receive;
        assert_eq!(app.decoded_fraction(), 0.4);
    }

    #[test]
    fn completed_tabs_draw_a_full_raster() {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.tab = Tab::History;
        assert_eq!(app.decoded_fraction(), 1.0);
    }

    #[test]
    fn an_idle_receiver_draws_nothing_as_decoded() {
        assert_eq!(App::headless().decoded_fraction(), 0.0);
    }

    #[test]
    fn selecting_a_mode_replaces_the_raster() {
        let mut app = App::headless();
        app.select_rx_mode(Mode::Robot36);
        assert_eq!(app.rx_mode, Mode::Robot36);
        assert_eq!(
            app.rx_raster.size().width(),
            Mode::Robot36.spec().width() as usize
        );
        assert_eq!(
            app.rx_raster.size().height(),
            Mode::Robot36.spec().height() as usize
        );
    }

    #[rstest]
    #[case(Dsp::Afc, false)]
    #[case(Dsp::Lms, true)]
    #[case(Dsp::Slant, false)]
    fn dsp_toggles_flip_one_flag(#[case] dsp: Dsp, #[case] expected: bool) {
        let mut app = App::headless();
        app.toggle_dsp(dsp);
        assert_eq!(app.dsp.get(dsp), expected);
        if dsp == Dsp::Slant {
            assert_eq!(app.audio.slant(), expected);
        }
    }

    #[test]
    fn slant_is_enabled_by_default_in_the_ui_and_worker_settings() {
        let app = App::headless();
        assert!(app.dsp.slant);
        assert!(app.audio.slant());
    }

    #[test]
    fn callsign_input_is_normalized_and_clearable() {
        let mut app = App::headless();
        app.qso.call = "ja1xyz".to_owned();
        app.normalize_call();
        assert_eq!(app.qso.call, "JA1XYZ");
        app.clear_qso();
        assert!(app.qso.call.is_empty());
    }

    #[test]
    fn locale_switching_replaces_the_bundle() {
        let mut app = App::headless();
        let english = app.i18n.text("tab-receive");
        app.select_locale(Locale::Ja);
        assert_eq!(app.i18n.locale(), Locale::Ja);
        assert_ne!(app.i18n.text("tab-receive"), english);
    }

    #[test]
    fn changed_settings_are_written_and_restored_on_the_next_start() {
        let root = TestDirectory::new();
        let paths = library(&root);

        let mut app = disconnected(paths.clone(), &Settings::default());
        app.refresh_library();
        app.select_locale(Locale::Ja);
        app.select_rx_mode(Mode::Robot36);
        app.toggle_dsp(Dsp::Lms);
        app.auto_mode = false;
        app.auto_history = false;
        app.select_tx_mode(Mode::Martin1);
        app.station_callsign = "JA1ABC".to_owned();
        app.template = Some(1);
        app.stock = Some(1);
        app.persist();
        assert!(app.config_error().is_none());

        let restored = Config::load(paths.config_file()).settings();
        let mut next = disconnected(paths.clone(), &restored);
        next.refresh_library();
        next.restore_selection(&restored);

        assert_eq!(next.i18n.locale(), Locale::Ja);
        assert_eq!(next.rx_mode, Mode::Robot36);
        assert_eq!(next.tx_mode, Mode::Martin1);
        assert!(next.dsp.lms);
        assert!(!next.auto_mode);
        assert!(!next.auto_history);
        assert_eq!(next.station_callsign, "JA1ABC");
        assert_eq!(next.templates[next.template.unwrap()].name, "beta.kdl");
        assert_eq!(next.stocks[next.stock.unwrap()].name, "second.png");
    }

    #[test]
    fn a_stored_selection_that_disappeared_falls_back_to_the_first_entry() {
        let root = TestDirectory::new();
        let paths = library(&root);
        let settings = Settings {
            template: Some("gone.kdl".to_owned()),
            ..Settings::default()
        };

        let mut app = disconnected(paths, &settings);
        app.refresh_library();
        app.restore_selection(&settings);

        assert_eq!(app.templates[app.template.unwrap()].name, "alpha.kdl");
    }

    #[test]
    fn transient_state_is_not_written_to_the_configuration_file() {
        let root = TestDirectory::new();
        let paths = library(&root);

        let mut app = disconnected(paths.clone(), &Settings::default());
        app.refresh_library();
        app.saved = app.settings();
        app.tab = Tab::Transmit;
        app.qso.call = "JA1XYZ".to_owned();
        app.poll_audio();
        app.persist();

        assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), "");
    }

    #[test]
    fn an_unchanged_frame_does_not_rewrite_the_configuration_file() {
        let root = TestDirectory::new();
        let paths = library(&root);

        let mut app = disconnected(paths.clone(), &Settings::default());
        app.refresh_library();
        app.saved = app.settings();
        app.select_locale(Locale::Ja);
        app.persist();
        let written = fs::metadata(paths.config_file())
            .unwrap()
            .modified()
            .unwrap();

        app.persist();
        app.persist();

        assert_eq!(
            fs::metadata(paths.config_file())
                .unwrap()
                .modified()
                .unwrap(),
            written
        );
    }

    #[test]
    fn library_refresh_lists_matching_files_and_preserves_selection() {
        let root = TestDirectory::new();
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
            root.0.join("state"),
        );
        paths.initialize().unwrap();
        fs::write(paths.templates_dir().join("beta.kdl"), "").unwrap();
        fs::write(paths.templates_dir().join("alpha.KDL"), "").unwrap();
        fs::write(paths.templates_dir().join("ignored.txt"), "").unwrap();
        image::RgbImage::new(7, 5)
            .save(paths.stocks_dir().join("valid.png"))
            .unwrap();
        fs::write(paths.stocks_dir().join("broken.png"), "not an image").unwrap();

        let mut app = disconnected(paths.clone(), &Settings::default());
        app.refresh_library();

        assert_eq!(
            app.templates
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha.KDL", "beta.kdl"]
        );
        assert_eq!(app.template, Some(0));
        assert_eq!(app.stocks.len(), 1);
        assert_eq!(app.stocks[0].name, "valid.png");
        assert_eq!(app.stocks[0].geometry, "7×5");

        app.template = Some(1);
        fs::write(paths.templates_dir().join("aardvark.kdl"), "").unwrap();
        app.refresh_templates();

        assert_eq!(
            app.template.map(|index| app.templates[index].name.as_str()),
            Some("beta.kdl")
        );
    }

    #[test]
    fn composed_preview_can_be_frozen_for_transmit() {
        let root = TestDirectory::new();
        let paths = library(&root);
        fs::write(
            paths.templates_dir().join("alpha.kdl"),
            r##"rximage {
    position x=(fw)0 y=(fh)0
    size width=(fw)100 height=(fh)100 fit="stretch"
}
"##,
        )
        .unwrap();
        let settings = Settings {
            station_callsign: "JA1ABC".to_owned(),
            ..Settings::default()
        };
        let mut app = disconnected(paths, &settings);
        app.refresh_library();
        app.request_preview();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app.can_set_for_transmit() {
            app.poll_audio();
            assert!(app.tx_error.is_none(), "{:?}", app.tx_error);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }

        app.set_for_transmit();

        assert!(app.prepared_frame.is_some());
        assert_eq!(
            app.tx_raster.size().width(),
            app.tx_mode.spec().width() as usize
        );
        assert_eq!(
            app.tx_raster.size().height(),
            app.tx_mode.spec().height() as usize
        );
        app.qso.call = "N0CALL".to_owned();
        app.qso_changed();
        assert!(!app.can_set_for_transmit());
        assert!(app.prepared_frame.is_some());
    }
}
