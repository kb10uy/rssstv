//! Settings that survive a restart, and the file holding them.
//!
//! The configuration file is edited in place through a format preserving
//! document, so comments and hand-written layout are not lost when the
//! application saves a changed selection.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use rssstv_rig::DEFAULT_ADDRESS;
use rssstv_sstv::mode::Mode;
use rssstv_template::valid_variable_name;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::{
    app::{DspFlags, FIRST_QSO_NUMBER},
    i18n::Locale,
    storage::history::HistoryFormat,
};

pub const DEFAULT_RX_MODE: Mode = Mode::Pd120;
pub const DEFAULT_TX_MODE: Mode = Mode::Scottie2;
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// Transmit level a first run starts at.
///
/// Full scale, because the modulator already produces normalized PCM and the
/// operator's own output mixer is what the level is usually set against.
pub const DEFAULT_TX_VOLUME: f32 = 1.0;
/// How far the interface may be scaled.
///
/// A stored value is clamped to this, so a hand-edited file cannot shrink the
/// interface past the point where the setting could be changed back.
pub const UI_SCALE_RANGE: core::ops::RangeInclusive<f32> = 0.5..=3.0;
/// How often the rig is asked what it is tuned to, in seconds.
pub const DEFAULT_POLL_SECONDS: f32 = 1.0;
/// How long the rig is given to settle between keying and the first sample.
///
/// A rig switching to transmit takes a moment its own audio does not wait for,
/// and anything sent inside it is simply lost. A fifth of a second covers the
/// relays in a station that has them; a rig that switches faster only wastes
/// it.
pub const DEFAULT_LEAD_IN_SECONDS: f32 = 0.2;
/// How long the carrier is held after the last sample has been played.
pub const DEFAULT_TAIL_SECONDS: f32 = 0.05;
/// The longest either side of a transmission may be padded by, in seconds.
pub const KEYING_SECONDS_RANGE: core::ops::RangeInclusive<f32> = 0.0..=5.0;
/// How far apart polls may be asked to be, in seconds.
///
/// Zero is a value rather than a floor: it turns polling off, which is what an
/// operator wants who keys through rig control but reads the frequency off the
/// front panel.
pub const POLL_SECONDS_RANGE: core::ops::RangeInclusive<f32> = 0.0..=60.0;

/// Everything the application restores on the next start.
///
/// Selections that name something outside the configuration file, such as a
/// capture device or a library file, are stored by name: the identifiers
/// behind them are assigned per run and mean nothing to a later one.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub locale: Locale,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub station_callsign: String,
    /// Where the station is operating from, in plain words.
    pub station_qth: String,
    /// The station's Maidenhead grid locator.
    pub station_grid: String,
    /// The serial number the QSO panel is counting from.
    ///
    /// Kept across runs because a contest outlives a session: an operator who
    /// restarts the application in the middle of one is still on the number
    /// they had reached. Stored as text, because the field it comes from holds
    /// whatever exchange the contest asks for and its leading zeros are part
    /// of what is sent.
    pub qso_number: String,
    /// Names the operator defined, read by templates as `${custom.<name>}`.
    ///
    /// Ordered so the file the application writes back stays in the order the
    /// operator reads it in, rather than being shuffled on every save.
    pub custom_variables: BTreeMap<String, String>,
    pub template: Option<String>,
    pub stock: Option<String>,
    pub rx_mode: Mode,
    pub tx_mode: Mode,
    pub auto_mode: bool,
    pub dsp: DspFlags,
    pub vis_restart: bool,
    pub send_fskid: bool,
    /// Whether the QSO panel's serial number is worked and sent.
    pub contest_mode: bool,
    pub tx_volume: f32,
    pub auto_history: bool,
    pub history_format: HistoryFormat,
    pub ui_scale: f32,
    pub rig: RigSettings,
}

