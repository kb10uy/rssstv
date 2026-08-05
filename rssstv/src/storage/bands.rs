//! The band plan: where each band is, and what the script does on it.
//!
//! A file of its own rather than a section of `config.toml`, because band
//! plans are regional and worth swapping whole, while the configuration file
//! is the station's own settings.
//!
//! Only `name`, `low`, and `high` mean anything here, plus `step` for the
//! interface to move by. Everything else the operator wrote is carried through
//! to `rigcontrol.lua` untouched: what to do on a band is the script's, and a
//! setting this module understood would be one the script could not add to.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use toml_edit::{DocumentMut, Item, Value};

/// The file the operator's own band plan is read from.
pub const BANDS_FILE: &str = "bands.toml";

/// The plan used when no file has been written.
pub const DEFAULT_PLAN: &str = include_str!("../../bands.toml");

/// A setting the operator attached to a band, as the script will read it.
#[derive(Clone, Debug, PartialEq)]
pub enum BandValue {
    Text(String),
    Integer(i64),
    Decimal(f64),
    Flag(bool),
}

impl BandValue {
    fn of(value: &Value) -> Option<Self> {
        match value {
            Value::String(text) => Some(Self::Text(text.value().clone())),
            Value::Integer(number) => Some(Self::Integer(*number.value())),
            Value::Float(number) => Some(Self::Decimal(*number.value())),
            Value::Boolean(flag) => Some(Self::Flag(*flag.value())),
            // An array or an inline table has no shape the script has been
            // promised, so it is dropped rather than guessed at.
            _ => None,
        }
    }

    /// The value as a count of hertz, when it is written as one.
    fn frequency(&self) -> Option<u64> {
        match self {
            Self::Integer(number) => u64::try_from(*number).ok(),
            _ => None,
        }
    }
}

/// One band, as `bands.toml` describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct BandDefinition {
    pub name: String,
    pub low_hz: u64,
    pub high_hz: u64,
    /// Everything else written under the band, keyed as it was written.
    pub settings: BTreeMap<String, BandValue>,
}

impl BandDefinition {
    pub fn contains(&self, frequency_hz: u64) -> bool {
        (self.low_hz..=self.high_hz).contains(&frequency_hz)
    }

    /// How far one press of the step buttons moves, if the band says.
    ///
    /// The one setting the interface reads for itself. A band without it has
    /// nothing to move by, and the buttons are disabled rather than guessing.
    pub fn step_hz(&self) -> Option<u64> {
        self.settings
            .get("step")
            .and_then(BandValue::frequency)
            .filter(|step| *step > 0)
    }
}

/// The bands, in the order the interface offers them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandPlan {
    bands: Vec<BandDefinition>,
}

impl BandPlan {
    /// Reads the plan beside `config.toml`, falling back to the built-in one.
    ///
    /// A malformed file is reported and the built-in plan is used, for the
    /// same reason a malformed `config.toml` does not stop the application:
    /// one hand-edited value is not a reason to refuse to run.
    pub fn load(config_dir: &Path) -> (Self, Option<String>) {
        let path = config_dir.join(BANDS_FILE);
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => DEFAULT_PLAN.to_owned(),
            Err(error) => {
                return (
                    Self::built_in(),
                    Some(format!("{}: {error}", path.display())),
                );
            }
        };
        match Self::parse(&source) {
            Ok(plan) => (plan, None),
            Err(error) => (
                Self::built_in(),
                Some(format!("{}: {error}", path.display())),
            ),
        }
    }

    /// The plan compiled into the application.
    pub fn built_in() -> Self {
        Self::parse(DEFAULT_PLAN).expect("the built-in band plan is well formed")
    }

    pub fn parse(source: &str) -> Result<Self, String> {
        let document: DocumentMut = source
            .parse()
            .map_err(|error: toml_edit::TomlError| error.to_string())?;
        let Some(entries) = document.get("bands").and_then(Item::as_array_of_tables) else {
            return Err("no [[bands]] entries".to_owned());
        };
        let bands = entries
            .iter()
            .filter_map(|table| {
                let name = table.get("name")?.as_str()?.trim();
                let low_hz = frequency(table.get("low")?)?;
                let high_hz = frequency(table.get("high")?)?;
                if name.is_empty() || low_hz > high_hz {
                    return None;
                }
                let settings = table
                    .iter()
                    .filter(|(key, _)| !matches!(*key, "name" | "low" | "high"))
                    .filter_map(|(key, item)| {
                        Some((key.to_owned(), BandValue::of(item.as_value()?)?))
                    })
                    .collect();
                Some(BandDefinition {
                    name: name.to_owned(),
                    low_hz,
                    high_hz,
                    settings,
                })
            })
            .collect();
        Ok(Self { bands })
    }

    pub fn bands(&self) -> &[BandDefinition] {
        &self.bands
    }

    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Returns the band `frequency_hz` falls in, if it falls in one at all.
    ///
    /// A frequency between the bands has no band to report. That is a normal
    /// state rather than a fault: a rig parked on a broadcast station or on a
    /// listening frequency is still something the operator may be doing.
    pub fn for_frequency(&self, frequency_hz: u64) -> Option<&BandDefinition> {
        self.bands.iter().find(|band| band.contains(frequency_hz))
    }

    pub fn by_name(&self, name: &str) -> Option<&BandDefinition> {
        self.bands.iter().find(|band| band.name == name)
    }

    /// Writes the built-in plan out for the operator to edit.
    ///
    /// Refuses to write over one that is already there: the file only exists
    /// because someone wrote it, and replacing it with the default is the one
    /// thing this must never do.
    pub fn write_default(config_dir: &Path) -> io::Result<PathBuf> {
        let path = config_dir.join(BANDS_FILE);
        if path.exists() {
            return Ok(path);
        }
        fs::write(&path, DEFAULT_PLAN)?;
        Ok(path)
    }
}

