use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use jiff::{Timestamp, Zoned};
use rssstv_audio::{InputDevice, OutputDevice, Playback, PlaybackWriter, StreamFault};
use rssstv_fskid::{FskId, FskNumber};
use rssstv_sstv::{
    image::RgbImage,
    mode::{Mode, Support},
};
use rssstv_template::valid_variable_name;

use rssstv_demodulator::SyncStart;

use crate::{
    error::AppError,
    i18n::{I18n, Locale, owned},
    platform::{self, Activity, Platform},
    storage::{
        bands::{BandDefinition, BandPlan},
        config::{Config, RigSettings, Settings, UI_SCALE_RANGE},
        paths::{AppPaths, Folder},
    },
    ui::raster::{Raster, test_pattern_image},
    worker::{
        Waker,
        audio::AudioState,
        compose::{ComposeRequest, Composer},
        receive::{Frame, RxProgress},
        rig::{Reading, RigSnapshot, RigState, RigWorker, script},
        transmit::{
            Identification, TUNE_FREQUENCY_HZ, TUNE_LIMIT, TxGain, TxPhase, TxProgress, TxSnapshot,
            TxWorker,
        },
    },
};

const PLAYBACK_QUEUE_SAMPLES: usize = 48_000;

/// How often the interface draws while it is showing something that moves.
const LIVE_INTERVAL: Duration = Duration::from_millis(33);

/// How often a composition in progress is looked for.
const COMPOSE_POLL: Duration = Duration::from_millis(100);

/// How often the interface looks at what nothing reports on its own.
///
/// A device that stops is left on the capture stream rather than published,
/// and rig control answers on its own schedule; neither is worth a faster
/// frame than the operator would notice one at.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Tab {
    #[default]
    Receive,
    Transmit,
}

impl Tab {
    pub const ALL: [Self; 2] = [Self::Receive, Self::Transmit];

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Receive => "tab-receive",
            Self::Transmit => "tab-transmit",
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
    pub lms_filter: bool,
    pub live_slant: bool,
}

impl Default for DspFlags {
    fn default() -> Self {
        Self {
            afc: true,
            lms_filter: false,
            live_slant: true,
        }
    }
}

impl DspFlags {
    pub const fn get(self, dsp: Dsp) -> bool {
        match dsp {
            Dsp::Afc => self.afc,
            Dsp::Lms => self.lms_filter,
            Dsp::Slant => self.live_slant,
        }
    }

    const fn toggle(&mut self, dsp: Dsp) {
        match dsp {
            Dsp::Afc => self.afc = !self.afc,
            Dsp::Lms => self.lms_filter = !self.lms_filter,
            Dsp::Slant => self.live_slant = !self.live_slant,
        }
    }
}

/// The report offered before the operator has changed it.
pub const DEFAULT_RSV: &str = "595";
/// The serial number a fresh log starts from.
///
/// Three digits, which is how a serial is given whatever its value, and how
/// the counted FSKID record carries it.
pub const FIRST_QSO_NUMBER: &str = "001";
/// The report the identifier's contest number is read as.
///
/// Only the number travels over the air; the readability and strength in front
/// of it are what MMSSTV assumes of a station that got through at all.
const RECEIVED_REPORT: &str = "595";

#[derive(Clone, Debug)]
pub struct Qso {
    pub call: String,
    /// The report the other station gave, which `${report.received}` reads.
    pub rsv_received: String,
    pub rsv: String,
    /// The serial number being sent.
    ///
    /// Text rather than a count, because the buttons under it are a
    /// convenience rather than the only way it is worked: an exchange that is
    /// not a plain serial is typed here too.
    pub number: String,
}

impl Default for Qso {
    fn default() -> Self {
        Self {
            call: String::new(),
            rsv_received: String::new(),
            rsv: DEFAULT_RSV.to_owned(),
            number: FIRST_QSO_NUMBER.to_owned(),
        }
    }
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
    /// Whether a VIS header may start a reception over.
    pub vis_restart: bool,
    /// Whether a transmission ends with the station identifier.
    pub send_fskid: bool,
    /// Whether the serial number is worked and sent with that identifier.
    ///
    /// One switch for the whole exchange: a station giving out numbers sends
    /// them, and a station that is not has no serial to count either, so the
    /// panel's number field and its buttons follow this too.
    pub contest_mode: bool,
    /// How far along its travel the transmit level fader sits, in `0.0..=1.0`.
    ///
    /// Held here for the interface, and converted into the amplitude a
    /// transmission is scaled by in [`App::tx_gain`], which is what a running
    /// transmission reads.
    pub tx_volume: f32,
    pub auto_history: bool,
    pub history_format: crate::storage::history::HistoryFormat,
    pub qso: Qso,
    pub station: Station,
    /// The operator's own template variables, offered as `${custom.<name>}`.
    pub custom_variables: BTreeMap<String, String>,
    /// Set while the template variable dialog is open.
    pub variables_dialog_open: bool,
    /// The rows that dialog is editing.
    ///
    /// Kept as a list rather than edited in the map directly, because a map
    /// reorders itself the moment a name is typed into and the row under the
    /// cursor would move out from under it.
    pub variables_draft: Vec<(String, String)>,
    pub library: Library,
    /// The library scan whose result has not arrived yet, if one is running.
    library_scan: Option<mpsc::Receiver<LibraryScan>>,
    /// Receptions still being written out, joined before the interface goes.
    history_writers: Vec<thread::JoinHandle<()>>,
    /// A success the status line reports, as opposed to a failure: writing
    /// out the rig script or the band plan says where the file went.
    pub notice: Option<String>,
    pub rx_raster: Raster,
    pub tx_raster: Raster,
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
    /// How the rig is reached and what it is told.
    ///
    /// Only whether to connect is the interface's to change; the address, the
    /// timings, and the commands are edited in the configuration file, because
    /// what a rig needs is a station's own business rather than something a
    /// menu could offer a list of.
    pub rig: RigSettings,
    /// What rig control last reported, read once per frame.
    pub rig_snapshot: RigSnapshot,
    /// Where each band is, and what the script does on it.
    ///
    /// Shared with the worker rather than copied into it, because the plan is
    /// read once and the interface and the script both work from the same one.
    pub bands: Arc<BandPlan>,
    paths: AppPaths,
    config: Config,
    /// The settings as last written to disk.
    ///
    /// An immediate mode interface mutates state in place, so there is no
    /// message to hang a save off. Comparing against this at the end of the
    /// frame catches every change without the interface having to remember to
    /// announce one.
    saved: Settings,
    composition: Composition,
    /// The amplitude a running transmission reads, shared with its worker.
    tx_gain: Arc<TxGain>,
    playback: Option<Playback>,
    tx_worker: Option<TxWorker>,
    playback_started: bool,
    /// When a tune tone stops itself, set for as long as one is being sent.
    ///
    /// The deadline is what says a tone is running rather than a picture: the
    /// two use the same worker and the same stream, and everything else about
    /// them differs only in what the operator is told.
    tune_until: Option<Instant>,
    rig_worker: Option<RigWorker>,
    /// Set while a transmission has asked for the rig to be keyed.
    ///
    /// A transmission that keyed the rig owes it an unkeying however it ends,
    /// so this outlives the reason the transmission stopped.
    rig_keyed: bool,
    /// How many decoded station identifiers have reached the QSO panel.
    ///
    /// The worker republishes every identifier it has decoded on each
    /// snapshot, so the count is what distinguishes a new arrival from the
    /// same list observed again.
    adopted_callsigns: usize,
    /// How many decoded contest numbers have reached the QSO panel, followed
    /// the same way as the identifiers beside them.
    adopted_numbers: usize,
    /// The receive session those counts were taken in.
    ///
    /// A reopened device starts a new worker with empty lists, and a count
    /// carried over from the old session would swallow as many new arrivals
    /// as it had already adopted.
    adopted_session: u64,
    platform: Box<dyn Platform>,
    /// What the platform was last told the application is doing.
    ///
    /// Compared against each frame so a transition is reported once instead of
    /// restated on every frame.
    activity: Activity,
}