/// How the station's rig is reached.
///
/// Only which `rigctld` instances to reach and the keying timing. What is sent
/// over them is the operator's own script, because what a rig wants around a
/// transmission is a property of that rig and of the station around it, and
/// none of that belongs hard-coded in an SSTV application.
#[derive(Clone, Debug, PartialEq)]
pub struct RigSettings {
    /// Whether the application connects at all.
    pub enabled: bool,
    /// The transports to open, under the names the script reaches them by.
    pub ports: BTreeMap<String, PortSettings>,
    /// How often the frequency is read, in seconds; zero never reads it.
    pub poll_seconds: f32,
    /// How long after keying the first sample is sent, in seconds.
    pub lead_in_seconds: f32,
    /// How long after the last sample the rig is unkeyed, in seconds.
    pub tail_seconds: f32,
}

/// The name the default port is offered to the script under.
pub const DEFAULT_PORT_NAME: &str = "rig";

/// One `rigctld` the application connects to.
///
/// There is nothing to choose between here: Hamlib already covers the CI-V and
/// DTR/RTS keying that a second kind of transport would be for, so a port is
/// an address and no more.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortSettings {
    pub address: String,
}

impl Default for RigSettings {
    fn default() -> Self {
        Self {
            // A station with no rigctld running is the state every first run
            // is in, and trying to reach one would only produce an error the
            // operator did not ask for.
            enabled: false,
            ports: BTreeMap::from([(
                DEFAULT_PORT_NAME.to_owned(),
                PortSettings {
                    address: DEFAULT_ADDRESS.to_owned(),
                },
            )]),
            poll_seconds: DEFAULT_POLL_SECONDS,
            lead_in_seconds: DEFAULT_LEAD_IN_SECONDS,
            tail_seconds: DEFAULT_TAIL_SECONDS,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            input_device: None,
            output_device: None,
            station_callsign: String::new(),
            station_qth: String::new(),
            station_grid: String::new(),
            qso_number: FIRST_QSO_NUMBER.to_owned(),
            custom_variables: BTreeMap::new(),
            template: None,
            stock: None,
            rx_mode: DEFAULT_RX_MODE,
            tx_mode: DEFAULT_TX_MODE,
            auto_mode: true,
            dsp: DspFlags::default(),
            vis_restart: true,
            send_fskid: true,
            // A station that is not in a contest has no number to give, and
            // one that is says so once.
            contest_mode: false,
            tx_volume: DEFAULT_TX_VOLUME,
            auto_history: true,
            history_format: HistoryFormat::default(),
            ui_scale: DEFAULT_UI_SCALE,
            rig: RigSettings::default(),
        }
    }
}

/// The configuration file, kept open as a document across saves.
#[derive(Debug)]
pub struct Config {
    path: PathBuf,
    document: DocumentMut,
    /// Set when the file must not be rewritten.
    ///
    /// A file that could not be read back is left alone rather than saved
    /// over, because rewriting it would replace whatever the operator was in
    /// the middle of editing.
    read_only: bool,
    error: Option<String>,
}

impl Config {
    /// Reads `path`, falling back to an empty document.
    ///
    /// Neither a missing nor a malformed file is fatal: the application starts
    /// on defaults and reports why, rather than refusing to run because one
    /// hand-edited value is wrong.
    pub fn load(path: &Path) -> Self {
        let (document, read_only, error) = match fs::read_to_string(path) {
            Ok(source) => match source.parse::<DocumentMut>() {
                Ok(document) => (document, false, None),
                Err(error) => (DocumentMut::new(), true, Some(error.to_string())),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                (DocumentMut::new(), false, None)
            }
            Err(error) => (DocumentMut::new(), true, Some(error.to_string())),
        };
        Self {
            path: path.to_owned(),
            document,
            read_only,
            error,
        }
    }

