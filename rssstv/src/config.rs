//! Settings that survive a restart, and the file holding them.
//!
//! The configuration file is edited in place through a format preserving
//! document, so comments and hand-written layout are not lost when the
//! application saves a changed selection.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use rssstv_sstv::mode::Mode;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::{app::DspFlags, i18n::Locale};

pub const DEFAULT_RX_MODE: Mode = Mode::Pd120;
pub const DEFAULT_TX_MODE: Mode = Mode::Scottie2;
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// How far the interface may be scaled.
///
/// A stored value is clamped to this, so a hand-edited file cannot shrink the
/// interface past the point where the setting could be changed back.
pub const UI_SCALE_RANGE: core::ops::RangeInclusive<f32> = 0.5..=3.0;

/// Everything the application restores on the next start.
///
/// Selections that name something outside the configuration file, such as a
/// capture device or a library file, are stored by name: the identifiers
/// behind them are assigned per run and mean nothing to a later one.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub locale: Locale,
    pub input_device: Option<String>,
    pub template: Option<String>,
    pub stock: Option<String>,
    pub rx_mode: Mode,
    pub tx_mode: Mode,
    pub auto_mode: bool,
    pub dsp: DspFlags,
    pub auto_history: bool,
    pub ui_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: Locale::default(),
            input_device: None,
            template: None,
            stock: None,
            rx_mode: DEFAULT_RX_MODE,
            tx_mode: DEFAULT_TX_MODE,
            auto_mode: true,
            dsp: DspFlags::default(),
            auto_history: true,
            ui_scale: DEFAULT_UI_SCALE,
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
            auto_history: boolean(&self.document, Some("receive"), "auto-history")
                .unwrap_or(defaults.auto_history),
            ui_scale: float(&self.document, None, "ui-scale")
                .map(|scale| scale.clamp(*UI_SCALE_RANGE.start(), *UI_SCALE_RANGE.end()))
                .unwrap_or(defaults.ui_scale),
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
            "auto-history",
            Some(value(settings.auto_history)),
        );
        set(
            document,
            Some("transmit"),
            "mode",
            Some(value(settings.tx_mode.spec().name())),
        );
        self.error = self.write().err().map(|error| error.to_string());
    }

    fn write(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.path, self.document.to_string())
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
            template: Some("field-day.kdl".to_owned()),
            stock: Some("antenna.png".to_owned()),
            rx_mode: Mode::Robot36,
            tx_mode: Mode::Martin1,
            auto_mode: false,
            dsp: DspFlags {
                afc: false,
                lms: true,
                slant: false,
            },
            auto_history: false,
            ui_scale: 1.5,
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
            "# rig settings, edited by hand\ncallsign = \"JA1ABC\"\n",
        )
        .unwrap();

        let mut config = Config::load(&root.config());
        config.store(&Settings::default());

        let stored = fs::read_to_string(root.config()).unwrap();
        assert!(stored.contains("# rig settings, edited by hand"));
        assert!(stored.contains("callsign = \"JA1ABC\""));
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