/// The composed transmit frame, and everything it was composed from.
///
/// A frame is only true for the moment it was made in: it can print the clock
/// and what the rig is tuned to, so what it was made from is kept beside it
/// and compared against each frame to decide whether to compose again.
/// The lists the operator picks a template and a background from.
///
/// The selections are indices rather than names because the lists are rebuilt
/// from the directories they name, and what is stored between runs is the name
/// the index stood for.
/// What the station says about itself.
///
/// Set once for the operator rather than per contact, which is why it is
/// edited in a dialog of its own instead of in the panel beside the image.
pub struct Station {
    pub callsign: String,
    pub qth: String,
    pub grid: String,
    /// Set while the station dialog is open in front of the interface.
    pub open: bool,
}

pub struct Library {
    pub templates: Vec<Entry>,
    pub template: Option<usize>,
    pub stocks: Vec<Entry>,
    pub stock: Option<usize>,
    /// The last failure reading either directory, shown on the status line.
    pub error: Option<String>,
}

struct Composition {
    composer: Composer,
    generation: u64,
    template_generation: u64,
    stock_generation: u64,
    /// The last reception worth keeping, for `rximage` template layers.
    ///
    /// Held in memory rather than read back from the received folder: the
    /// layer shows what was just received, which is true whether or not the
    /// operator saves receptions at all. It starts as a test pattern so a
    /// template built around a reception composes before the first one
    /// arrives.
    received_image: Arc<RgbImage>,
    /// When that image was adopted, which `${rx.timestamp...}` reads.
    ///
    /// The test pattern counts as adopted at startup, so the variable resolves
    /// from the first launch for the same reason the layer itself shows
    /// something before any reception has arrived.
    received_at: Zoned,
    /// The composed image the transmit tab shows, and the one a transmission
    /// starting now would send.
    frame: Option<Arc<RgbImage>>,
    /// Set when something changed the composition while a transmission was
    /// running, so the change is made once the transmission ends.
    deferred: bool,
    /// Set while the worker has been asked for a frame and has not answered.
    ///
    /// The composer has no way to ask for a repaint of its own, so this is what
    /// keeps the interface looking until the frame it asked for arrives.
    pending: bool,
    /// Whether the composed frame shows the time, and the minute it was
    /// composed in.
    ///
    /// A frame that prints the clock is only true for the minute it was made
    /// in, so the pair together say when it has to be made again.
    shows_clock: bool,
    minute: i64,
    /// Whether the composed frame prints what the rig is tuned to, and what it
    /// was tuned to when it was composed.
    ///
    /// The pair says the same thing about the rig as the two above say about
    /// the clock: a frame showing a frequency is only true while the rig is
    /// still on it.
    shows_frequency: bool,
    reading: Option<Reading>,
}

impl App {
    pub fn new(paths: AppPaths, waker: Waker) -> Self {
        let config = Config::load(paths.config_file());
        let settings = config.settings();
        let audio = AudioState::new(
            settings.input_device.as_deref(),
            settings.output_device.as_deref(),
            settings.dsp.live_slant,
            settings.vis_restart,
            waker,
        );
        let mut app = Self::from_parts(audio, paths, config, &settings, platform::host());
        app.refresh_library();
        app.restore_selection(&settings);
        app.request_composition();
        app.saved = app.settings();
        app
    }

