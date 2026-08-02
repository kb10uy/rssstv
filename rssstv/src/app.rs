use std::fmt;
use std::time::Instant;

use iced::widget::canvas::Cache;
use iced::{Element, Subscription};
use rssstv_audio::InputDevice;
use rssstv_sstv::mode::{Mode, Support};

use crate::audio::AudioState;
use crate::i18n::{I18n, Locale};
use crate::raster::Raster;
use crate::view;

const DEFAULT_RX_MODE: Mode = Mode::Pd120;
const DEFAULT_TX_MODE: Mode = Mode::Scottie2;
const SIMULATED_CYCLE_SECONDS: f32 = 12.0;

pub fn run() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
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

#[derive(Clone, Copy, Debug)]
pub struct DspFlags {
    pub afc: bool,
    pub lms: bool,
    pub slant: bool,
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
}

impl Entry {
    fn new(name: &str, geometry: &str) -> Self {
        Self {
            name: name.to_owned(),
            geometry: geometry.to_owned(),
        }
    }
}

/// Simulated raster progress.
///
/// Capture is real, but nothing decodes it yet, so raster progress and
/// synchronization strength stand in for the receive worker described in
/// `docs/gui-design.md`. Neither is protocol behavior.
#[derive(Clone, Copy, Debug)]
pub struct Simulation {
    pub decoded_fraction: f32,
    pub sync_strength: f32,
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
    Tick(Instant),
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
    pub template: usize,
    pub stocks: Vec<Entry>,
    pub stock: usize,
    pub simulation: Simulation,
    pub rx_raster: Raster,
    pub tx_raster: Raster,
    pub main_cache: Cache,
    pub preview_cache: Cache,
    started: Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            tab: Tab::default(),
            i18n: I18n::new(Locale::default()),
            audio: AudioState::new(),
            auto_mode: true,
            rx_mode: ModeChoice(DEFAULT_RX_MODE),
            tx_mode: ModeChoice(DEFAULT_TX_MODE),
            rx_modes: modes(|mode| mode.spec().decode_support()),
            tx_modes: modes(|mode| mode.spec().encode_support()),
            dsp: DspFlags {
                afc: true,
                lms: false,
                slant: false,
            },
            auto_history: true,
            qso: Qso::default(),
            templates: vec![
                Entry::new("cqsstv-640.kdl", "640×496"),
                Entry::new("cqsstv-320.kdl", "320×256"),
                Entry::new("cq-nyp.kdl", "320×256"),
                Entry::new("tocall-595.kdl", "320×256"),
                Entry::new("tocall-newyear.kdl", "320×256"),
                Entry::new("rx-report.kdl", "640×496"),
                Entry::new("plain-callsign.kdl", "320×256"),
            ],
            template: 0,
            stocks: vec![
                Entry::new("202608022202.png", "640×496"),
                Entry::new("202607281940.png", "640×496"),
                Entry::new("garden-bg.png", "320×256"),
                Entry::new("newyear-bg.png", "320×256"),
                Entry::new("shack.png", "640×496"),
            ],
            stock: 0,
            simulation: Simulation {
                decoded_fraction: 0.0,
                sync_strength: 0.94,
            },
            rx_raster: Raster::test_pattern(DEFAULT_RX_MODE),
            tx_raster: Raster::test_pattern(DEFAULT_TX_MODE),
            main_cache: Cache::new(),
            preview_cache: Cache::new(),
            started: Instant::now(),
        }
    }

    fn title(&self) -> String {
        self.i18n.text("app-title")
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::window::frames().map(Message::Tick)
    }

    fn view(&self) -> Element<'_, Message> {
        view::view(self)
    }

    fn update(&mut self, message: Message) {
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
            Message::Tick(now) => self.tick(now),
        }
    }

    fn update_rx(&mut self, message: RxMessage) {
        match message {
            RxMessage::AutoModeToggled(enabled) => self.auto_mode = enabled,
            RxMessage::ModeSelected(mode) => {
                self.rx_mode = mode;
                self.rx_raster = Raster::test_pattern(mode.0);
                self.main_cache.clear();
            }
            RxMessage::DspToggled(dsp) => self.dsp.toggle(dsp),
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
            LibraryMessage::TemplateSelected(index) => self.template = index,
            LibraryMessage::StockSelected(index) => self.stock = index,
        }
    }

    fn update_qso(&mut self, message: QsoMessage) {
        match message {
            QsoMessage::CallChanged(value) => self.qso.call = value.to_uppercase(),
            QsoMessage::RsvChanged(value) => self.qso.rsv = value,
            QsoMessage::NumberChanged(value) => self.qso.number = value,
            QsoMessage::Cleared => self.qso = Qso::default(),
        }
    }

    fn tick(&mut self, now: Instant) {
        self.audio.poll();
        let elapsed = now.duration_since(self.started).as_secs_f32();
        let fraction = (elapsed / SIMULATED_CYCLE_SECONDS).fract();
        if (fraction - self.simulation.decoded_fraction).abs() < f32::EPSILON {
            return;
        }
        self.simulation = Simulation {
            decoded_fraction: fraction,
            sync_strength: (0.92 + 0.06 * (elapsed * 1.7).sin()).clamp(0.0, 1.0),
        };
        self.main_cache.clear();
    }

    /// Fraction of the active tab's raster that is drawn as decoded.
    pub fn decoded_fraction(&self) -> f32 {
        match self.tab {
            Tab::Receive => self.simulation.decoded_fraction,
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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn modes(support: fn(Mode) -> Support) -> Vec<ModeChoice> {
    Mode::ALL
        .into_iter()
        .filter(|mode| support(*mode) == Support::Supported)
        .map(ModeChoice)
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn only_supported_modes_are_selectable() {
        let app = App::new();
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

    #[test]
    fn switching_tabs_preserves_receive_progress() {
        let mut app = App::new();
        app.simulation.decoded_fraction = 0.4;
        app.update(Message::TabSelected(Tab::Transmit));
        app.update(Message::TabSelected(Tab::Receive));
        assert_eq!(app.simulation.decoded_fraction, 0.4);
        assert_eq!(app.decoded_fraction(), 0.4);
    }

    #[test]
    fn completed_tabs_draw_a_full_raster() {
        let mut app = App::new();
        app.simulation.decoded_fraction = 0.4;
        app.update(Message::TabSelected(Tab::History));
        assert_eq!(app.decoded_fraction(), 1.0);
    }

    #[test]
    fn selecting_a_mode_replaces_the_raster() {
        let mut app = App::new();
        app.update(Message::Rx(RxMessage::ModeSelected(ModeChoice(
            Mode::Robot36,
        ))));
        assert_eq!(app.rx_mode.0, Mode::Robot36);
        assert_eq!(app.rx_raster.mode(), Mode::Robot36);
        assert_eq!(
            app.rx_raster.size().width(),
            Mode::Robot36.spec().width() as usize
        );
    }

    #[rstest]
    #[case(Dsp::Afc, false)]
    #[case(Dsp::Lms, true)]
    #[case(Dsp::Slant, true)]
    fn dsp_toggles_flip_one_flag(#[case] dsp: Dsp, #[case] expected: bool) {
        let mut app = App::new();
        app.update(Message::Rx(RxMessage::DspToggled(dsp)));
        assert_eq!(app.dsp.get(dsp), expected);
    }

    #[test]
    fn callsign_input_is_normalized_and_clearable() {
        let mut app = App::new();
        app.update(Message::Qso(QsoMessage::CallChanged("ja1xyz".to_owned())));
        assert_eq!(app.qso.call, "JA1XYZ");
        app.update(Message::Qso(QsoMessage::Cleared));
        assert!(app.qso.call.is_empty());
    }

    #[test]
    fn locale_switching_replaces_the_bundle() {
        let mut app = App::new();
        let english = app.i18n.text("tab-receive");
        app.update(Message::LocaleSelected(Locale::Ja));
        assert_eq!(app.i18n.locale(), Locale::Ja);
        assert_ne!(app.i18n.text("tab-receive"), english);
    }
}