    /// Builds a configuration backed by no file at all, for tests.
    #[cfg(test)]
    pub fn detached() -> Self {
        Self {
            path: PathBuf::new(),
            document: DocumentMut::new(),
            read_only: true,
            error: None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Reads the stored settings, substituting the default for anything
    /// missing, mistyped, or unrecognized.
    pub fn settings(&self) -> Settings {
        let defaults = Settings::default();
        Settings {
            locale: string(&self.document, None, "language")
                .and_then(Locale::from_tag)
                .unwrap_or(defaults.locale),
            input_device: owned(&self.document, Some("audio"), "input-device"),
            output_device: owned(&self.document, Some("audio"), "output-device"),
            // The callsign used to sit at the top level, before the station
            // had anything else to say about itself. A file written by that
            // version is still read.
            station_callsign: owned(&self.document, Some("station"), "callsign")
                .or_else(|| owned(&self.document, None, "callsign"))
                .unwrap_or_default(),
            station_qth: owned(&self.document, Some("station"), "qth").unwrap_or_default(),
            station_grid: owned(&self.document, Some("station"), "grid").unwrap_or_default(),
            qso_number: owned(&self.document, Some("qso"), "number").unwrap_or(defaults.qso_number),
            custom_variables: custom_variables(&self.document),
            template: owned(&self.document, Some("library"), "template"),
            stock: owned(&self.document, Some("library"), "stock"),
            rx_mode: string(&self.document, Some("receive"), "mode")
                .and_then(mode_by_name)
                .unwrap_or(defaults.rx_mode),
            tx_mode: string(&self.document, Some("transmit"), "mode")
                .and_then(mode_by_name)
                .unwrap_or(defaults.tx_mode),
            auto_mode: boolean(&self.document, Some("receive"), "auto-vis")
                .unwrap_or(defaults.auto_mode),
            dsp: DspFlags {
                afc: boolean(&self.document, Some("receive"), "afc").unwrap_or(defaults.dsp.afc),
                lms: boolean(&self.document, Some("receive"), "lms").unwrap_or(defaults.dsp.lms),
                slant: boolean(&self.document, Some("receive"), "slant")
                    .unwrap_or(defaults.dsp.slant),
            },
            vis_restart: boolean(&self.document, Some("receive"), "vis-restart")
                .unwrap_or(defaults.vis_restart),
            send_fskid: boolean(&self.document, Some("transmit"), "fskid")
                .unwrap_or(defaults.send_fskid),
            contest_mode: boolean(&self.document, Some("transmit"), "contest")
                .unwrap_or(defaults.contest_mode),
            tx_volume: float(&self.document, Some("transmit"), "volume")
                .map(|volume| volume.clamp(0.0, 1.0))
                .unwrap_or(defaults.tx_volume),
            auto_history: boolean(&self.document, Some("receive"), "auto-history")
                .unwrap_or(defaults.auto_history),
            history_format: string(&self.document, Some("receive"), "history-format")
                .and_then(HistoryFormat::from_config)
                .unwrap_or(defaults.history_format),
            ui_scale: float(&self.document, None, "ui-scale")
                .map(|scale| scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end()))
                .unwrap_or(defaults.ui_scale),
            rig: rig_settings(&self.document),
        }
    }

    /// Writes `settings` back, leaving every unrelated key untouched.
    pub fn store(&mut self, settings: &Settings) {
        if self.read_only {
            return;
        }
        let document = &mut self.document;
        set(
            document,
            None,
            "language",
            Some(value(settings.locale.tag())),
        );
        // Rounded on the way out: widening the f32 directly writes the likes
        // of 1.2999999523162842 into a file meant to be readable by hand.
        set(
            document,
            None,
            "ui-scale",
            Some(value(
                (f64::from(settings.ui_scale) * 100.0).round() / 100.0,
            )),
        );
        set(
            document,
            Some("audio"),
            "input-device",
            settings.input_device.as_deref().map(value),
        );
        set(
            document,
            Some("audio"),
            "output-device",
            settings.output_device.as_deref().map(value),
        );
        set(document, None, "callsign", None);
        for (key, text) in [
            ("callsign", &settings.station_callsign),
            ("qth", &settings.station_qth),
            ("grid", &settings.station_grid),
        ] {
            set(
                document,
                Some("station"),
                key,
                (!text.is_empty()).then(|| value(text)),
            );
        }
        set(
            document,
            Some("qso"),
            "number",
            Some(value(settings.qso_number.as_str())),
        );
        store_custom_variables(document, &settings.custom_variables);
        set(
            document,
            Some("library"),
            "template",
            settings.template.as_deref().map(value),
        );
        set(
            document,
            Some("library"),
            "stock",
            settings.stock.as_deref().map(value),
        );
        set(
            document,
            Some("receive"),
            "mode",
            Some(value(settings.rx_mode.spec().name())),
        );
        set(
            document,
            Some("receive"),
            "auto-vis",
            Some(value(settings.auto_mode)),
        );
        set(
            document,
            Some("receive"),
            "afc",
            Some(value(settings.dsp.afc)),
        );
        set(
            document,
            Some("receive"),
            "lms",
            Some(value(settings.dsp.lms)),
        );
        set(
            document,
            Some("receive"),
            "slant",
            Some(value(settings.dsp.slant)),
        );
        set(
            document,
            Some("receive"),
            "vis-restart",
            Some(value(settings.vis_restart)),
        );
        set(
            document,
            Some("transmit"),
            "fskid",
            Some(value(settings.send_fskid)),
        );
        set(
            document,
            Some("transmit"),
            "contest",
            Some(value(settings.contest_mode)),
        );
        set(
            document,
            Some("transmit"),
            "volume",
            Some(value(
                (f64::from(settings.tx_volume) * 100.0).round() / 100.0,
            )),
        );
        set(
            document,
            Some("receive"),
            "auto-history",
            Some(value(settings.auto_history)),
        );
        set(
            document,
            Some("receive"),
            "history-format",
            Some(value(settings.history_format.config_name())),
        );
        set(
            document,
            Some("transmit"),
            "mode",
            Some(value(settings.tx_mode.spec().name())),
        );
        store_rig(document, &settings.rig);
        self.error = self.write().err().map(|error| error.to_string());
    }

    fn write(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, self.document.to_string())
    }
}

