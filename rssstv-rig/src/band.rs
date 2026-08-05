use core::cmp::Ordering;

/// What the rig was found to be tuned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    pub frequency_hz: u64,
    /// The band the frequency falls in, if it falls in one.
    pub band: Option<Band>,
}

impl Reading {
    /// Reads a frequency and names the band it sits in.
    pub fn at(frequency_hz: u64) -> Self {
        Self {
            frequency_hz,
            band: Band::for_frequency(frequency_hz),
        }
    }
}

/// An amateur band, named the way an operator writes it.
///
/// The edges are the widest any region allocates, because the band is only
/// used to name a frequency and to pick the commands the operator attached to
/// it. Deciding what may be transmitted where is the operator's licence, not
/// this table's job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Band {
    name: &'static str,
    start_hz: u64,
    end_hz: u64,
}

impl Band {
    /// Every band this crate can name, in ascending frequency order.
    pub const ALL: [Self; 19] = [
        Self::new("2200m", 135_700, 137_800),
        Self::new("630m", 472_000, 479_000),
        Self::new("160m", 1_800_000, 2_000_000),
        Self::new("80m", 3_500_000, 4_000_000),
        Self::new("60m", 5_060_000, 5_450_000),
        Self::new("40m", 7_000_000, 7_300_000),
        Self::new("30m", 10_100_000, 10_150_000),
        Self::new("20m", 14_000_000, 14_350_000),
        Self::new("17m", 18_068_000, 18_168_000),
        Self::new("15m", 21_000_000, 21_450_000),
        Self::new("12m", 24_890_000, 24_990_000),
        Self::new("10m", 28_000_000, 29_700_000),
        Self::new("6m", 50_000_000, 54_000_000),
        Self::new("4m", 70_000_000, 70_500_000),
        Self::new("2m", 144_000_000, 148_000_000),
        Self::new("1.25m", 222_000_000, 225_000_000),
        Self::new("70cm", 420_000_000, 450_000_000),
        Self::new("33cm", 902_000_000, 928_000_000),
        Self::new("23cm", 1_240_000_000, 1_300_000_000),
    ];

    const fn new(name: &'static str, start_hz: u64, end_hz: u64) -> Self {
        Self {
            name,
            start_hz,
            end_hz,
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn start_hz(self) -> u64 {
        self.start_hz
    }

    pub const fn end_hz(self) -> u64 {
        self.end_hz
    }

    /// Returns the band `frequency_hz` falls in, if it falls in one at all.
    ///
    /// A frequency between the bands has no name to report. That is a normal
    /// state rather than a fault: a rig parked on a broadcast station or on a
    /// listening frequency is still something the operator may be doing.
    pub fn for_frequency(frequency_hz: u64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|band| (band.start_hz..=band.end_hz).contains(&frequency_hz))
    }

    /// Resolves a band written by hand, tolerating a difference in case.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|band| band.name.eq_ignore_ascii_case(name))
    }
}

/// Ordered by frequency rather than by name, so a map keyed by band reads in
/// the order a band plan does instead of putting 10m before 160m.
impl Ord for Band {
    fn cmp(&self, other: &Self) -> Ordering {
        self.start_hz.cmp(&other.start_hz)
    }
}

impl PartialOrd for Band {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl core::fmt::Display for Band {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.name)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(7_178_000, Some("40m"))]
    #[case(7_000_000, Some("40m"))]
    #[case(7_300_000, Some("40m"))]
    #[case(14_230_000, Some("20m"))]
    #[case(144_500_000, Some("2m"))]
    #[case(1_296_000_000, Some("23cm"))]
    // Between the bands, which is where a rig tuned to a broadcast sits.
    #[case(6_000_000, None)]
    #[case(0, None)]
    #[case(30_000_000, None)]
    fn a_frequency_names_the_band_it_sits_in(
        #[case] frequency_hz: u64,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(Band::for_frequency(frequency_hz).map(Band::name), expected);
    }

    #[rstest]
    #[case("40m", Some("40m"))]
    #[case("  70CM  ", Some("70cm"))]
    #[case("40", None)]
    #[case("", None)]
    fn a_written_band_name_resolves_without_regard_to_case(
        #[case] written: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(Band::from_name(written).map(Band::name), expected);
    }

    #[test]
    fn the_table_is_ordered_and_does_not_overlap() {
        for pair in Band::ALL.windows(2) {
            assert!(
                pair[0].end_hz() < pair[1].start_hz(),
                "{} and {} overlap",
                pair[0],
                pair[1]
            );
        }
    }
}