fn frequency(item: &Item) -> Option<u64> {
    u64::try_from(item.as_integer()?).ok()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn the_built_in_plan_parses_and_is_ordered() {
        let plan = BandPlan::built_in();
        assert!(!plan.is_empty());
        for pair in plan.bands().windows(2) {
            assert!(
                pair[0].high_hz < pair[1].low_hz,
                "{} and {} overlap",
                pair[0].name,
                pair[1].name
            );
        }
    }

    #[rstest]
    #[case(7_171_000, Some("40m"))]
    #[case(7_000_000, Some("40m"))]
    #[case(7_300_000, Some("40m"))]
    #[case(14_230_000, Some("20m"))]
    // Between the bands, which is where a rig tuned to a broadcast sits.
    #[case(6_000_000, None)]
    #[case(0, None)]
    fn a_frequency_names_the_band_it_sits_in(
        #[case] frequency_hz: u64,
        #[case] expected: Option<&str>,
    ) {
        let plan = BandPlan::built_in();
        assert_eq!(
            plan.for_frequency(frequency_hz)
                .map(|band| band.name.as_str()),
            expected
        );
    }

    /// What to do on a band is the script's, so a setting this module
    /// understood would be one the script could not add to.
    #[test]
    fn every_setting_but_the_range_is_carried_through_untouched() {
        let plan = BandPlan::parse(concat!(
            "[[bands]]\n",
            "name = \"40m\"\n",
            "low = 7000000\n",
            "high = 7300000\n",
            "target = 7171000\n",
            "receive-mode = \"LSB\"\n",
            "monitor-gain = 0.15\n",
            "amplifier = true\n",
        ))
        .unwrap();

        let band = plan.by_name("40m").unwrap();
        assert_eq!(band.low_hz, 7_000_000);
        assert_eq!(band.high_hz, 7_300_000);
        assert_eq!(
            band.settings.get("target"),
            Some(&BandValue::Integer(7_171_000))
        );
        assert_eq!(
            band.settings.get("receive-mode"),
            Some(&BandValue::Text("LSB".to_owned()))
        );
        assert_eq!(
            band.settings.get("monitor-gain"),
            Some(&BandValue::Decimal(0.15))
        );
        assert_eq!(band.settings.get("amplifier"), Some(&BandValue::Flag(true)));
        // The range is the application's and is not repeated as a setting.
        assert!(!band.settings.contains_key("low"));
        assert!(!band.settings.contains_key("name"));
    }

    #[rstest]
    #[case::no_name("[[bands]]\nlow = 1\nhigh = 2\n")]
    #[case::no_range("[[bands]]\nname = \"40m\"\n")]
    #[case::an_empty_name("[[bands]]\nname = \"  \"\nlow = 1\nhigh = 2\n")]
    #[case::inverted("[[bands]]\nname = \"40m\"\nlow = 2\nhigh = 1\n")]
    #[case::a_negative_edge("[[bands]]\nname = \"40m\"\nlow = -1\nhigh = 2\n")]
    fn a_band_that_names_no_range_is_dropped(#[case] written: &str) {
        assert!(BandPlan::parse(written).unwrap().is_empty());
    }

    #[test]
    fn a_file_with_no_bands_in_it_is_refused_rather_than_read_as_empty() {
        assert!(BandPlan::parse("bands = 3\n").is_err());
        assert!(BandPlan::parse("").is_err());
        assert!(BandPlan::parse("[[bands]\n").is_err());
    }

    /// The buttons move by whole hertz, so a step of nothing is no step.
    #[rstest]
    #[case("step = 1000", Some(1_000))]
    #[case("step = 0", None)]
    #[case("step = \"1k\"", None)]
    #[case("", None)]
    fn the_step_is_read_for_the_interface(#[case] written: &str, #[case] expected: Option<u64>) {
        let plan = BandPlan::parse(&format!(
            "[[bands]]\nname = \"40m\"\nlow = 7000000\nhigh = 7300000\n{written}\n"
        ))
        .unwrap();

        assert_eq!(plan.by_name("40m").unwrap().step_hz(), expected);
    }
}