/// Reads the operator's own template variables.
///
/// A name no `${...}` expression could ever hold is dropped rather than
/// carried around unreadable, which is the same treatment every other
/// unusable value in the file gets.
fn custom_variables(document: &DocumentMut) -> BTreeMap<String, String> {
    let Some(table) = document.get("variables").and_then(Item::as_table) else {
        return BTreeMap::new();
    };
    table
        .iter()
        .filter(|(name, _)| valid_variable_name(name))
        .filter_map(|(name, item)| Some((name.to_owned(), item.as_str()?.to_owned())))
        .collect()
}

/// Writes the variable table back, leaving the keys that survived in place.
///
/// Keys are assigned rather than the table being rebuilt, so a comment written
/// beside one by hand outlives a save that did not touch it.
fn store_custom_variables(document: &mut DocumentMut, variables: &BTreeMap<String, String>) {
    if variables.is_empty() {
        document.remove("variables");
        return;
    }
    let table = table_mut(document, "variables");
    table.retain(|name, _| variables.contains_key(name));
    for (name, text) in variables {
        table[name.as_str()] = value(text);
    }
}

/// Reads how the rig is reached and what it is told.
fn rig_settings(document: &DocumentMut) -> RigSettings {
    let defaults = RigSettings::default();
    RigSettings {
        enabled: boolean(document, Some("rig"), "enabled").unwrap_or(defaults.enabled),
        ports: rig_ports(document).unwrap_or(defaults.ports),
        poll_seconds: seconds(
            document,
            "poll-interval",
            &POLL_SECONDS_RANGE,
            defaults.poll_seconds,
        ),
        lead_in_seconds: seconds(
            document,
            "lead-in",
            &KEYING_SECONDS_RANGE,
            defaults.lead_in_seconds,
        ),
        tail_seconds: seconds(
            document,
            "tail",
            &KEYING_SECONDS_RANGE,
            defaults.tail_seconds,
        ),
    }
}

/// Reads a rig duration, held within what the application can act on.
fn seconds(
    document: &DocumentMut,
    key: &str,
    range: &core::ops::RangeInclusive<f32>,
    default: f32,
) -> f32 {
    float(document, Some("rig"), key)
        .map(|seconds| seconds.clamp(*range.start(), *range.end()))
        .unwrap_or(default)
}

