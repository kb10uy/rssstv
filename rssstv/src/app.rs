use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use iced::widget::canvas::Cache;
use iced::{Element, Font, Subscription};
use rssstv_audio::InputDevice;
use rssstv_sstv::mode::{Mode, Support};

use crate::audio::AudioState;
use crate::config::{Config, Settings};
use crate::i18n::{I18n, Locale};
use crate::paths::AppPaths;
use crate::raster::Raster;
use crate::view;

#[cfg(target_os = "windows")]
const UI_FONT: Font = Font::with_name("Yu Gothic UI");
#[cfg(target_os = "macos")]
const UI_FONT: Font = Font::with_name("Hiragino Sans");
#[cfg(target_os = "linux")]
const UI_FONT: Font = Font::with_name("Noto Sans CJK JP");
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
const UI_FONT: Font = Font::DEFAULT;

pub fn run(paths: AppPaths) -> iced::Result {
    iced::application(move || App::new(paths.clone()), App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .default_font(UI_FONT)
        .window_size((1280.0, 940.0))
        .run()
}

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

/// Display wrapper so modes can populate a dropdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeChoice(pub Mode);

impl fmt::Display for ModeChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.spec().name())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dsp {
    Afc,
    Lms,
    Slant,
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

#[derive(Clone, Debug)]
pub enum Message {
    TabSelected(Tab),
    LocaleSelected(Locale),
    DeviceSelected(InputDevice),
    Rx(RxMessage),
    Tx(TxMessage),
    Library(LibraryMessage),
    Qso(QsoMessage),
    Tick,
}

impl Message {
    /// Whether handling this message can change a persisted setting.
    ///
    /// Deciding here rather than in each arm keeps the per-frame [`Self::Tick`]
    /// away from the configuration file.
    const fn persists(&self) -> bool {
        match self {
            Self::LocaleSelected(_) | Self::DeviceSelected(_) | Self::Rx(_) | Self::Tx(_) => true,
            Self::Library(message) => !matches!(
                message,
                LibraryMessage::RevealTemplates | LibraryMessage::RevealStocks
            ),
            Self::TabSelected(_) | Self::Qso(_) | Self::Tick => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RxMessage {
    AutoModeToggled(bool),
    ModeSelected(ModeChoice),
    DspToggled(Dsp),
    AutoHistoryToggled(bool),
}

#[derive(Clone, Copy, Debug)]
pub enum TxMessage {
    ModeSelected(ModeChoice),
}

#[derive(Clone, Copy, Debug)]
pub enum LibraryMessage {
    TemplateSelected(usize),
    StockSelected(usize),
    RevealTemplates,
    RevealStocks,
    RefreshTemplates,
    RefreshStocks,
}

#[derive(Clone, Debug)]
pub enum QsoMessage {
    CallChanged(String),
    RsvChanged(String),
    NumberChanged(String),
    Cleared,
}

pub struct App {
    pub tab: Tab,
    pub i18n: I18n,
    pub audio: AudioState,
    pub auto_mode: bool,
    pub rx_mode: ModeChoice,
    pub tx_mode: ModeChoice,
    pub rx_modes: Vec<ModeChoice>,
    pub tx_modes: Vec<ModeChoice>,
    pub dsp: DspFlags,
    pub auto_history: bool,
    pub qso: Qso,
    pub templates: Vec<Entry>,
    pub template: Option<usize>,
    pub stocks: Vec<Entry>,
    pub stock: Option<usize>,
    pub library_error: Option<String>,
    pub rx_raster: Raster,
    pub tx_raster: Raster,
    pub main_cache: Cache,
    pub preview_cache: Cache,
    paths: AppPaths,
    config: Config,
}

impl App {
    pub fn new(paths: AppPaths) -> Self {
        let config = Config::load(paths.config_file());
        let settings = config.settings();
        let audio = AudioState::new(settings.input_device.as_deref(), settings.dsp.slant);
        let mut app = Self::from_parts(audio, paths, config, &settings);
        app.refresh_library();
        app.restore_selection(&settings);
        app
    }

    /// Builds an interface with no host audio and no stored settings, for
    /// tests.
    #[cfg(test)]
    fn headless() -> Self {
        Self::from_parts(
            AudioState::disconnected(),
            AppPaths::from_roots(PathBuf::new(), PathBuf::new(), PathBuf::new()),
            Config::detached(),
            &Settings::default(),
        )
    }

    fn from_parts(audio: AudioState, paths: AppPaths, config: Config, settings: &Settings) -> Self {
        Self {
            tab: Tab::default(),
            i18n: I18n::new(settings.locale),
            audio,
            auto_mode: settings.auto_mode,
            rx_mode: ModeChoice(settings.rx_mode),
            tx_mode: ModeChoice(settings.tx_mode),
            rx_modes: modes(|mode| mode.spec().decode_support()),
            tx_modes: modes(|mode| mode.spec().encode_support()),
            dsp: settings.dsp,
            auto_history: settings.auto_history,
            qso: Qso::default(),
            templates: Vec::new(),
            template: None,
            stocks: Vec::new(),
            stock: None,
            library_error: None,
            rx_raster: Raster::blank(settings.rx_mode),
            tx_raster: Raster::test_pattern(settings.tx_mode),
            main_cache: Cache::new(),
            preview_cache: Cache::new(),
            paths,
            config,
        }
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
            template: selected_name(&self.templates, self.template),
            stock: selected_name(&self.stocks, self.stock),
            rx_mode: self.rx_mode.0,
            tx_mode: self.tx_mode.0,
            auto_mode: self.auto_mode,
            dsp: self.dsp,
            auto_history: self.auto_history,
        }
    }

    pub fn config_error(&self) -> Option<&str> {
        self.config.error()
    }

    fn title(&self) -> String {
        self.i18n.text("app-title")
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::window::frames().map(|_| Message::Tick)
    }

    fn view(&self) -> Element<'_, Message> {
        view::view(self)
    }

    fn update(&mut self, message: Message) {
        // Settings are written as they change rather than on exit, so an
        // interrupted session still keeps what the operator selected.
        let persists = message.persists();
        match message {
            Message::TabSelected(tab) => {
                self.tab = tab;
                self.main_cache.clear();
            }
            Message::LocaleSelected(locale) => {
                if locale != self.i18n.locale() {
                    self.i18n = I18n::new(locale);
                }
            }
            Message::DeviceSelected(device) => self.audio.select(device),
            Message::Rx(message) => self.update_rx(message),
            Message::Tx(message) => self.update_tx(message),
            Message::Library(message) => self.update_library(message),
            Message::Qso(message) => self.update_qso(message),
            Message::Tick => self.tick(),
        }
        if persists {
            let settings = self.settings();
            self.config.store(&settings);
        }
    }

    fn update_rx(&mut self, message: RxMessage) {
        match message {
            RxMessage::AutoModeToggled(enabled) => self.auto_mode = enabled,
            RxMessage::ModeSelected(mode) => {
                self.rx_mode = mode;
                self.rx_raster = Raster::blank(mode.0);
                self.main_cache.clear();
            }
            RxMessage::DspToggled(dsp) => {
                self.dsp.toggle(dsp);
                if dsp == Dsp::Slant {
                    self.audio.set_slant(self.dsp.slant);
                }
            }
            RxMessage::AutoHistoryToggled(enabled) => self.auto_history = enabled,
        }
    }

    fn update_tx(&mut self, message: TxMessage) {
        match message {
            TxMessage::ModeSelected(mode) => {
                self.tx_mode = mode;
                self.tx_raster = Raster::test_pattern(mode.0);
                self.main_cache.clear();
                self.preview_cache.clear();
            }
        }
    }

    fn update_library(&mut self, message: LibraryMessage) {
        match message {
            LibraryMessage::TemplateSelected(index) if index < self.templates.len() => {
                self.template = Some(index);
            }
            LibraryMessage::StockSelected(index) if index < self.stocks.len() => {
                self.stock = Some(index);
            }
            LibraryMessage::RevealTemplates => {
                self.library_error = reveal_directory(self.paths.templates_dir())
                    .err()
                    .map(|error| error.to_string());
            }
            LibraryMessage::RevealStocks => {
                self.library_error = reveal_directory(self.paths.stocks_dir())
                    .err()
                    .map(|error| error.to_string());
            }
            LibraryMessage::RefreshTemplates => self.refresh_templates(),
            LibraryMessage::RefreshStocks => self.refresh_stocks(),
            LibraryMessage::TemplateSelected(_) | LibraryMessage::StockSelected(_) => {}
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

    fn refresh_templates(&mut self) {
        self.library_error = self.load_templates().err().map(|error| error.to_string());
    }

    fn refresh_stocks(&mut self) {
        self.library_error = self.load_stocks().err().map(|error| error.to_string());
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

    fn update_qso(&mut self, message: QsoMessage) {
        match message {
            QsoMessage::CallChanged(value) => self.qso.call = value.to_uppercase(),
            QsoMessage::RsvChanged(value) => self.qso.rsv = value,
            QsoMessage::NumberChanged(value) => self.qso.number = value,
            QsoMessage::Cleared => self.qso = Qso::default(),
        }
    }

    /// Adopts anything the receive worker produced since the last frame.
    ///
    /// The canvas is invalidated only when what it draws actually changed, so
    /// an idle receiver does not retessellate the raster every frame.
    fn tick(&mut self) {
        let previous_fraction = self.decoded_fraction();
        let mut changed = false;
        if let Some(frame) = self.audio.poll() {
            if let Some(raster) = Raster::from_frame(frame) {
                self.rx_raster = raster;
            }
            changed = true;
        }
        // A detected mode only takes over the selection while automatic
        // detection is on; otherwise it would undo the operator's choice.
        if self.auto_mode
            && let Some(mode) = self.audio.snapshot().mode
            && self.rx_mode.0 != mode
        {
            self.rx_mode = ModeChoice(mode);
            changed = true;
        }
        if changed || self.decoded_fraction() != previous_fraction {
            self.main_cache.clear();
        }
    }

    /// Fraction of the active tab's raster that is drawn as decoded.
    pub fn decoded_fraction(&self) -> f32 {
        match self.tab {
            Tab::Receive => self.audio.snapshot().progress.fraction(),
            Tab::Transmit | Tab::History => 1.0,
        }
    }

    pub const fn active_raster(&self) -> &Raster {
        match self.tab {
            Tab::Transmit => &self.tx_raster,
            Tab::Receive | Tab::History => &self.rx_raster,
        }
    }
}

fn modes(support: fn(Mode) -> Support) -> Vec<ModeChoice> {
    Mode::ALL
        .into_iter()
        .filter(|mode| support(*mode) == Support::Supported)
        .map(ModeChoice)
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

fn reveal_directory(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "opening a directory is not supported on this platform",
    ));

    let mut child = command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;

    use super::*;
    use crate::receive::{Progress, Snapshot};

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

    #[test]
    fn only_supported_modes_are_selectable() {
        let app = App::headless();
        assert!(!app.rx_modes.is_empty());
        assert!(!app.tx_modes.is_empty());
        assert!(
            app.rx_modes
                .iter()
                .all(|mode| mode.0.spec().decode_support() == Support::Supported)
        );
        assert!(
            app.tx_modes
                .iter()
                .all(|mode| mode.0.spec().encode_support() == Support::Supported)
        );
    }

    fn decoding(rows: usize, total: usize) -> Snapshot {
        Snapshot {
            progress: Progress::Decoding { rows, total },
            ..Snapshot::default()
        }
    }

    #[test]
    fn switching_tabs_preserves_receive_progress() {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.update(Message::TabSelected(Tab::Transmit));
        app.update(Message::TabSelected(Tab::Receive));
        assert_eq!(app.decoded_fraction(), 0.4);
    }

    #[test]
    fn completed_tabs_draw_a_full_raster() {
        let mut app = App::headless();
        app.audio.set_snapshot(decoding(40, 100));
        app.update(Message::TabSelected(Tab::History));
        assert_eq!(app.decoded_fraction(), 1.0);
    }

    #[test]
    fn an_idle_receiver_draws_nothing_as_decoded() {
        assert_eq!(App::headless().decoded_fraction(), 0.0);
    }

    #[test]
    fn selecting_a_mode_replaces_the_raster() {
        let mut app = App::headless();
        app.update(Message::Rx(RxMessage::ModeSelected(ModeChoice(
            Mode::Robot36,
        ))));
        assert_eq!(app.rx_mode.0, Mode::Robot36);
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
        app.update(Message::Rx(RxMessage::DspToggled(dsp)));
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
        app.update(Message::Qso(QsoMessage::CallChanged("ja1xyz".to_owned())));
        assert_eq!(app.qso.call, "JA1XYZ");
        app.update(Message::Qso(QsoMessage::Cleared));
        assert!(app.qso.call.is_empty());
    }

    #[test]
    fn locale_switching_replaces_the_bundle() {
        let mut app = App::headless();
        let english = app.i18n.text("tab-receive");
        app.update(Message::LocaleSelected(Locale::Ja));
        assert_eq!(app.i18n.locale(), Locale::Ja);
        assert_ne!(app.i18n.text("tab-receive"), english);
    }

    /// Builds an interface over real directories but no host audio.
    fn disconnected(paths: AppPaths, settings: &Settings) -> App {
        let config = Config::load(paths.config_file());
        App::from_parts(AudioState::disconnected(), paths, config, settings)
    }

    fn library(root: &TestDirectory) -> AppPaths {
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
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
    fn changed_settings_are_written_and_restored_on_the_next_start() {
        let root = TestDirectory::new();
        let paths = library(&root);

        let mut app = disconnected(paths.clone(), &Settings::default());
        app.refresh_library();
        app.update(Message::LocaleSelected(Locale::Ja));
        app.update(Message::Rx(RxMessage::ModeSelected(ModeChoice(
            Mode::Robot36,
        ))));
        app.update(Message::Rx(RxMessage::DspToggled(Dsp::Lms)));
        app.update(Message::Rx(RxMessage::AutoModeToggled(false)));
        app.update(Message::Rx(RxMessage::AutoHistoryToggled(false)));
        app.update(Message::Tx(TxMessage::ModeSelected(ModeChoice(
            Mode::Martin1,
        ))));
        app.update(Message::Library(LibraryMessage::TemplateSelected(1)));
        app.update(Message::Library(LibraryMessage::StockSelected(1)));
        assert!(app.config_error().is_none());

        let restored = Config::load(paths.config_file()).settings();
        let mut next = disconnected(paths.clone(), &restored);
        next.refresh_library();
        next.restore_selection(&restored);

        assert_eq!(next.i18n.locale(), Locale::Ja);
        assert_eq!(next.rx_mode.0, Mode::Robot36);
        assert_eq!(next.tx_mode.0, Mode::Martin1);
        assert!(next.dsp.lms);
        assert!(!next.auto_mode);
        assert!(!next.auto_history);
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
        app.update(Message::TabSelected(Tab::Transmit));
        app.update(Message::Qso(QsoMessage::CallChanged("ja1xyz".to_owned())));
        app.update(Message::Tick);

        assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), "");
    }

    #[test]
    fn library_refresh_lists_matching_files_and_preserves_selection() {
        let root = TestDirectory::new();
        let paths = AppPaths::from_roots(
            root.0.join("config"),
            root.0.join("data"),
            root.0.join("pictures"),
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

        app.update_library(LibraryMessage::TemplateSelected(1));
        fs::write(paths.templates_dir().join("aardvark.kdl"), "").unwrap();
        app.refresh_templates();

        assert_eq!(
            app.template.map(|index| app.templates[index].name.as_str()),
            Some("beta.kdl")
        );
    }
}
