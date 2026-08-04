use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use jiff::{Timestamp, Zoned};
use rssstv_audio::{InputDevice, OutputDevice, Playback, StreamFault};
use rssstv_fskid::FskId;
use rssstv_sstv::{
    image::RgbImage,
    mode::{Mode, Support},
};
use rssstv_template::valid_variable_name;

use rssstv_demodulator::SyncStart;

use crate::{
    i18n::{I18n, Locale},
    platform::{self, Activity, Platform},
    storage::{
        config::{Config, RigSettings, Settings, UI_SCALE_RANGE},
        paths::{AppPaths, Folder},
    },
    ui::raster::{Raster, test_pattern_image},
    worker::{
        audio::AudioState,
        compose::{ComposeRequest, Composer},
        receive::{Frame, Progress},
        transmit::{TxGain, TxPhase, TxProgress, TxSnapshot, TxWorker},
    },
};

const PLAYBACK_QUEUE_SAMPLES: usize = 48_000;

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
    /// The report the other station gave, which `${report.received}` reads.
    pub rsv_received: String,
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
    /// How far along its travel the transmit level fader sits, in `0.0..=1.0`.
    ///
    /// Held here for the interface, and converted into the amplitude a
    /// transmission is scaled by in [`App::tx_gain`], which is what a running
    /// transmission reads.
    pub tx_volume: f32,
    pub auto_history: bool,
    pub history_format: crate::storage::history::HistoryFormat,
    pub qso: Qso,
    pub station_callsign: String,
    pub station_qth: String,
    pub station_grid: String,
    /// Set while the station dialog is open in front of the interface.
    pub station_open: bool,
    /// The operator's own template variables, offered as `${custom.<name>}`.
    pub custom_variables: BTreeMap<String, String>,
    /// Set while the template variable dialog is open.
    pub custom_open: bool,
    /// The rows that dialog is editing.
    ///
    /// Kept as a list rather than edited in the map directly, because a map
    /// reorders itself the moment a name is typed into and the row under the
    /// cursor would move out from under it.
    pub custom_draft: Vec<(String, String)>,
    pub templates: Vec<Entry>,
    pub template: Option<usize>,
    pub stocks: Vec<Entry>,
    pub stock: Option<usize>,
    pub library_error: Option<String>,
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
    prepared_frame: Option<Arc<RgbImage>>,
    /// Set when something changed the composition while a transmission was
    /// running, so the change is made once the transmission ends.
    composition_deferred: bool,
    /// Whether the composed frame shows the time, and the minute it was
    /// composed in.
    ///
    /// A frame that prints the clock is only true for the minute it was made
    /// in, so the pair together say when it has to be made again.
    composition_timed: bool,
    composition_minute: i64,
    /// The amplitude a running transmission reads, shared with its worker.
    tx_gain: Arc<TxGain>,
    playback: Option<Playback>,
    tx_worker: Option<TxWorker>,
    playback_started: bool,
    /// How many decoded station identifiers have reached the QSO panel.
    ///
    /// The worker republishes every identifier it has decoded on each
    /// snapshot, so the count is what distinguishes a new arrival from the
    /// same list observed again.
    adopted_callsigns: usize,
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
            settings.vis_restart,
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
            tx_volume: settings.tx_volume,
            tx_gain: Arc::new(TxGain::from_travel(settings.tx_volume)),
            auto_history: settings.auto_history,
            history_format: settings.history_format,
            qso: Qso::default(),
            station_callsign: settings.station_callsign.trim().to_ascii_uppercase(),
            station_qth: settings.station_qth.clone(),
            station_grid: settings.station_grid.clone(),
            station_open: false,
            custom_variables: settings.custom_variables.clone(),
            custom_open: false,
            custom_draft: Vec::new(),
            templates: Vec::new(),
            template: None,
            stocks: Vec::new(),
            stock: None,
            library_error: None,
            rx_raster: Raster::blank(settings.rx_mode),
            tx_raster: Raster::test_pattern(settings.tx_mode),
            tx_snapshot: TxSnapshot::default(),
            tx_error: None,
            device_fault: None,
            ui_scale: settings.ui_scale,
            rig: settings.rig.clone(),
            paths,
            config,
            saved: settings.clone(),
            composer: Composer::spawn(),
            compose_generation: 0,
            received_image: Arc::new(test_pattern_image(settings.rx_mode)),
            received_at: Zoned::now(),
            prepared_frame: None,
            composition_deferred: false,
            composition_timed: false,
            composition_minute: current_minute(),
            playback: None,
            tx_worker: None,
            playback_started: false,
            adopted_callsigns: 0,
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
            station_qth: self.station_qth.clone(),
            station_grid: self.station_grid.clone(),
            custom_variables: self.custom_variables.clone(),
            template: selected_name(&self.templates, self.template),
            stock: selected_name(&self.stocks, self.stock),
            rx_mode: self.rx_mode,
            tx_mode: self.tx_mode,
            auto_mode: self.auto_mode,
            dsp: self.dsp,
            vis_restart: self.vis_restart,
            send_fskid: self.send_fskid,
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
            self.prepared_frame = None;
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
            self.audio.set_slant(self.dsp.slant);
        }
    }

    /// Opens one of the application's directories in the file manager.
    ///
    /// Received images and everything else the application stores live in the
    /// operator's own directories rather than in a session of its own, so
    /// browsing them is the file manager's job and the interface only has to
    /// point at the folder.
    pub fn reveal(&mut self, folder: Folder) {
        let opened = self.platform.reveal_directory(self.paths.folder(folder));
        self.library_error = opened.err().map(|error| error.to_string());
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
        self.request_composition();
    }

    pub fn qso_changed(&mut self) {
        self.request_composition();
    }

    pub fn clear_qso(&mut self) {
        self.qso = Qso::default();
        self.request_composition();
    }

    /// Puts the operator's own variables in front of them for editing.
    pub fn open_custom_variables(&mut self) {
        self.custom_draft = self
            .custom_variables
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        self.custom_open = true;
    }

    /// Adds a row for a variable that has not been named yet.
    pub fn add_custom_variable(&mut self) {
        self.custom_draft.push((String::new(), String::new()));
    }

    /// Takes the edited rows as the variables templates may read.
    ///
    /// A row whose name no `${...}` expression could hold is left in the
    /// dialog to be corrected but kept out of the composition, so an unusable
    /// name never becomes a missing-variable error against a template that
    /// never asked for it.
    pub fn commit_custom_variables(&mut self) {
        let variables: BTreeMap<_, _> = self
            .custom_draft
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
        self.library_error = (!errors.is_empty()).then(|| errors.join("; "));
    }

    pub fn refresh_templates(&mut self) {
        self.library_error = self.load_templates().err().map(|error| error.to_string());
        self.request_composition();
    }

    pub fn refresh_stocks(&mut self) {
        self.library_error = self.load_stocks().err().map(|error| error.to_string());
        self.request_composition();
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
        if let Some(candidate) = self.audio.take_history() {
            self.adopt_received_image(&candidate.frame);
            if self.auto_history
                && let Err(error) = crate::storage::history::save(
                    self.paths.received_dir(),
                    candidate,
                    self.history_format,
                )
            {
                crate::storage::log::note(&format!("failed to save receive history: {error}"));
            }
        }
        self.adopt_decoded_callsign();
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
        self.poll_transmit();
        self.refresh_timed_composition();
        self.report_activity();
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
        // Reopening a device restarts the worker with an empty list, so the
        // count is followed down as well as up.
        if decoded < self.adopted_callsigns {
            self.adopted_callsigns = 0;
        }
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
        self.received_image = Arc::new(image);
        self.received_at = Zoned::now();
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
            Tab::Receive => self.audio.snapshot().display_fraction,
            Tab::Transmit => {
                if self.tx_snapshot.phase.is_active() {
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

    /// Composes again after something the transmit image is built from
    /// changed.
    pub fn composition_changed(&mut self) {
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
        self.composition_minute = current_minute();
        self.composition_timed = false;
        // A transmission is sending the frame currently on the tab. Replacing
        // it would show something that is not going out, so the change waits
        // until the transmission is over.
        if self.tx_snapshot.phase.is_active() {
            self.composition_deferred = true;
            return;
        }
        let (Some(template), Some(stock)) = (
            self.template.and_then(|index| self.templates.get(index)),
            self.stock.and_then(|index| self.stocks.get(index)),
        ) else {
            self.prepared_frame = None;
            return;
        };
        self.compose_generation = self.compose_generation.wrapping_add(1);
        self.prepared_frame = None;
        self.tx_error = None;
        self.composer.request(ComposeRequest {
            generation: self.compose_generation,
            template_path: template.path.clone(),
            background_path: stock.path.clone(),
            assets_dir: self.paths.assets_dir().to_path_buf(),
            mode: self.tx_mode,
            received_image: self.received_image.clone(),
            received_at: self.received_at.clone(),
            station_callsign: self.station_callsign.clone(),
            station_qth: self.station_qth.clone(),
            station_grid: self.station_grid.clone(),
            contact_callsign: self.qso.call.clone(),
            report: self.qso.rsv.clone(),
            number: self.qso.number.clone(),
            report_received: self.qso.rsv_received.clone(),
            custom: self.custom_variables.clone(),
        });
    }

    /// Composes again once a frame that shows the time has fallen behind it.
    ///
    /// The finest unit a composition can print is the minute, so the clock
    /// moving on is exactly when the frame on the transmit tab stops being
    /// what a transmission should send.
    fn refresh_timed_composition(&mut self) {
        if self.composition_timed && current_minute() != self.composition_minute {
            self.request_composition();
        }
    }

    fn poll_transmit(&mut self) {
        if let Some(result) = self.composer.latest()
            && result.generation == self.compose_generation
        {
            self.composition_timed = result.uses_timestamps;
            match result.frame {
                Ok(frame) => {
                    self.tx_raster = Raster::from_image(&frame);
                    self.prepared_frame = Some(frame);
                    self.tx_error = None;
                }
                Err(error) => {
                    self.prepared_frame = None;
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

    pub fn can_transmit(&self) -> bool {
        self.transmit_problem().is_none() && !self.tx_snapshot.phase.is_active()
    }

    pub fn transmit_problem(&self) -> Option<String> {
        // Reported first, and required whether or not the identifier is sent:
        // a transmission is made by a station, and this is the only thing that
        // names it. The rest is about this transmission; this is about being
        // allowed to make one at all.
        if let Err(error) = FskId::new(self.station_callsign.trim()) {
            return Some(self.i18n.text_with(
                "error-invalid-station-call",
                &[("error", error.to_string().into())],
            ));
        }
        if self.prepared_frame.is_none() {
            return Some(self.i18n.text("error-no-transmit-frame"));
        }
        if self.audio.output_device.is_none() {
            return Some(self.i18n.text("error-no-output-device"));
        }
        None
    }

    /// Returns the identifier a transmission would end with, if any.
    fn station_id(&self) -> Option<Result<FskId, rssstv_fskid::FskIdError>> {
        self.send_fskid
            .then(|| FskId::new(self.station_callsign.trim()))
    }

    pub fn start_transmit(&mut self) {
        if let Some(error) = self.transmit_problem() {
            self.tx_error = Some(error);
            return;
        }
        let station_id = match self.station_id().transpose() {
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
        self.tx_gain.set_travel(self.tx_volume);
        self.tx_worker = Some(TxWorker::spawn(
            writer,
            self.tx_mode,
            frame,
            station_id,
            Arc::clone(&self.tx_gain),
        ));
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
        // Whatever was chosen while the transmission was running takes effect
        // now that nothing is being sent.
        if self.composition_deferred {
            self.composition_deferred = false;
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

/// Which minute the clock is in, counted from the epoch.
///
/// A composition is compared against this rather than against a wall-clock
/// field, so it follows the same instant in every zone and needs no calendar.
fn current_minute() -> i64 {
    Timestamp::now().as_second().div_euclid(60)
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

    use rssstv_sstv::image::Rgb8;
    use rstest::rstest;

    use super::*;
    use crate::worker::receive::{Frame, HistoryCandidate, Snapshot};

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
            Box::new(platform::QuietPlatform),
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
            display_fraction: Progress::Decoding { rows, total }.fraction(),
            ..Snapshot::default()
        }
    }

    fn identified(calls: &[&str]) -> Snapshot {
        Snapshot {
            callsigns: calls.iter().map(|call| (*call).to_owned()).collect(),
            ..Snapshot::default()
        }
    }

    #[rstest]
    #[case(true, 1)]
    #[case(false, 0)]
    fn automatic_history_follows_its_setting(#[case] enabled: bool, #[case] expected_files: usize) {
        let root = TestDirectory::new();
        let paths = library(&root);
        let received = paths.received_dir().to_owned();
        let settings = Settings {
            auto_history: enabled,
            ..Settings::default()
        };
        let mut app = disconnected(paths, &settings);
        app.audio.set_snapshot(Snapshot {
            history: Some(HistoryCandidate {
                mode: Mode::Robot36,
                frame: Frame {
                    width: 1,
                    height: 1,
                    rgba: vec![10, 20, 30, 255],
                },
                received_at: "2026-08-04T12:34:56+09:00".to_owned(),
                fsk_ids: vec!["JA1ABC".to_owned()],
            }),
            ..Snapshot::default()
        });

        app.poll_audio();

        assert_eq!(fs::read_dir(received).unwrap().count(), expected_files);
    }

    /// The received image belongs to the template, not to the received folder,
    /// so an operator who keeps nothing on disk still transmits over what was
    /// just received.
    #[rstest]
    #[case(true)]
    #[case(false)]
    fn a_kept_reception_becomes_the_received_image(#[case] saving: bool) {
        let root = TestDirectory::new();
        let settings = Settings {
            auto_history: saving,
            ..Settings::default()
        };
        let mut app = disconnected(library(&root), &settings);
        app.audio.set_snapshot(Snapshot {
            history: Some(HistoryCandidate {
                mode: Mode::Robot36,
                frame: Frame {
                    width: 2,
                    height: 1,
                    rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
                },
                received_at: "2026-08-04T12:34:56+09:00".to_owned(),
                fsk_ids: Vec::new(),
            }),
            ..Snapshot::default()
        });

        app.poll_audio();

        assert_eq!(app.received_image.size().width(), 2);
        assert_eq!(app.received_image.size().height(), 1);
        assert_eq!(
            app.received_image.pixels().first(),
            Some(&Rgb8::new(10, 20, 30))
        );
    }

    /// A reception the worker never offered leaves the layer showing what it
    /// showed before, so a lost signal does not blank a prepared transmission.
    #[test]
    fn a_reception_without_a_candidate_leaves_the_received_image_alone() {
        let mut app = App::headless();

        app.audio.set_snapshot(decoding(10, 100));
        app.poll_audio();

        assert_eq!(*app.received_image, test_pattern_image(app.rx_mode));
    }

    #[test]
    fn a_decoded_identifier_fills_the_qso_contact_field() {
        let mut app = App::headless();

        app.audio.set_snapshot(identified(&["JA1ABC"]));
        app.poll_audio();

        assert_eq!(app.qso.call, "JA1ABC");
    }

    /// The worker republishes every identifier it has decoded, so the same
    /// list observed again must not undo an edit made in the meantime.
    #[test]
    fn an_unchanged_identifier_list_leaves_the_contact_field_alone() {
        let mut app = App::headless();

        app.audio.set_snapshot(identified(&["JA1ABC"]));
        app.poll_audio();
        app.qso.call = "JA1XYZ".to_owned();
        app.poll_audio();

        assert_eq!(app.qso.call, "JA1XYZ");
    }

    /// The identifier names whoever is on the air now, so a new arrival takes
    /// the field even when the operator had put something else there.
    #[test]
    fn a_newly_decoded_identifier_replaces_the_contact_field() {
        let mut app = App::headless();

        app.audio.set_snapshot(identified(&["JA1ABC"]));
        app.poll_audio();
        app.qso.call = "TYPED".to_owned();
        app.audio.set_snapshot(identified(&["JA1ABC", "JH1XYZ"]));
        app.poll_audio();

        assert_eq!(app.qso.call, "JH1XYZ");
    }

    /// Reopening a device restarts the worker with an empty list; the next
    /// identifier it decodes is a new arrival even though the count went down.
    #[test]
    fn an_identifier_after_a_restart_is_adopted_again() {
        let mut app = App::headless();

        app.audio.set_snapshot(identified(&["JA1ABC", "JH1XYZ"]));
        app.poll_audio();
        app.audio.set_snapshot(Snapshot::default());
        app.poll_audio();
        app.audio.set_snapshot(identified(&["JA1ABC"]));
        app.poll_audio();

        assert_eq!(app.qso.call, "JA1ABC");
    }

    /// A sync-interval match can only narrow a mode down, never confirm it the
    /// way a header does, so how far it may reach follows the operator's own
    /// choice: it may pick any mode while automatic detection is on, and
    /// otherwise only confirm the mode already selected.
    #[test]
    fn sync_start_scope_follows_automatic_mode_detection() {
        let mut app = App::headless();
        app.auto_mode = true;
        app.poll_audio();
        assert_eq!(app.audio.sync_start(), SyncStart::Any);

        app.auto_mode = false;
        app.select_rx_mode(Mode::Scottie1);
        app.poll_audio();
        assert_eq!(app.audio.sync_start(), SyncStart::Only(Mode::Scottie1));

        app.select_rx_mode(Mode::Pd120);
        app.poll_audio();
        assert_eq!(app.audio.sync_start(), SyncStart::Only(Mode::Pd120));
    }

    /// The FSKID alphabet includes the space, and an identifier that is only
    /// spaces names nobody.
    #[test]
    fn a_blank_identifier_does_not_reach_the_contact_field() {
        let mut app = App::headless();
        app.qso.call = "JA1ABC".to_owned();

        app.audio.set_snapshot(identified(&["   "]));
        app.poll_audio();

        assert_eq!(app.qso.call, "JA1ABC");
    }

    /// A platform that records what the interface asked it for.
    #[derive(Clone, Default)]
    struct RecordingPlatform(Rc<RefCell<Vec<Activity>>>);

    impl Platform for RecordingPlatform {
        fn set_activity(&mut self, activity: Activity) {
            self.0.borrow_mut().push(activity);
        }

        fn reveal_directory(&mut self, _path: &Path) -> io::Result<()> {
            Ok(())
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
    fn an_idle_transmit_tab_draws_a_full_raster() {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.tab = Tab::Transmit;
        assert_eq!(app.decoded_fraction(), 1.0);
    }

    #[test]
    fn an_idle_receiver_draws_nothing_as_decoded() {
        assert_eq!(App::headless().decoded_fraction(), 0.0);
    }

    #[test]
    fn waiting_receiver_keeps_the_retained_frame_fraction() {
        let mut app = App::headless();
        app.audio.set_snapshot(Snapshot {
            display_fraction: 0.65,
            ..Snapshot::default()
        });
        assert_eq!(app.decoded_fraction(), 0.65);
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

    /// A frame that prints the clock is composed again as the minute turns,
    /// and one that does not is left alone however long it sits there.
    #[test]
    fn only_a_composition_that_shows_the_time_is_made_again() {
        let mut app = App::headless();
        let stale = current_minute() - 1;

        app.composition_timed = false;
        app.composition_minute = stale;
        app.refresh_timed_composition();
        assert_eq!(app.composition_minute, stale);

        app.composition_timed = true;
        app.refresh_timed_composition();
        assert_eq!(app.composition_minute, current_minute());
    }

    /// A half-typed name is still in the dialog to be finished, but it is not
    /// offered to a template, which could only fail to read it.
    #[test]
    fn an_unusable_custom_variable_name_is_kept_out_of_the_composition() {
        let mut app = App::headless();
        app.custom_draft = vec![
            ("club".to_owned(), "JARL".to_owned()),
            ("2m rig".to_owned(), "FT-991A".to_owned()),
        ];

        app.commit_custom_variables();

        assert_eq!(
            app.custom_variables,
            BTreeMap::from([("club".to_owned(), "JARL".to_owned())])
        );
        assert_eq!(app.custom_draft.len(), 2);
    }

    #[test]
    fn slant_is_enabled_by_default_in_the_ui_and_worker_settings() {
        let app = App::headless();
        assert!(app.dsp.slant);
        assert!(app.audio.slant());
    }

    /// The setting reaches the receive worker rather than only the menu mark,
    /// or turning it off would leave the reception restarting anyway.
    #[test]
    fn the_vis_restart_setting_reaches_the_receive_worker() {
        let mut app = App::headless();
        assert!(app.vis_restart);
        assert!(app.audio.vis_restart());

        app.set_vis_restart(false);

        assert!(!app.audio.vis_restart());
    }

    /// The setting decides whether the identifier is sent, not whether the
    /// station needs a callsign: a transmission is made by a station either
    /// way, and nothing else names it.
    #[test]
    fn the_identifier_setting_decides_only_whether_the_callsign_is_sent() {
        let mut app = App::headless();
        app.station_callsign = "!".to_owned();
        assert!(app.send_fskid);
        assert!(matches!(app.station_id(), Some(Err(_))));

        app.send_fskid = false;

        // Nothing is sent now, but the station still has to be named: the
        // callsign is reported before anything else a transmission needs.
        assert!(app.station_id().is_none());
        let problem = app.transmit_problem().expect("an unusable callsign");
        assert_ne!(problem, app.i18n.text("error-no-transmit-frame"));

        app.station_callsign = "JA1ABC".to_owned();

        assert_eq!(
            app.transmit_problem(),
            Some(app.i18n.text("error-no-transmit-frame"))
        );
    }

    /// The station dialog writes the same fields the composition reads, and a
    /// callsign typed into it reaches the transmit check uppercased.
    #[test]
    fn station_details_are_kept_and_normalized() {
        let mut app = App::headless();

        app.station_callsign = " ja1abc ".to_owned();
        app.normalize_station_callsign();
        app.station_qth = "Chiyoda, Tokyo".to_owned();
        app.station_grid = "PM95uq".to_owned();

        assert_eq!(app.station_callsign, "JA1ABC");
        let settings = app.settings();
        assert_eq!(settings.station_callsign, "JA1ABC");
        assert_eq!(settings.station_qth, "Chiyoda, Tokyo");
        assert_eq!(settings.station_grid, "PM95uq");
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
        app.history_format = crate::storage::history::HistoryFormat::Jpeg;
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
        assert_eq!(
            next.history_format,
            crate::storage::history::HistoryFormat::Jpeg
        );
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

    /// Polls until the composition worker has delivered the transmit image.
    fn composed(app: &mut App) -> Arc<RgbImage> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.poll_audio();
            assert!(app.tx_error.is_none(), "{:?}", app.tx_error);
            assert!(Instant::now() < deadline, "no composition arrived");
            if let Some(frame) = app.prepared_frame.clone() {
                return frame;
            }
            std::thread::yield_now();
        }
    }

    /// A pixel from the middle of the frame, away from any resampled edge.
    fn center(frame: &RgbImage) -> Rgb8 {
        let size = frame.size();
        frame.row(size.height() / 2).expect("a middle row")[size.width() / 2]
    }

    #[test]
    fn a_composed_image_becomes_the_transmit_image() {
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
        app.request_composition();

        composed(&mut app);

        assert_eq!(
            app.tx_raster.size().width(),
            app.tx_mode.spec().width() as usize
        );
        assert_eq!(
            app.tx_raster.size().height(),
            app.tx_mode.spec().height() as usize
        );
        assert!(app.transmit_problem().is_none() || app.audio.output_device.is_none());
    }

    /// A kept reception reaches the transmit image on its own, so the operator
    /// does not have to touch the library to send what was just received.
    #[test]
    fn a_kept_reception_reaches_the_transmit_image() {
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
            auto_history: false,
            ..Settings::default()
        };
        let mut app = disconnected(paths, &settings);
        app.refresh_library();
        app.audio.set_snapshot(Snapshot {
            history: Some(HistoryCandidate {
                mode: Mode::Robot36,
                frame: Frame {
                    width: 1,
                    height: 1,
                    rgba: vec![200, 10, 20, 255],
                },
                received_at: "2026-08-04T12:34:56+09:00".to_owned(),
                fsk_ids: Vec::new(),
            }),
            ..Snapshot::default()
        });

        app.poll_audio();

        // The stock images the library holds are black, so a composition made
        // of the received color could not have come from the background.
        let frame = composed(&mut app);
        assert_eq!(frame.pixels().first(), Some(&Rgb8::new(200, 10, 20)));
    }

    /// The transmit image is what a transmission is sending, so a stock chosen
    /// while one runs must not replace it until the transmission is over.
    #[test]
    fn a_selection_during_a_transmission_takes_effect_when_it_ends() {
        let root = TestDirectory::new();
        let paths = library(&root);
        image::RgbImage::from_pixel(8, 6, image::Rgb([200, 30, 30]))
            .save(paths.stocks_dir().join("first.png"))
            .unwrap();
        image::RgbImage::from_pixel(8, 6, image::Rgb([30, 200, 30]))
            .save(paths.stocks_dir().join("second.png"))
            .unwrap();
        let mut app = disconnected(paths, &Settings::default());
        app.refresh_library();
        app.request_composition();
        let first = composed(&mut app);
        assert!(center(&first).r > center(&first).g);

        app.tx_snapshot.phase = TxPhase::Producing;
        app.stock = Some(1);
        app.composition_changed();

        app.poll_audio();
        assert_eq!(app.prepared_frame.as_deref(), Some(first.as_ref()));

        app.stop_transmit();

        let second = composed(&mut app);
        assert!(center(&second).g > center(&second).r);
    }
}