/// Reads the ports to connect to, or nothing when the file names none.
///
/// A section that is present is taken as written: an entry that is not a
/// section is dropped, and a section that leaves nothing behind leaves the
/// script with no port to reach rather than being quietly given the default
/// one back.
fn rig_ports(document: &DocumentMut) -> Option<BTreeMap<String, PortSettings>> {
    let table = subtable(document, "rig", "ports")?;
    Some(
        table
            .iter()
            .filter_map(|(name, item)| Some((name.to_owned(), port_settings(item.as_table()?))))
            .collect(),
    )
}

fn port_settings(table: &Table) -> PortSettings {
    PortSettings {
        address: table
            .get("address")
            .and_then(Item::as_str)
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .unwrap_or(DEFAULT_ADDRESS)
            .to_owned(),
    }
}

/// Writes the rig section back, ports and all.
///
/// The ports are written even at their defaults, because the configuration
/// file is where they are edited: an operator who has to add the keys before
/// changing them has no way to learn the keys exist.
fn store_rig(document: &mut DocumentMut, rig: &RigSettings) {
    set(document, Some("rig"), "enabled", Some(value(rig.enabled)));
    for (key, seconds) in [
        ("poll-interval", rig.poll_seconds),
        ("lead-in", rig.lead_in_seconds),
        ("tail", rig.tail_seconds),
    ] {
        set(
            document,
            Some("rig"),
            key,
            // Rounded to the millisecond on the way out, so a widened f32 does
            // not write 0.20000000298023224 into a file meant to be read.
            Some(value((f64::from(seconds) * 1_000.0).round() / 1_000.0)),
        );
    }
    // The rig used to be told what to send from this file, under a single
    // address. A script says it now, over ports that are named, so the keys
    // that meant the old arrangement are taken out rather than left looking
    // like settings that still do something.
    set(document, Some("rig"), "address", None);
    if let Some(rig) = document.get_mut("rig").and_then(Item::as_table_mut) {
        rig.remove("commands");
        rig.remove("bands");
    }
    let ports = subtable_mut(document, "rig", "ports");
    ports.retain(|name, _| rig.ports.contains_key(name));
    for (name, port) in &rig.ports {
        // A port used to name the kind of transport it was. There is only one,
        // so the key is taken back out along with the rest of what it meant.
        let entry = child_table_mut(ports, name);
        entry.remove("kind");
        entry["address"] = value(port.address.as_str());
    }
}

/// Resolves a stored mode name, tolerating a hand-edited difference in case.
fn mode_by_name(name: &str) -> Option<Mode> {
    Mode::ALL
        .into_iter()
        .find(|mode| mode.spec().name().eq_ignore_ascii_case(name))
}

fn get<'a>(document: &'a DocumentMut, table: Option<&str>, key: &str) -> Option<&'a Item> {
    match table {
        None => document.get(key),
        Some(name) => document.get(name)?.as_table()?.get(key),
    }
}

fn string<'a>(document: &'a DocumentMut, table: Option<&str>, key: &str) -> Option<&'a str> {
    get(document, table, key)?.as_str()
}

fn owned(document: &DocumentMut, table: Option<&str>, key: &str) -> Option<String> {
    string(document, table, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn boolean(document: &DocumentMut, table: Option<&str>, key: &str) -> Option<bool> {
    get(document, table, key)?.as_bool()
}

/// Reads a number, accepting the integer a hand-edited file may hold.
fn float(document: &DocumentMut, table: Option<&str>, key: &str) -> Option<f32> {
    let item = get(document, table, key)?;
    item.as_float()
        .or_else(|| item.as_integer().map(|value| value as f64))
        .map(|value| value as f32)
}

/// Assigns `item` under `key`, or removes the key when there is no value.
fn set(document: &mut DocumentMut, table: Option<&str>, key: &str, item: Option<Item>) {
    let Some(item) = item else {
        // An absent table stays absent: a setting with nothing to store is not
        // a reason to write out an empty section.
        let target = match table {
            None => Some(document.as_table_mut()),
            Some(name) => document.get_mut(name).and_then(Item::as_table_mut),
        };
        if let Some(target) = target {
            target.remove(key);
        }
        return;
    };
    let target = match table {
        None => document.as_table_mut(),
        Some(name) => table_mut(document, name),
    };
    target[key] = item;
}

fn subtable<'a>(document: &'a DocumentMut, parent: &str, name: &str) -> Option<&'a Table> {
    document.get(parent)?.as_table()?.get(name)?.as_table()
}