    /// Builds an interface with no host audio and no stored settings, for
    /// tests.
    #[cfg(test)]
    pub(crate) fn headless() -> Self {
        Self::headless_on(Box::new(platform::QuietPlatform))
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
        let (bands, bands_error) = BandPlan::load(paths.config_dir());
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
            vis_restart: settings.vis_restart,
            send_fskid: settings.send_fskid,
            contest_mode: settings.contest_mode,
            tx_volume: settings.tx_volume,
            tx_gain: Arc::new(TxGain::from_travel(settings.tx_volume)),
            auto_history: settings.auto_history,
            history_format: settings.history_format,
            qso: Qso {
                number: settings.qso_number.trim().to_owned(),
                ..Qso::default()
            },
            station: Station {
                callsign: settings.station_callsign.trim().to_ascii_uppercase(),
                qth: settings.station_qth.trim().to_owned(),
                grid: settings.station_grid.trim().to_owned(),
                open: false,
            },
            custom_variables: settings.custom_variables.clone(),
            variables_dialog_open: false,
            variables_draft: Vec::new(),
            library: Library {
                templates: Vec::new(),
                template: None,
                stocks: Vec::new(),
                stock: None,
                error: bands_error,
            },
            library_scan: None,
            history_writers: Vec::new(),
            notice: None,
            rx_raster: Raster::blank(settings.rx_mode),
            tx_raster: Raster::test_pattern(settings.tx_mode),
            tx_snapshot: TxSnapshot::default(),
            tx_error: None,
            device_fault: None,
            ui_scale: settings.ui_scale,
            rig: settings.rig.clone(),
            rig_snapshot: RigSnapshot::default(),
            bands: Arc::new(bands),
            paths,
            config,
            saved: settings.clone(),
            composition: Composition {
                composer: Composer::spawn(),
                generation: 0,
                template_generation: 0,
                stock_generation: 0,
                received_image: Arc::new(test_pattern_image(settings.rx_mode)),
                received_at: Zoned::now(),
                frame: None,
                deferred: false,
                pending: false,
                shows_clock: false,
                minute: current_minute(),
                shows_frequency: false,
                reading: None,
            },
            playback: None,
            tx_worker: None,
            playback_started: false,
            tune_until: None,
            rig_worker: None,
            rig_keyed: false,
            adopted_callsigns: 0,
            adopted_numbers: 0,
            adopted_session: 0,
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
        self.library.template = index_of(&self.library.templates, settings.template.as_deref())
            .or(self.library.template);
        self.library.stock =
            index_of(&self.library.stocks, settings.stock.as_deref()).or(self.library.stock);
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
            station_callsign: self.station.callsign.clone(),
            station_qth: self.station.qth.clone(),
            station_grid: self.station.grid.clone(),
            qso_number: self.qso.number.clone(),
            custom_variables: self.custom_variables.clone(),
            template: selected_name(&self.library.templates, self.library.template),
            stock: selected_name(&self.library.stocks, self.library.stock),
            rx_mode: self.rx_mode,
            tx_mode: self.tx_mode,
            auto_mode: self.auto_mode,
            dsp: self.dsp,
            vis_restart: self.vis_restart,
            send_fskid: self.send_fskid,
            contest_mode: self.contest_mode,
            tx_volume: self.tx_volume,
            auto_history: self.auto_history,
            history_format: self.history_format,
            ui_scale: self.ui_scale,
            rig: self.rig.clone(),
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
            self.composition.frame = None;
            self.request_composition();
        }
    }

    /// Chooses whether a VIS header may start a reception over.
    ///
    /// Pushed to the receive worker at once rather than on the next frame, so
    /// the change applies to the reception in progress.
    pub fn set_vis_restart(&mut self, enabled: bool) {
        self.vis_restart = enabled;
        self.audio.set_vis_restart(enabled);
    }

    pub fn toggle_dsp(&mut self, dsp: Dsp) {
        self.dsp.toggle(dsp);
        if dsp == Dsp::Slant {
            self.audio.set_live_slant(self.dsp.live_slant);
        }
    }

    /// Opens one of the application's directories in the file manager.
    ///
    /// Received images and everything else the application stores live in the
    /// operator's own directories rather than in a session of its own, so
    /// browsing them is the file manager's job and the interface only has to
    /// point at the folder.
    pub fn reveal(&mut self, folder: Folder) {
        let opened = self.platform.open_path(self.paths.folder(folder));
        self.library.error = opened.err().map(|error| error.to_string());
    }

    /// Opens the bundled manual.
    ///
    /// The pages are HTML and go to the browser rather than to a viewer of the
    /// application's own, which would be a worse browser reachable from one
    /// program. A build run from the source tree has no manual beside it, so
    /// the interface says so instead of opening nothing.
    pub fn open_manual(&mut self) {
        let Some(manual) = manual_path() else {
            self.library.error = Some(self.i18n.text("error-manual-missing"));
            return;
        };
        let opened = self.platform.open_path(&manual);
        self.library.error = opened.err().map(|error| error.to_string());
    }

    /// Uppercases the callsign field after an edit.
    pub fn normalize_call(&mut self) {
        if self.qso.call.chars().any(char::is_lowercase) {
            self.qso.call = self.qso.call.to_uppercase();
        }
    }

    /// Takes up the QSO fields once the operator has left one.
    ///
    /// Each of them is a value rather than text — a callsign, a report, a
    /// serial — and none of them means anything with spaces around it, so what
    /// is taken up is the trimmed field. Trimmed on leaving rather than on
    /// every keystroke, because a space deleted from under the cursor is one
    /// the operator is still typing.
    pub fn finish_qso_edit(&mut self) {
        let mut trimmed = trim_in_place(&mut self.qso.call);
        trimmed |= trim_in_place(&mut self.qso.rsv);
        trimmed |= trim_in_place(&mut self.qso.rsv_received);
        trimmed |= trim_in_place(&mut self.qso.number);
        if trimmed {
            self.qso_changed();
        }
    }

    /// Takes up what the station says about itself, the same way.
    pub fn normalize_station(&mut self) {
        let normalized = self.station.callsign.trim().to_ascii_uppercase();
        if normalized != self.station.callsign {
            self.station.callsign = normalized;
        }
        trim_in_place(&mut self.station.qth);
        trim_in_place(&mut self.station.grid);
        self.request_composition();
    }

    pub fn qso_changed(&mut self) {
        self.request_composition();
    }

    /// Counts the serial number on for the next contact.
    ///
    /// Padded back to three digits, and allowed to grow past them: a contest
    /// worked past its thousandth contact is still counting. A number that is
    /// not one is left alone, because there is nothing to count it to.
    pub fn increment_number(&mut self) {
        let Ok(number) = self.qso.number.trim().parse::<u32>() else {
            return;
        };
        self.qso.number = format!("{:03}", number.saturating_add(1));
        self.qso_changed();
    }

    /// Takes the serial number back to the one a contest starts on.
    pub fn reset_number(&mut self) {
        self.qso.number = FIRST_QSO_NUMBER.to_owned();
        self.qso_changed();
    }

    /// Puts the operator's own variables in front of them for editing.
    pub fn open_custom_variables(&mut self) {
        self.variables_draft = self
            .custom_variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.variables_dialog_open = true;
    }

    /// Adds a row for a variable that has not been named yet.
    pub fn add_custom_variable(&mut self) {
        self.variables_draft.push((String::new(), String::new()));
    }

    /// Takes the edited rows as the variables templates may read.
    ///
    /// A row whose name no `${...}` expression could hold is left in the
    /// dialog to be corrected but kept out of the composition, so an unusable
    /// name never becomes a missing-variable error against a template that
    /// never asked for it.
    pub fn commit_custom_variables(&mut self) {
        let variables: BTreeMap<_, _> = self
            .variables_draft
            .iter()
            .filter(|(name, _)| valid_variable_name(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if variables != self.custom_variables {
            self.custom_variables = variables;
            self.request_composition();
        }
    }

    fn refresh_library(&mut self) {
        let mut errors = Vec::new();
        if let Err(error) = self.load_templates() {
            errors.push(error.to_string());
        }
        if let Err(error) = self.load_stocks() {
            errors.push(error.to_string());
        }
        self.library.error = (!errors.is_empty()).then(|| errors.join("; "));
    }

    /// Reads the library again without making the interface wait for it.
    ///
    /// A stock folder is read file by file for each image's geometry, which on
    /// a networked folder takes longer than a frame has. The scan runs on a
    /// thread of its own and is adopted on the frame its result arrives.
    fn start_library_scan(&mut self) {
        let (sender, receiver) = mpsc::channel();
        let templates_dir = self.paths.templates_dir().to_path_buf();
        let stocks_dir = self.paths.stocks_dir().to_path_buf();
        let spawned = thread::Builder::new()
            .name("rssstv-library".to_owned())
            .spawn(move || {
                let _ = sender.send(LibraryScan {
                    templates: template_entries(&templates_dir),
                    stocks: stock_entries(&stocks_dir),
                });
            });
        match spawned {
            Ok(_) => self.library_scan = Some(receiver),
            Err(_) => {
                let scan = LibraryScan {
                    templates: template_entries(self.paths.templates_dir()),
                    stocks: stock_entries(self.paths.stocks_dir()),
                };
                self.adopt_library_scan(scan);
            }
        }
    }

    fn poll_library_scan(&mut self) {
        let Some(receiver) = self.library_scan.as_ref() else {
            return;
        };
        match receiver.try_recv() {
            Ok(scan) => {
                self.library_scan = None;
                self.adopt_library_scan(scan);
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => self.library_scan = None,
        }
    }

    fn adopt_library_scan(&mut self, scan: LibraryScan) {
        let mut errors = Vec::new();
        match scan.templates {
            Ok(entries) => replace_entries(
                &mut self.library.templates,
                &mut self.library.template,
                entries,
            ),
            Err(error) => errors.push(error.to_string()),
        }
        match scan.stocks {
            Ok(entries) => {
                replace_entries(&mut self.library.stocks, &mut self.library.stock, entries);
            }
            Err(error) => errors.push(error.to_string()),
        }
        self.library.error = (!errors.is_empty()).then(|| errors.join("; "));
        self.composition.template_generation = self.composition.template_generation.wrapping_add(1);
        self.composition.stock_generation = self.composition.stock_generation.wrapping_add(1);
        self.request_composition();
    }

    /// Writes a completed reception out without making the interface wait.
    ///
    /// Encoding a lossless frame and writing it to disk takes longer than a
    /// frame has, and it would land exactly when the operator is looking at
    /// the finished picture. The writer is joined when the interface goes
    /// down, so quitting right after a reception still keeps it.
    fn save_history_in_background(&mut self, candidate: crate::worker::receive::HistoryCandidate) {
        let directory = self.paths.received_dir().to_path_buf();
        let format = self.history_format;
        let spawned = thread::Builder::new()
            .name("rssstv-history".to_owned())
            .spawn(move || {
                if let Err(error) = crate::storage::history::save(&directory, candidate, format) {
                    crate::storage::log::note(&format!("failed to save receive history: {error}"));
                }
            });
        match spawned {
            Ok(writer) => self.history_writers.push(writer),
            Err(_) => crate::storage::log::note("failed to start the receive history writer"),
        }
    }

    /// Waits for every reception on its way to disk, for tests that read it.
    #[cfg(test)]
    fn wait_for_history_writers(&mut self) {
        for writer in self.history_writers.drain(..) {
            let _ = writer.join();
        }
    }

    /// Blocks on a scan already started, for tests that assert on its result.
    #[cfg(test)]
    fn wait_for_library_scan(&mut self) {
        if let Some(receiver) = self.library_scan.take()
            && let Ok(scan) = receiver.recv()
        {
            self.adopt_library_scan(scan);
        }
    }

    pub fn refresh_templates(&mut self) {
        self.start_library_scan();
    }

    pub fn refresh_stocks(&mut self) {
        self.start_library_scan();
    }

    fn load_templates(&mut self) -> io::Result<()> {
        let entries = template_entries(self.paths.templates_dir())?;
        replace_entries(
            &mut self.library.templates,
            &mut self.library.template,
            entries,
        );
        Ok(())
    }

    fn load_stocks(&mut self) -> io::Result<()> {
        let entries = stock_entries(self.paths.stocks_dir())?;
        replace_entries(&mut self.library.stocks, &mut self.library.stock, entries);
        Ok(())
    }

    /// How long the interface may sit still before it has to draw again.
    ///
    /// A reception asks for its own frames through the receive worker's waker,
    /// so what is left here is everything that has no producer to ask on its
    /// behalf: state only the clock changes, a worker read by polling, and a
    /// device whose failure arrives on the stream rather than in a snapshot.
    /// `None` leaves the interface asleep until the operator touches it.
    pub fn repaint_after(&self) -> Option<Duration> {
        let mut soonest = None;
        let mut at_most = |interval: Duration| {
            soonest = Some(soonest.map_or(interval, |current: Duration| current.min(interval)));
        };

        // A transmission is read from its worker and from the playback queue,
        // neither of which announces anything, and the row being sent moves
        // continuously while it runs.
        if self.tx_snapshot.phase.is_active() || self.tune_until.is_some() {
            at_most(LIVE_INTERVAL);
        }
        if self.composition.pending {
            at_most(COMPOSE_POLL);
        }
        // The library scan has no way to ask for a repaint of its own either.
        if self.library_scan.is_some() {
            at_most(COMPOSE_POLL);
        }
        // A composed frame that prints the clock stops being what a
        // transmission should send as the minute turns.
        if self.composition.shows_clock {
            at_most(Duration::from_secs(
                60 - Timestamp::now().as_second().rem_euclid(60) as u64,
            ));
        }
        // Nothing in a snapshot reports a device that stopped, and a rig is
        // read by polling, so both are watched for rather than waited on.
        if self.audio.is_capturing() || self.rig.enabled {
            at_most(WATCH_INTERVAL);
        }
        soonest
    }

    /// Adopts anything any worker produced since the last frame.
    ///
    /// The receive snapshot, the finished history write, the library scan,
    /// the rig, the transmit state machine, and the composition are all read
    /// by polling, and this is the once-per-frame pump that reads them.
    pub fn poll_workers(&mut self) {
        self.poll_device_fault();
        if let Some(frame) = self.audio.poll() {
            self.rx_raster.set_frame(&frame);
        }
        if let Some(candidate) = self.audio.take_history() {
            self.adopt_received_image(&candidate.frame);
            if self.auto_history {
                self.save_history_in_background(candidate);
            }
        }
        self.history_writers.retain(|writer| !writer.is_finished());
        self.poll_library_scan();
        if self.audio.session() != self.adopted_session {
            self.adopted_session = self.audio.session();
            self.adopted_callsigns = 0;
            self.adopted_numbers = 0;
        }
        self.adopt_decoded_callsign();
        self.adopt_decoded_number();
        self.audio.set_sync_start(self.sync_start());
        self.audio.set_vis_restart(self.vis_restart);
        self.tx_gain.set_travel(self.tx_volume);
        // A detected mode only takes over the selection while automatic
        // detection is on; otherwise it would undo the operator's choice.
        if self.auto_mode
            && let Some(mode) = self.audio.snapshot().mode
        {
            self.select_rx_mode(mode);
        }
        self.poll_rig();
        self.poll_transmit();
        // Reception stops for as long as anything is going out. The station's
        // own signal comes back off the antenna, and what would be decoded from
        // it is the picture being sent. Derived from the phase rather than
        // switched at either end of a transmission, so a tone, a picture, and a
        // transmission that failed all release it the same way.
        self.audio
            .set_muted_for_transmit(self.tx_snapshot.phase.is_active());
        self.refresh_timed_composition();
        self.refresh_tuned_composition();
        self.report_activity();
    }

    /// Starts, stops, and reads rig control.
    ///
    /// The switch in the menu is the whole of the decision, so it is acted on
    /// here rather than through a path of its own: a worker exists exactly
    /// while the operator has asked for one.
    fn poll_rig(&mut self) {
        match (self.rig.enabled, self.rig_worker.is_some()) {
            (true, false) => {
                let worker =
                    RigWorker::spawn(&self.rig, self.paths.config_dir(), Arc::clone(&self.bands));
                self.rig_worker = Some(worker);
                self.rig_snapshot = RigSnapshot::default();
            }
            (false, true) => {
                // A rig switched off mid-transmission is still a keyed rig,
                // and the request has to go before the channel closes for the
                // worker to see it at all.
                self.unkey_rig();
                if let Some(worker) = self.rig_worker.take() {
                    worker.stop_in_background();
                }
                self.rig_snapshot = RigSnapshot::default();
            }
            _ => {}
        }
        if let Some(worker) = self.rig_worker.as_ref() {
            self.rig_snapshot = worker.latest();
        }
    }

    /// Switches rig control on or off.
    ///
    /// The whole of the decision: the next frame is what starts or stops the
    /// worker, and switching off mid-transmission is what gives the keying
    /// back before the connection goes.
    pub const fn set_rig_enabled(&mut self, enabled: bool) {
        self.rig.enabled = enabled;
    }

    /// Makes the connection again after it failed.
    ///
    /// Giving up the worker is the whole of it: while rig control is switched
    /// on, the next frame is what starts one.
    pub fn retry_rig(&mut self) {
        self.unkey_rig();
        if let Some(worker) = self.rig_worker.take() {
            worker.stop_in_background();
        }
        self.rig_snapshot = RigSnapshot::default();
    }

    /// Whether the rig may be tuned from the interface.
    ///
    /// Not while transmitting: moving a rig that is on the air moves the
    /// transmission with it.
    pub fn can_tune(&self) -> bool {
        self.rig_snapshot.state == RigState::Receiving && !self.bands.is_empty()
    }

    /// The band the rig is on, as the plan describes it.
    pub fn tuned_band(&self) -> Option<&BandDefinition> {
        let name = self.rig_snapshot.reading.as_ref()?.band.as_deref()?;
        self.bands.by_name(name)
    }

    /// Moves the rig to `name`, handing the script the whole band.
    pub fn change_band(&mut self, name: &str) {
        if !self.can_tune() {
            return;
        }
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.change_band(name);
        }
    }

    /// Where a press of the step buttons would put the rig, if anywhere.
    ///
    /// Stepping out of the band is not tuning, so the edge is where it stops
    /// rather than somewhere the operator is not licensed to be. A band with
    /// no step has nowhere to go, which is what disables the buttons.
    pub fn stepped_frequency(&self, steps: i64) -> Option<u64> {
        let frequency_hz = self.rig_snapshot.reading.as_ref()?.frequency_hz;
        let band = self.tuned_band()?;
        let moved = i64::try_from(frequency_hz).ok()?
            + steps.checked_mul(i64::try_from(band.step_hz()?).ok()?)?;
        let moved = u64::try_from(moved).ok()?;
        band.contains(moved).then_some(moved)
    }

    /// Steps the rig by that many of the band's steps.
    pub fn step_frequency(&mut self, steps: i64) {
        if !self.can_tune() {
            return;
        }
        let Some(target_hz) = self.stepped_frequency(steps) else {
            return;
        };
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.set_frequency(target_hz);
        }
    }

    /// Writes the default script out for the operator to edit.
    ///
    /// Refuses to write over one that is already there: the file only exists
    /// because someone edited it, and replacing it with the default is the one
    /// thing this must never do. The result is reported on the status line
    /// rather than in front of the interface, because nothing was lost either
    /// way.
    pub fn write_rig_script(&mut self) {
        let path = self.paths.config_dir().join(script::SCRIPT_FILE);
        let written = if path.exists() {
            Ok(path)
        } else {
            fs::write(&path, script::DEFAULT_SCRIPT).map(|()| path)
        };
        self.report_written(written);
    }

    /// Writes the built-in band plan out for the operator to edit.
    ///
    /// Refuses to write over one that is already there, for the same reason.
    /// The plan in use is not reloaded: a file being edited is not one to act
    /// on halfway through, so the next start is when it takes effect.
    pub fn write_band_plan(&mut self) {
        let written = BandPlan::write_default(self.paths.config_dir());
        self.report_written(written);
    }

    fn report_written(&mut self, written: io::Result<PathBuf>) {
        match written {
            Ok(path) => {
                self.notice = Some(self.i18n.text_with(
                    "rig-script-written",
                    &[("path", owned(path.display().to_string()))],
                ));
                self.library.error = None;
                self.reveal(Folder::Config);
            }
            Err(error) => {
                self.library.error = Some(error.to_string());
                self.notice = None;
            }
        }
    }

    /// Gives back the keying a transmission asked for, if it asked for any.
    fn unkey_rig(&mut self) {
        if !self.rig_keyed {
            return;
        }
        self.rig_keyed = false;
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.receive();
        }
    }

    /// Whether the audio may start.
    ///
    /// A transmission that keyed the rig waits for the rig to say it is
    /// transmitting, because the lead-in the rig needs to switch over is time
    /// anything sent into it is simply lost.
    const fn rig_ready_to_send(&self) -> bool {
        !self.rig_keyed || matches!(self.rig_snapshot.state, RigState::Transmitting)
    }

    /// Chooses which modes a reception may start in without a VIS header.
    ///
    /// A transmission joined after its header, or that never sent one, can only
    /// be identified by the spacing of its sync pulses, and a period can be
    /// matched by more than one mode. So the operator's own choice decides how
    /// far the inference may reach: with automatic detection on, any supported
    /// mode may be inferred, and with it off only the selected mode is
    /// considered, which confirms rather than overrides that choice.
    const fn sync_start(&self) -> SyncStart {
        if self.auto_mode {
            SyncStart::Any
        } else {
            SyncStart::Only(self.rx_mode)
        }
    }

    /// Puts a newly decoded station identifier in the QSO contact field.
    ///
    /// The identifier names the station on the air right now, so it takes the
    /// field outright: an operator who has just heard a new station wants that
    /// call, not the one left over from the previous contact. Only an arrival
    /// writes, so the field stays editable between receptions.
    fn adopt_decoded_callsign(&mut self) {
        // The count is compared before anything is copied: this runs on every
        // frame, and the list is the same one on almost all of them.
        let decoded = self.audio.snapshot().callsigns.len();
        if decoded == self.adopted_callsigns {
            return;
        }
        self.adopted_callsigns = decoded;
        // The FSKID alphabet has no lowercase, but it does have spaces, and an
        // identifier that is nothing else says nothing about who is calling.
        let Some(call) = self
            .audio
            .snapshot()
            .callsigns
            .last()
            .map(|call| call.trim().to_owned())
            .filter(|call| !call.is_empty())
        else {
            return;
        };
        if call == self.qso.call {
            return;
        }
        self.qso.call = call;
        self.qso_changed();
    }

    /// Puts a newly decoded contest number in the received report field.
    ///
    /// The number is all the identifier carries, so the report it is read as
    /// is filled in around it the way MMSSTV fills it in. It is followed the
    /// same way as the identifier beside it: only an arrival writes, and the
    /// count is followed down as well as up.
    fn adopt_decoded_number(&mut self) {
        let decoded = self.audio.snapshot().numbers.len();
        if decoded == self.adopted_numbers {
            return;
        }
        self.adopted_numbers = decoded;
        let Some(number) = self
            .audio
            .snapshot()
            .numbers
            .last()
            .map(|number| number.trim().to_owned())
            .filter(|number| !number.is_empty())
        else {
            return;
        };
        let report = format!("{RECEIVED_REPORT}{number}");
        if report == self.qso.rsv_received {
            return;
        }
        self.qso.rsv_received = report;
        self.qso_changed();
    }

    /// Keeps a finished reception as the image `rximage` layers show.
    ///
    /// The receive worker only offers a reception that completed, or that was
    /// interrupted with enough of the raster decoded to be worth keeping, so
    /// the layer follows the same rule as the received folder without being
    /// tied to whether anything was written there.
    fn adopt_received_image(&mut self, frame: &Frame) {
        let Some(image) = frame.to_image() else {
            return;
        };
        self.composition.received_image = Arc::new(image);
        self.composition.received_at = Zoned::now();
        self.request_composition();
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
        crate::storage::log::note(&format!("capture stopped: {fault}"));
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
            RxProgress::Acquiring | RxProgress::Decoding { .. }
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
            Tab::Receive => self.audio.snapshot().display_fraction,
            Tab::Transmit => {
                // A tone sends no picture, so the one on the tab is not being
                // drawn out and stays whole while it goes out.
                if self.tx_snapshot.phase.is_active() && !self.is_tuning() {
                    self.tx_progress().fraction()
                } else {
                    1.0
                }
            }
        }
    }

    pub const fn active_mode(&self) -> Mode {
        match self.tab {
            Tab::Transmit => self.tx_mode,
            Tab::Receive => self.rx_mode,
        }
    }

    pub const fn active_raster_mut(&mut self) -> &mut Raster {
        match self.tab {
            Tab::Transmit => &mut self.tx_raster,
            Tab::Receive => &mut self.rx_raster,
        }
    }

    pub fn template_changed(&mut self) {
        self.composition.template_generation = self.composition.template_generation.wrapping_add(1);
        self.request_composition();
    }

    pub fn stock_changed(&mut self) {
        self.composition.stock_generation = self.composition.stock_generation.wrapping_add(1);
        self.request_composition();
    }

    /// Asks the worker for the image the transmit tab should be showing.
    ///
    /// The result replaces the transmit image outright: what is on the tab is
    /// what a transmission would send, so there is nothing to confirm between
    /// choosing a template and keying up.
    fn request_composition(&mut self) {
        // Recorded before anything can turn back, so a composition that is put
        // off is not also asked for again on every frame that follows.
        self.composition.minute = current_minute();
        self.composition.shows_clock = false;
        self.composition.reading = self.rig_snapshot.reading.clone();
        self.composition.shows_frequency = false;
        // A transmission is sending the frame currently on the tab. Replacing
        // it would show something that is not going out, so the change waits
        // until the transmission is over.
        if self.tx_snapshot.phase.is_active() {
            self.composition.deferred = true;
            return;
        }
        let (Some(template), Some(stock)) = (
            self.library
                .template
                .and_then(|index| self.library.templates.get(index)),
            self.library
                .stock
                .and_then(|index| self.library.stocks.get(index)),
        ) else {
            self.composition.frame = None;
            return;
        };
        self.composition.generation = self.composition.generation.wrapping_add(1);
        self.composition.frame = None;
        self.composition.pending = true;
        self.tx_error = None;
        self.composition.composer.request(ComposeRequest {
            generation: self.composition.generation,
            template_generation: self.composition.template_generation,
            stock_generation: self.composition.stock_generation,
            template_path: template.path.clone(),
            background_path: stock.path.clone(),
            assets_dir: self.paths.assets_dir().to_path_buf(),
            mode: self.tx_mode,
            received_image: self.composition.received_image.clone(),
            received_at: self.composition.received_at.clone(),
            station_callsign: self.station.callsign.clone(),
            station_qth: self.station.qth.clone(),
            station_grid: self.station.grid.clone(),
            contact_callsign: self.qso.call.clone(),
            report: self.qso.rsv.clone(),
            number: self.qso.number.clone(),
            report_received: self.qso.rsv_received.clone(),
            custom: self.custom_variables.clone(),
            radio: self.composition.reading.clone(),
        });
    }

    /// Composes again once a frame that shows the time has fallen behind it.
    ///
    /// The finest unit a composition can print is the minute, so the clock
    /// moving on is exactly when the frame on the transmit tab stops being
    /// what a transmission should send.
    fn refresh_timed_composition(&mut self) {
        if self.composition.shows_clock && current_minute() != self.composition.minute {
            self.request_composition();
        }
    }

    /// Composes again once the rig has moved off what a frame printed.
    ///
    /// Only for a frame that prints it: a template that says nothing about the
    /// frequency has no reason to be composed again every time the operator
    /// turns the dial.
    fn refresh_tuned_composition(&mut self) {
        if self.composition.shows_frequency && self.rig_snapshot.reading != self.composition.reading
        {
            self.request_composition();
        }
    }

    fn poll_transmit(&mut self) {
        if let Some(result) = self.composition.composer.latest()
            && result.generation == self.composition.generation
        {
            self.composition.pending = false;
            self.composition.shows_clock = result.uses_timestamps;
            self.composition.shows_frequency = result.uses_radio;
            match result.frame {
                Ok(frame) => {
                    self.tx_raster.set_image(&frame);
                    self.composition.frame = Some(frame);
                    self.tx_error = None;
                }
                Err(error) => {
                    self.composition.frame = None;
                    self.tx_error = Some(error);
                }
            }
        }

        // Checked before the worker is read, because the tone it produces never
        // runs out on its own and the deadline is the only thing that ends it.
        if let Some(deadline) = self.tune_until
            && Instant::now() >= deadline
        {
            self.stop_transmit_with(TxPhase::Complete);
            return;
        }

        let Some(worker) = self.tx_worker.as_ref() else {
            return;
        };
        self.tx_snapshot = worker.latest();
        if self.tx_snapshot.phase == TxPhase::Failed {
            self.tx_error = self.tx_snapshot.error.as_ref().map(AppError::to_string);
            self.stop_transmit_with(TxPhase::Failed);
            return;
        }
        // A rig that refused one of the operator's commands has not been keyed,
        // so the transmission it was keyed for cannot go out.
        if self.rig_keyed
            && let Some(error) = self.rig_snapshot.error.as_ref()
        {
            self.tx_error = Some(error.to_string());
            self.stop_transmit_with(TxPhase::Failed);
            return;
        }
        let should_start = !self.playback_started
            && self.rig_ready_to_send()
            && matches!(
                self.tx_snapshot.phase,
                TxPhase::Producing | TxPhase::Draining
            );
        if should_start {
            let result = self
                .playback
                .as_ref()
                .ok_or(AppError::PlaybackClosed)
                .and_then(|playback| Ok(playback.play()?))
                .map_err(|error: AppError| error.to_string());
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
            self.tx_error = Some(AppError::PlaybackUnderrun.to_string());
            self.stop_transmit_with(TxPhase::Failed);
            return;
        }
        if self.tx_snapshot.phase == TxPhase::Draining
            && self.playback.as_ref().is_some_and(Playback::is_complete)
        {
            self.stop_transmit_with(TxPhase::Complete);
        }
    }

    pub fn can_transmit(&self) -> bool {
        self.transmit_problem().is_none() && !self.tx_snapshot.phase.is_active()
    }

    pub fn transmit_problem(&self) -> Option<String> {
        // The tone holds the stream and the rig, so a picture cannot start over
        // one; it is reported ahead of everything else because it is the only
        // problem here the operator ends by pressing the button beside it.
        if self.is_tuning() {
            return Some(self.i18n.text("error-tone-active"));
        }
        // Reported first, and required whether or not the identifier is sent:
        // a transmission is made by a station, and this is the only thing that
        // names it. The rest is about this transmission; this is about being
        // allowed to make one at all.
        if let Err(error) = FskId::new(self.station.callsign.trim()) {
            return Some(self.i18n.text_with(
                "error-invalid-station-call",
                &[("error", owned(error.to_string()))],
            ));
        }
        if self.composition.frame.is_none() {
            return Some(self.i18n.text("error-no-transmit-frame"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        // Reported last, because it is the only problem here the operator can
        // decide not to have: switching rig control off transmits anyway.
        self.rig_problem()
    }

    /// Reports why the rig could not be handed a transmission, if it could not.
    ///
    /// A rig that was asked to key and did not leaves a transmission nobody
    /// hears, so it stops one rather than being let through quietly.
    fn rig_problem(&self) -> Option<String> {
        if !self.rig.enabled || self.rig_snapshot.state.is_ready() {
            return None;
        }
        let detail = self.rig_snapshot.error.as_ref().map_or_else(
            || self.i18n.text(self.rig_snapshot.state.label_key()),
            AppError::to_string,
        );
        Some(
            self.i18n
                .text_with("error-rig-unavailable", &[("error", owned(detail))]),
        )
    }

    /// Returns the identifier a transmission would end with, if any.
    fn station_id(&self) -> Option<Result<FskId, rssstv_fskid::FskIdError>> {
        self.send_fskid
            .then(|| FskId::new(self.station.callsign.trim()))
    }

    /// Returns the contest number that identifier would carry, if any.
    ///
    /// Only in contest mode, and only for a number the record can hold. The
    /// text is filtered and uppercased the way MMSSTV filters it, and a number
    /// that still does not fit is left off rather than refused: the picture is
    /// what the transmission is for.
    fn contest_number(&self) -> Option<FskNumber> {
        if !self.contest_mode {
            return None;
        }
        let text: String = self
            .qso
            .number
            .to_ascii_uppercase()
            .chars()
            .filter(|character| *character >= '0')
            .collect();
        FskNumber::new(&text).ok()
    }

    pub fn start_transmit(&mut self) {
        if let Some(error) = self.transmit_problem() {
            self.tx_error = Some(error);
            return;
        }
        let identification = match self.station_id().transpose() {
            Ok(station_id) => station_id.map(|station_id| Identification {
                station_id,
                number: self.contest_number(),
            }),
            Err(error) => {
                self.tx_error = Some(error.to_string());
                return;
            }
        };
        let frame = self
            .composition
            .frame
            .clone()
            .expect("transmit availability requires a prepared frame");
        let Some(writer) = self.begin_transmission() else {
            return;
        };
        self.tx_worker = Some(TxWorker::spawn(
            writer,
            self.tx_mode,
            frame,
            identification,
            Arc::clone(&self.tx_gain),
        ));
    }

    /// Claims the output stream and the rig for something about to go out.
    ///
    /// Returns the writer the worker fills, or nothing when the stream could
    /// not be opened, in which case the reason is already in front of the
    /// operator.
    fn begin_transmission(&mut self) -> Option<PlaybackWriter> {
        let (playback, writer) = match self.audio.open_playback(PLAYBACK_QUEUE_SAMPLES) {
            Ok(playback) => playback,
            Err(error) => {
                self.tx_error = Some(error.to_string());
                return None;
            }
        };
        self.tx_snapshot = TxSnapshot {
            phase: TxPhase::Priming,
            ..TxSnapshot::default()
        };
        self.tx_error = None;
        self.playback = Some(playback);
        self.playback_started = false;
        self.tx_gain.set_travel(self.tx_volume);
        // Asked for before the audio is generated, so the lead-in the rig
        // needs runs alongside filling the queue rather than after it.
        if let Some(worker) = self.rig_worker.as_ref() {
            worker.transmit();
            self.rig_keyed = true;
        }
        Some(writer)
    }

    /// Reports why the tune tone could not be sent, if it could not.
    ///
    /// Less stands in its way than in a picture's: a tone carries no image and
    /// names no station, so only the output device and the rig are asked.
    pub fn tune_problem(&self) -> Option<String> {
        if self.tx_snapshot.phase.is_active() && !self.is_tuning() {
            return Some(self.i18n.text("error-transmit-active"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        self.rig_problem()
    }

    /// Keys the rig and sends the steady tone a repeater is opened with.
    pub fn start_tune(&mut self) {
        if let Some(error) = self.tune_problem() {
            self.tx_error = Some(error);
            return;
        }
        let Some(writer) = self.begin_transmission() else {
            return;
        };
        self.tune_until = Some(Instant::now() + TUNE_LIMIT);
        self.tx_worker = Some(TxWorker::spawn_tune(
            writer,
            TUNE_FREQUENCY_HZ,
            Arc::clone(&self.tx_gain),
        ));
    }

    pub fn stop_tune(&mut self) {
        if self.is_tuning() {
            self.stop_transmit_with(TxPhase::Cancelled);
        }
    }

    /// Whether a tune tone is what the rig is currently keyed for.
    pub const fn is_tuning(&self) -> bool {
        self.tune_until.is_some()
    }

    /// Puts the interface in the state a running tone leaves it in.
    ///
    /// Tests have no device to send one on, and what is asserted about a tone
    /// is what the operator can do while it runs rather than its audio.
    #[cfg(test)]
    pub(crate) fn tune_for_test(&mut self) {
        self.tune_until = Some(Instant::now() + TUNE_LIMIT);
        self.tx_snapshot.phase = TxPhase::Producing;
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
        self.tune_until = None;
        self.unkey_rig();
        self.tx_snapshot.phase = phase;
        // Whatever was chosen while the transmission was running takes effect
        // now that nothing is being sent.
        if self.composition.deferred {
            self.composition.deferred = false;
            self.request_composition();
        }
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

/// Takes the spaces off a finished field, reporting whether any were there.
///
/// The operator's own template variables are the exception and are never put
/// through this: what they hold is text the operator wrote, and its spaces are
/// theirs to keep.
fn trim_in_place(text: &mut String) -> bool {
    let trimmed = text.trim();
    if trimmed.len() == text.len() {
        return false;
    }
    let trimmed = trimmed.to_owned();
    *text = trimmed;
    true
}

/// Which minute the clock is in, counted from the epoch.
///
/// A composition is compared against this rather than against a wall-clock
/// field, so it follows the same instant in every zone and needs no calendar.
fn current_minute() -> i64 {
    Timestamp::now().as_second().div_euclid(60)
}

/// The manual's first page, where a release archive puts it.
///
/// Found beside the executable rather than under the application's own
/// directories: the manual belongs to the copy that was extracted, so two
/// versions on one machine each answer with their own, and nothing has to
/// install it anywhere. `None` when it is not there, which is every build run
/// from the source tree.
fn manual_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let manual = executable.parent()?.join("help").join("index.html");
    manual.is_file().then_some(manual)
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

impl Drop for App {
    fn drop(&mut self) {
        // A reception may still be on its way to disk, and the interface
        // going down is the last chance to wait for it.
        for writer in self.history_writers.drain(..) {
            let _ = writer.join();
        }
    }
}

/// What a library scan brought back, adopted on the frame it arrives.
struct LibraryScan {
    templates: io::Result<Vec<Entry>>,
    stocks: io::Result<Vec<Entry>>,
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
mod tests;