/// Returns `parent.name` as a table, replacing anything else stored under it.
fn subtable_mut<'a>(document: &'a mut DocumentMut, parent: &str, name: &str) -> &'a mut Table {
    child_table_mut(table_mut(document, parent), name)
}

/// Returns `name` within `parent` as a table, replacing anything else there.
fn child_table_mut<'a>(parent: &'a mut Table, name: &str) -> &'a mut Table {
    let entry = parent.entry(name).or_insert(Item::Table(Table::new()));
    if !entry.is_table() {
        *entry = Item::Table(Table::new());
    }
    entry.as_table_mut().expect("the entry holds a table")
}

/// Returns `name` as a table, replacing anything else stored under it.
fn table_mut<'a>(document: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    let entry = document.entry(name).or_insert(Item::Table(Table::new()));
    if !entry.is_table() {
        *entry = Item::Table(Table::new());
    }
    entry.as_table_mut().expect("the entry holds a table")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let index = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rssstv-config-{}-{index}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn populated() -> Settings {
        Settings {
            locale: Locale::Ja,
            input_device: Some("Line In (Interface)".to_owned()),
            output_device: Some("Speakers (Interface)".to_owned()),
            station_callsign: "JA1ABC".to_owned(),
            station_qth: "Chiyoda, Tokyo".to_owned(),
            station_grid: "PM95uq".to_owned(),
            qso_number: "042".to_owned(),
            custom_variables: BTreeMap::from([
                ("club".to_owned(), "JARL".to_owned()),
                ("rig".to_owned(), "FT-991A".to_owned()),
            ]),
            template: Some("field-day.kdl".to_owned()),
            stock: Some("antenna.png".to_owned()),
            rx_mode: Mode::Robot36,
            tx_mode: Mode::Martin1,
            auto_mode: false,
            vis_restart: false,
            send_fskid: false,
            contest_mode: true,
            tx_volume: 0.5,
            dsp: DspFlags {
                afc: false,
                lms: true,
                slant: false,
            },
            auto_history: false,
            history_format: HistoryFormat::Jpeg,
            ui_scale: 1.5,
            rig: RigSettings {
                enabled: true,
                ports: BTreeMap::from([
                    (
                        "rig".to_owned(),
                        PortSettings {
                            address: "192.168.0.8:4532".to_owned(),
                        },
                    ),
                    (
                        "amplifier".to_owned(),
                        PortSettings {
                            address: "127.0.0.1:4533".to_owned(),
                        },
                    ),
                ]),
                poll_seconds: 2.5,
                lead_in_seconds: 0.3,
                tail_seconds: 0.1,
            },
        }
    }

    #[test]
    fn a_missing_file_reads_as_the_defaults() {
        let root = TestDirectory::new();
        let config = Config::load(&root.config());
        assert_eq!(config.settings(), Settings::default());
        assert!(config.error().is_none());
    }

    #[test]
    fn stored_settings_survive_a_reload() {
        let root = TestDirectory::new();
        let settings = populated();

        let mut config = Config::load(&root.config());
        config.store(&settings);
        assert!(config.error().is_none());

        assert_eq!(Config::load(&root.config()).settings(), settings);
    }

    #[test]
    fn saving_keeps_comments_and_unrelated_keys() {
        let root = TestDirectory::new();
        fs::write(
            root.config(),
            "# rig settings, edited by hand\nrig-port = \"COM3\"\n",
        )
        .unwrap();

        let mut config = Config::load(&root.config());
        config.store(&Settings::default());

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(stored.contains("# rig settings, edited by hand"));
        assert!(stored.contains("rig-port = \"COM3\""));
        assert!(stored.contains("[receive]"));
    }

    #[test]
    fn a_setting_with_no_value_is_removed_rather_than_emptied() {
        let root = TestDirectory::new();
        let mut config = Config::load(&root.config());
        config.store(&populated());
        config.store(&Settings::default());

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(!stored.contains("input-device"));
        assert!(!stored.contains("template"));
        assert_eq!(Config::load(&root.config()).settings(), Settings::default());
    }

    #[test]
    fn a_malformed_file_reports_the_error_and_is_not_overwritten() {
        let root = TestDirectory::new();
        fs::write(root.config(), "language = \n").unwrap();

        let mut config = Config::load(&root.config());
        assert!(config.error().is_some());
        assert_eq!(config.settings(), Settings::default());

        config.store(&populated());
        assert_eq!(fs::read_to_string(root.config()).unwrap(), "language = \n");
    }

    #[test]
    fn unrecognized_values_fall_back_to_the_defaults() {
        let root = TestDirectory::new();
        fs::write(
            root.config(),
            concat!(
                "language = \"tlh\"\n",
                "[receive]\n",
                "mode = \"Scottie 9\"\n",
                "afc = \"yes\"\n",
                "[library]\n",
                "template = \"   \"\n",
            ),
        )
        .unwrap();

        let config = Config::load(&root.config());
        assert!(config.error().is_none());
        assert_eq!(config.settings(), Settings::default());
    }

    #[rstest]
    #[case("ui-scale = 1.5\n", 1.5)]
    // An integer is what a hand-edited file is likely to hold.
    #[case("ui-scale = 2\n", 2.0)]
    // Out of range values are clamped rather than ignored, so the interface
    // cannot be left too small to reach the setting that fixes it.
    #[case("ui-scale = 0.01\n", 0.5)]
    #[case("ui-scale = 99\n", 3.0)]
    #[case("ui-scale = \"big\"\n", DEFAULT_UI_SCALE)]
    fn the_ui_scale_is_read_within_range(#[case] stored: &str, #[case] expected: f32) {
        let root = TestDirectory::new();
        fs::write(root.config(), stored).unwrap();
        assert_eq!(Config::load(&root.config()).settings().ui_scale, expected);
    }

    #[test]
    fn the_ui_scale_is_written_readably() {
        let root = TestDirectory::new();
        let mut config = Config::load(&root.config());
        config.store(&Settings {
            ui_scale: 1.3,
            ..Settings::default()
        });

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(
            stored.contains("ui-scale = 1.3"),
            "the scale was written as {stored}"
        );
    }

    /// The ports are what the operator edits, so they have to be in the file
    /// before they are changed rather than only after.
    #[test]
    fn the_default_port_is_written_out_under_the_name_the_script_reaches_it_by() {
        let root = TestDirectory::new();
        let mut config = Config::load(&root.config());
        config.store(&Settings::default());

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(stored.contains("[rig.ports.rig]"), "{stored}");
        assert!(stored.contains(r#"address = "127.0.0.1:4532""#), "{stored}");
    }

    /// A station reaching two rigs names them, and the script reaches each by
    /// the name it was written under.
    #[test]
    fn every_named_port_is_read_back() {
        let root = TestDirectory::new();
        fs::write(
            root.config(),
            concat!(
                "[rig.ports.rig]\n",
                "kind = \"rigctld\"\n",
                "address = \"192.168.0.8:4532\"\n",
                "[rig.ports.amplifier]\n",
                "kind = \"rigctld\"\n",
            ),
        )
        .unwrap();

        let ports = Config::load(&root.config()).settings().rig.ports;

        assert_eq!(
            ports.get("rig"),
            Some(&PortSettings {
                address: "192.168.0.8:4532".to_owned()
            })
        );
        // A port that names no address is still a port; it is where rigctld
        // listens unless it was told otherwise.
        assert_eq!(
            ports.get("amplifier"),
            Some(&PortSettings {
                address: DEFAULT_ADDRESS.to_owned()
            })
        );
    }

    /// A section that is present is taken as written, and an entry that is not
    /// a section is not a port. Handing back the default would put the script
    /// on a rig the operator did not ask for.
    #[test]
    fn an_entry_that_is_not_a_section_is_not_a_port() {
        let root = TestDirectory::new();
        fs::write(root.config(), "[rig.ports]\nrig = 3\n").unwrap();

        assert!(Config::load(&root.config()).settings().rig.ports.is_empty());
    }

    /// A port named the kind of transport it was, back when there could have
    /// been more than one. There cannot, so the key goes.
    #[test]
    fn the_transport_kind_is_taken_out_of_a_port() {
        let root = TestDirectory::new();
        fs::write(
            root.config(),
            "[rig.ports.rig]\nkind = \"rigctld\"\naddress = \"192.168.0.8:4532\"\n",
        )
        .unwrap();

        let mut config = Config::load(&root.config());
        let settings = config.settings();
        assert_eq!(
            settings.rig.ports.get("rig"),
            Some(&PortSettings {
                address: "192.168.0.8:4532".to_owned()
            })
        );

        config.store(&settings);
        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(!stored.contains("kind"), "{stored}");
        assert!(stored.contains("192.168.0.8:4532"), "{stored}");
    }

    /// A section that is not a section says nothing about the ports, which is
    /// how every other unusable value in this file is treated.
    #[test]
    fn a_ports_key_that_is_not_a_section_leaves_the_default_port() {
        let root = TestDirectory::new();
        fs::write(root.config(), "[rig]\nports = 3\n").unwrap();

        assert_eq!(
            Config::load(&root.config()).settings().rig.ports,
            RigSettings::default().ports
        );
    }

    /// The rig used to be told what to send from this file. Leaving those keys
    /// behind would leave settings that look like they still do something.
    #[test]
    fn the_keys_of_the_earlier_arrangement_are_taken_out() {
        let root = TestDirectory::new();
        fs::write(
            root.config(),
            concat!(
                "[rig]\n",
                "address = \"192.168.0.8:4532\"\n",
                "[rig.commands]\n",
                "transmit = \"T 1\"\n",
                "[rig.bands]\n",
                "\"40m\" = '\\set_ant 1 0'\n",
            ),
        )
        .unwrap();

        let mut config = Config::load(&root.config());
        config.store(&Settings::default());

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(!stored.contains("[rig.commands]"), "{stored}");
        assert!(!stored.contains("[rig.bands]"), "{stored}");
        // The single address the rig used to be reached at; the ports written
        // in its place carry addresses of their own.
        assert!(!stored.contains("192.168.0.8"), "{stored}");
        assert!(stored.contains("[rig.ports.rig]"), "{stored}");
    }

    #[rstest]
    #[case("poll-interval = 2.5\n", 2.5, DEFAULT_LEAD_IN_SECONDS)]
    #[case("poll-interval = -4\n", 0.0, DEFAULT_LEAD_IN_SECONDS)]
    #[case("poll-interval = 600\n", 60.0, DEFAULT_LEAD_IN_SECONDS)]
    #[case("lead-in = 99\n", DEFAULT_POLL_SECONDS, 5.0)]
    #[case("lead-in = \"soon\"\n", DEFAULT_POLL_SECONDS, DEFAULT_LEAD_IN_SECONDS)]
    fn rig_timings_are_read_within_range(
        #[case] written: &str,
        #[case] expected_poll: f32,
        #[case] expected_lead_in: f32,
    ) {
        let root = TestDirectory::new();
        fs::write(root.config(), format!("[rig]\n{written}")).unwrap();

        let rig = Config::load(&root.config()).settings().rig;

        assert_eq!(rig.poll_seconds, expected_poll);
        assert_eq!(rig.lead_in_seconds, expected_lead_in);
    }

    #[test]
    fn mode_names_are_matched_without_regard_to_case() {
        assert_eq!(mode_by_name("pd120"), Some(Mode::Pd120));
        assert_eq!(mode_by_name("SCOTTIE 2"), Some(Mode::Scottie2));
        assert_eq!(mode_by_name("Scottie2"), None);
    }

    #[test]
    fn a_conflicting_table_name_is_replaced() {
        let root = TestDirectory::new();
        fs::write(root.config(), "receive = 3\n").unwrap();

        let mut config = Config::load(&root.config());
        config.store(&populated());

        assert_eq!(Config::load(&root.config()).settings(), populated());
    }
}
