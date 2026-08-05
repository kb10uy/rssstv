//! Mode values are derived from `sstv.h`, `sstv.cpp`, and the transmit raster
//! generators in `Main.cpp` from MMSSTV 1.13A.

mod scan;

use crate::{color::LevelFrequencyBand, time::SstvDuration};

pub use scan::{RasterScan, ScanChannel, ScanContent, ScanSegment};

/// An MMSSTV SSTV mode, in the source enum and UI table order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Mode {
    /// Robot 36.
    Robot36,
    /// Robot 72.
    Robot72,
    /// AVT 90.
    Avt90,
    /// Scottie 1.
    Scottie1,
    /// Scottie 2.
    Scottie2,
    /// Scottie DX.
    ScottieDx,
    /// Martin 1.
    Martin1,
    /// Martin 2.
    Martin2,
    /// Wraase SC2 180.
    Sc2_180,
    /// Wraase SC2 120.
    Sc2_120,
    /// Wraase SC2 60.
    Sc2_60,
    /// PD50.
    Pd50,
    /// PD90.
    Pd90,
    /// PD120.
    Pd120,
    /// PD160.
    Pd160,
    /// PD180.
    Pd180,
    /// PD240.
    Pd240,
    /// PD290.
    Pd290,
    /// Pasokon P3.
    P3,
    /// Pasokon P5.
    P5,
    /// Pasokon P7.
    P7,
    /// MMSSTV MR73.
    Mr73,
    /// MMSSTV MR90.
    Mr90,
    /// MMSSTV MR115.
    Mr115,
    /// MMSSTV MR140.
    Mr140,
    /// MMSSTV MR175.
    Mr175,
    /// MMSSTV MP73.
    Mp73,
    /// MMSSTV MP115.
    Mp115,
    /// MMSSTV MP140.
    Mp140,
    /// MMSSTV MP175.
    Mp175,
    /// MMSSTV ML180.
    Ml180,
    /// MMSSTV ML240.
    Ml240,
    /// MMSSTV ML280.
    Ml280,
    /// MMSSTV ML320.
    Ml320,
    /// Robot 24.
    Robot24,
    /// Robot monochrome 8.
    Bw8,
    /// Robot monochrome 12.
    Bw12,
    /// Narrow MP73.
    Mp73N,
    /// Narrow MP110.
    Mp110N,
    /// Narrow MP140.
    Mp140N,
    /// Narrow MC110.
    Mc110N,
    /// Narrow MC140.
    Mc140N,
    /// Narrow MC180.
    Mc180N,
}

impl Mode {
    /// Every mode inventoried by MMSSTV, in its source enum order.
    pub const ALL: [Self; 43] = [
        Self::Robot36,
        Self::Robot72,
        Self::Avt90,
        Self::Scottie1,
        Self::Scottie2,
        Self::ScottieDx,
        Self::Martin1,
        Self::Martin2,
        Self::Sc2_180,
        Self::Sc2_120,
        Self::Sc2_60,
        Self::Pd50,
        Self::Pd90,
        Self::Pd120,
        Self::Pd160,
        Self::Pd180,
        Self::Pd240,
        Self::Pd290,
        Self::P3,
        Self::P5,
        Self::P7,
        Self::Mr73,
        Self::Mr90,
        Self::Mr115,
        Self::Mr140,
        Self::Mr175,
        Self::Mp73,
        Self::Mp115,
        Self::Mp140,
        Self::Mp175,
        Self::Ml180,
        Self::Ml240,
        Self::Ml280,
        Self::Ml320,
        Self::Robot24,
        Self::Bw8,
        Self::Bw12,
        Self::Mp73N,
        Self::Mp110N,
        Self::Mp140N,
        Self::Mc110N,
        Self::Mc140N,
        Self::Mc180N,
    ];

    /// Returns this mode's immutable protocol specification.
    pub const fn spec(self) -> &'static ModeSpec {
        &MODE_SPECS[self as usize]
    }

    /// Returns the timed raster segments shared by transmit and receive.
    ///
    /// Modes without an implemented raster description return an empty scan.
    pub const fn scan(self) -> RasterScan {
        scan::for_mode(self)
    }

    /// Returns the mode identified by a seven-bit conventional VIS value.
    pub fn from_conventional_vis(code: u8) -> Option<Self> {
        Self::find(
            |identification| matches!(identification, ModeIdentification::ConventionalVis { code: value, .. } if value == code),
        )
    }

    /// Returns the mode identified by a received conventional VIS byte.
    ///
    /// The byte is matched whole, as MMSSTV does, rather than by checking
    /// parity and looking up the low seven bits. B/W 12 is inventoried with the
    /// parity-violating byte `0x86` that MMSSTV both sends and accepts, so a
    /// parity test would reject a mode the reference implementation decodes.
    pub fn from_raw_vis(raw: u8) -> Option<Self> {
        Self::find(|identification| {
            matches!(
                identification,
                ModeIdentification::ConventionalVis { raw: value, .. } if value == raw
            )
        })
    }

    /// Returns the mode identified by the second byte of an extended VIS.
    ///
    /// The first byte is always [`Self::EXTENDED_VIS_PREFIX`].
    pub fn from_extended_vis(code: u8) -> Option<Self> {
        Self::find(
            |identification| matches!(identification, ModeIdentification::ExtendedVis { prefix, code: value } if prefix == Self::EXTENDED_VIS_PREFIX && value == code),
        )
    }

    /// Returns the narrow mode identified by a six-bit FSK value.
    pub fn from_narrow_vis(code: u8) -> Option<Self> {
        Self::find(
            |identification| matches!(identification, ModeIdentification::NarrowVis { code: value } if value == code),
        )
    }

    /// The first byte of every MMSSTV extended VIS sequence.
    pub const EXTENDED_VIS_PREFIX: u8 = 0x23;

    fn find(predicate: impl Fn(ModeIdentification) -> bool) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| predicate(mode.spec().identification()))
    }
}

/// The originating or structurally related mode family.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModeFamily {
    /// Robot color and monochrome modes.
    Robot,
    /// AVT synchronous modes.
    Avt,
    /// Scottie modes.
    Scottie,
    /// Martin modes.
    Martin,
    /// Wraase SC-2 modes.
    Sc2,
    /// PD paired-row modes.
    Pd,
    /// Pasokon modes.
    Pasokon,
    /// MMSSTV MR modes.
    Mr,
    /// MMSSTV MP modes.
    Mp,
    /// MMSSTV ML modes.
    Ml,
    /// MMSSTV narrow MP modes.
    NarrowMp,
    /// MMSSTV narrow direct-color modes.
    NarrowMc,
}

/// How picture components and rows are grouped in one raster period.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RasterOrganization {
    /// One luminance row with alternating R-Y and B-Y rows.
    AlternatingYCrCb,
    /// One Y, R-Y, and B-Y row in each period.
    YCrCb,
    /// Two rows encoded as Y0, R-Y, B-Y, and Y1.
    PairedYCrCb,
    /// Direct green, blue, and red components.
    DirectGbr,
    /// Direct red, green, and blue components.
    DirectRgb,
    /// A luminance-only raster unit representing two image rows.
    PairedLuminance,
}

/// A mode's on-air identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModeIdentification {
    /// Conventional seven-bit VIS and MMSSTV's parity-inclusive transmitted byte.
    ConventionalVis {
        /// Public seven-bit VIS value.
        code: u8,
        /// Eight transmitted bits including even parity.
        raw: u8,
    },
    /// MMSSTV's two-byte extended VIS.
    ExtendedVis {
        /// Extension prefix, always `0x23` for inventoried modes.
        prefix: u8,
        /// Parity-bearing second byte.
        code: u8,
    },
    /// MMSSTV narrow FSK mode value.
    NarrowVis {
        /// Six-bit value sent between `0x2d, 0x15` and its check value.
        code: u8,
    },
}

/// Implementation availability for one direction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Support {
    /// The protocol is in the initial implementation set.
    Supported,
    /// The protocol metadata is known but encoding or decoding is not implemented.
    NotImplemented,
}

/// Immutable source-derived metadata for an SSTV mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModeSpec {
    name: &'static str,
    family: ModeFamily,
    width: u16,
    height: u16,
    active_rows: u16,
    rows_per_raster_unit: u8,
    period: SstvDuration,
    identification: ModeIdentification,
    raster: RasterOrganization,
    signal_band: LevelFrequencyBand,
    encode_support: Support,
    decode_support: Support,
}

impl ModeSpec {
    /// Returns MMSSTV's displayed mode name.
    pub const fn name(&self) -> &'static str {
        self.name
    }
    /// Returns the mode family.
    pub const fn family(&self) -> ModeFamily {
        self.family
    }
    /// Returns the decoded image width.
    pub const fn width(&self) -> u16 {
        self.width
    }
    /// Returns the decoded image height.
    pub const fn height(&self) -> u16 {
        self.height
    }
    /// Returns the number of active picture rows.
    pub const fn active_rows(&self) -> u16 {
        self.active_rows
    }
    /// Returns the image rows represented by one nominal raster period.
    pub const fn rows_per_raster_unit(&self) -> u8 {
        self.rows_per_raster_unit
    }
    /// Returns the nominal horizontal synchronization interval.
    pub const fn period(&self) -> SstvDuration {
        self.period
    }
    /// Returns the mode's on-air identification.
    pub const fn identification(&self) -> ModeIdentification {
        self.identification
    }
    /// Returns the component organization of a raster unit.
    pub const fn raster_organization(&self) -> RasterOrganization {
        self.raster
    }
    /// Returns the wide or MMSSTV narrow image-frequency band.
    pub const fn signal_band(&self) -> LevelFrequencyBand {
        self.signal_band
    }
    /// Returns encoder availability.
    pub const fn encode_support(&self) -> Support {
        self.encode_support
    }
    /// Returns decoder availability.
    pub const fn decode_support(&self) -> Support {
        self.decode_support
    }
    /// Returns the seven-bit VIS value for a conventional VIS mode.
    pub const fn vis_code(&self) -> Option<u8> {
        match self.identification {
            ModeIdentification::ConventionalVis { code, .. } => Some(code),
            _ => None,
        }
    }
    /// Returns the parity-inclusive VIS byte for a conventional VIS mode.
    pub const fn raw_vis(&self) -> Option<u8> {
        match self.identification {
            ModeIdentification::ConventionalVis { raw, .. } => Some(raw),
            _ => None,
        }
    }
}

const SUP: Support = Support::Supported;
const NYI: Support = Support::NotImplemented;
const WIDE: LevelFrequencyBand = LevelFrequencyBand::Wide;
const NARROW: LevelFrequencyBand = LevelFrequencyBand::Narrow;

const fn vis(code: u8, raw: u8) -> ModeIdentification {
    ModeIdentification::ConventionalVis { code, raw }
}

const fn ext(code: u8) -> ModeIdentification {
    ModeIdentification::ExtendedVis {
        prefix: Mode::EXTENDED_VIS_PREFIX,
        code,
    }
}

const fn narrow(code: u8) -> ModeIdentification {
    ModeIdentification::NarrowVis { code }
}

macro_rules! spec {
    ($name:expr, $family:expr, $width:expr, $height:expr, $active_rows:expr, $rows:expr,
     $period_ps:expr, $identification:expr, $raster:expr, $signal_band:expr,
     $encode_support:expr, $decode_support:expr $(,)?) => {
        ModeSpec {
            name: $name,
            family: $family,
            width: $width,
            height: $height,
            active_rows: $active_rows,
            rows_per_raster_unit: $rows,
            period: SstvDuration::from_picos($period_ps),
            identification: $identification,
            raster: $raster,
            signal_band: $signal_band,
            encode_support: $encode_support,
            decode_support: $decode_support,
        }
    };
}

const MODE_SPECS: [ModeSpec; 43] = [
    spec!(
        "Robot 36",
        ModeFamily::Robot,
        320,
        240,
        240,
        1,
        150_000_000_000,
        vis(0x08, 0x88),
        RasterOrganization::AlternatingYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "Robot 72",
        ModeFamily::Robot,
        320,
        240,
        240,
        1,
        300_000_000_000,
        vis(0x0c, 0x0c),
        RasterOrganization::YCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "AVT 90",
        ModeFamily::Avt,
        320,
        240,
        240,
        1,
        375_000_000_000,
        vis(0x44, 0x44),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "Scottie 1",
        ModeFamily::Scottie,
        320,
        256,
        256,
        1,
        428_220_000_000,
        vis(0x3c, 0x3c),
        RasterOrganization::DirectGbr,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "Scottie 2",
        ModeFamily::Scottie,
        320,
        256,
        256,
        1,
        277_692_000_000,
        vis(0x38, 0xb8),
        RasterOrganization::DirectGbr,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "ScottieDX",
        ModeFamily::Scottie,
        320,
        256,
        256,
        1,
        1_050_300_000_000,
        vis(0x4c, 0xcc),
        RasterOrganization::DirectGbr,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "Martin 1",
        ModeFamily::Martin,
        320,
        256,
        256,
        1,
        446_446_000_000,
        vis(0x2c, 0xac),
        RasterOrganization::DirectGbr,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "Martin 2",
        ModeFamily::Martin,
        320,
        256,
        256,
        1,
        226_798_000_000,
        vis(0x28, 0x28),
        RasterOrganization::DirectGbr,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "SC2 180",
        ModeFamily::Sc2,
        320,
        256,
        256,
        1,
        711_043_700_000,
        vis(0x37, 0xb7),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "SC2 120",
        ModeFamily::Sc2,
        320,
        256,
        256,
        1,
        475_522_480_000,
        vis(0x3f, 0x3f),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "SC2 60",
        ModeFamily::Sc2,
        320,
        256,
        256,
        1,
        240_384_600_000,
        vis(0x3b, 0xbb),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "PD50",
        ModeFamily::Pd,
        320,
        256,
        256,
        2,
        388_160_000_000,
        vis(0x5d, 0xdd),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD90",
        ModeFamily::Pd,
        320,
        256,
        256,
        2,
        703_040_000_000,
        vis(0x63, 0x63),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD120",
        ModeFamily::Pd,
        640,
        496,
        496,
        2,
        508_480_000_000,
        vis(0x5f, 0x5f),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD160",
        ModeFamily::Pd,
        512,
        400,
        400,
        2,
        804_416_000_000,
        vis(0x62, 0xe2),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD180",
        ModeFamily::Pd,
        640,
        496,
        496,
        2,
        754_240_000_000,
        vis(0x60, 0x60),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD240",
        ModeFamily::Pd,
        640,
        496,
        496,
        2,
        1_000_000_000_000,
        vis(0x61, 0xe1),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "PD290",
        ModeFamily::Pd,
        800,
        616,
        616,
        2,
        937_280_000_000,
        vis(0x5e, 0xde),
        RasterOrganization::PairedYCrCb,
        WIDE,
        SUP,
        SUP,
    ),
    spec!(
        "P3",
        ModeFamily::Pasokon,
        640,
        496,
        496,
        1,
        409_375_000_000,
        vis(0x71, 0x71),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "P5",
        ModeFamily::Pasokon,
        640,
        496,
        496,
        1,
        614_062_500_000,
        vis(0x72, 0x72),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "P7",
        ModeFamily::Pasokon,
        640,
        496,
        496,
        1,
        818_750_000_000,
        vis(0x73, 0xf3),
        RasterOrganization::DirectRgb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MR73",
        ModeFamily::Mr,
        320,
        256,
        256,
        1,
        286_300_000_000,
        ext(0x45),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MR90",
        ModeFamily::Mr,
        320,
        256,
        256,
        1,
        352_300_000_000,
        ext(0x46),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MR115",
        ModeFamily::Mr,
        320,
        256,
        256,
        1,
        450_300_000_000,
        ext(0x49),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MR140",
        ModeFamily::Mr,
        320,
        256,
        256,
        1,
        548_300_000_000,
        ext(0x4a),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MR175",
        ModeFamily::Mr,
        320,
        256,
        256,
        1,
        684_300_000_000,
        ext(0x4c),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MP73",
        ModeFamily::Mp,
        320,
        256,
        256,
        2,
        570_000_000_000,
        ext(0x25),
        RasterOrganization::PairedYCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MP115",
        ModeFamily::Mp,
        320,
        256,
        256,
        2,
        902_000_000_000,
        ext(0x29),
        RasterOrganization::PairedYCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MP140",
        ModeFamily::Mp,
        320,
        256,
        256,
        2,
        1_090_000_000_000,
        ext(0x2a),
        RasterOrganization::PairedYCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MP175",
        ModeFamily::Mp,
        320,
        256,
        256,
        2,
        1_370_000_000_000,
        ext(0x2c),
        RasterOrganization::PairedYCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "ML180",
        ModeFamily::Ml,
        640,
        496,
        496,
        1,
        363_300_000_000,
        ext(0x85),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "ML240",
        ModeFamily::Ml,
        640,
        496,
        496,
        1,
        483_300_000_000,
        ext(0x86),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "ML280",
        ModeFamily::Ml,
        640,
        496,
        496,
        1,
        565_300_000_000,
        ext(0x89),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "ML320",
        ModeFamily::Ml,
        640,
        496,
        496,
        1,
        645_300_000_000,
        ext(0x8a),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "Robot 24",
        ModeFamily::Robot,
        320,
        240,
        240,
        2,
        200_000_000_000,
        vis(0x04, 0x84),
        RasterOrganization::YCrCb,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "B/W 8",
        ModeFamily::Robot,
        320,
        240,
        240,
        2,
        66_897_090_000,
        vis(0x02, 0x82),
        RasterOrganization::PairedLuminance,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "B/W 12",
        ModeFamily::Robot,
        320,
        240,
        240,
        2,
        100_000_000_000,
        vis(0x06, 0x86),
        RasterOrganization::PairedLuminance,
        WIDE,
        NYI,
        NYI,
    ),
    spec!(
        "MP73-N",
        ModeFamily::NarrowMp,
        320,
        256,
        256,
        2,
        570_000_000_000,
        narrow(0x02),
        RasterOrganization::PairedYCrCb,
        NARROW,
        NYI,
        NYI,
    ),
    spec!(
        "MP110-N",
        ModeFamily::NarrowMp,
        320,
        256,
        256,
        2,
        858_000_000_000,
        narrow(0x04),
        RasterOrganization::PairedYCrCb,
        NARROW,
        NYI,
        NYI,
    ),
    spec!(
        "MP140-N",
        ModeFamily::NarrowMp,
        320,
        256,
        256,
        2,
        1_090_000_000_000,
        narrow(0x05),
        RasterOrganization::PairedYCrCb,
        NARROW,
        NYI,
        NYI,
    ),
    spec!(
        "MC110-N",
        ModeFamily::NarrowMc,
        320,
        256,
        256,
        1,
        428_500_000_000,
        narrow(0x14),
        RasterOrganization::DirectRgb,
        NARROW,
        NYI,
        NYI,
    ),
    spec!(
        "MC140-N",
        ModeFamily::NarrowMc,
        320,
        256,
        256,
        1,
        548_500_000_000,
        narrow(0x15),
        RasterOrganization::DirectRgb,
        NARROW,
        NYI,
        NYI,
    ),
    spec!(
        "MC180-N",
        ModeFamily::NarrowMc,
        320,
        256,
        256,
        1,
        704_500_000_000,
        narrow(0x16),
        RasterOrganization::DirectRgb,
        NARROW,
        NYI,
        NYI,
    ),
];

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Mode::Robot36, "Robot 36", 320, 240, 240, 1, 150_000_000_000)]
    #[case(Mode::Robot72, "Robot 72", 320, 240, 240, 1, 300_000_000_000)]
    #[case(Mode::Avt90, "AVT 90", 320, 240, 240, 1, 375_000_000_000)]
    #[case(Mode::Scottie1, "Scottie 1", 320, 256, 256, 1, 428_220_000_000)]
    #[case(Mode::Scottie2, "Scottie 2", 320, 256, 256, 1, 277_692_000_000)]
    #[case(Mode::ScottieDx, "ScottieDX", 320, 256, 256, 1, 1_050_300_000_000)]
    #[case(Mode::Martin1, "Martin 1", 320, 256, 256, 1, 446_446_000_000)]
    #[case(Mode::Martin2, "Martin 2", 320, 256, 256, 1, 226_798_000_000)]
    #[case(Mode::Sc2_180, "SC2 180", 320, 256, 256, 1, 711_043_700_000)]
    #[case(Mode::Sc2_120, "SC2 120", 320, 256, 256, 1, 475_522_480_000)]
    #[case(Mode::Sc2_60, "SC2 60", 320, 256, 256, 1, 240_384_600_000)]
    #[case(Mode::Pd50, "PD50", 320, 256, 256, 2, 388_160_000_000)]
    #[case(Mode::Pd90, "PD90", 320, 256, 256, 2, 703_040_000_000)]
    #[case(Mode::Pd120, "PD120", 640, 496, 496, 2, 508_480_000_000)]
    #[case(Mode::Pd160, "PD160", 512, 400, 400, 2, 804_416_000_000)]
    #[case(Mode::Pd180, "PD180", 640, 496, 496, 2, 754_240_000_000)]
    #[case(Mode::Pd240, "PD240", 640, 496, 496, 2, 1_000_000_000_000)]
    #[case(Mode::Pd290, "PD290", 800, 616, 616, 2, 937_280_000_000)]
    #[case(Mode::P3, "P3", 640, 496, 496, 1, 409_375_000_000)]
    #[case(Mode::P5, "P5", 640, 496, 496, 1, 614_062_500_000)]
    #[case(Mode::P7, "P7", 640, 496, 496, 1, 818_750_000_000)]
    #[case(Mode::Mr73, "MR73", 320, 256, 256, 1, 286_300_000_000)]
    #[case(Mode::Mr90, "MR90", 320, 256, 256, 1, 352_300_000_000)]
    #[case(Mode::Mr115, "MR115", 320, 256, 256, 1, 450_300_000_000)]
    #[case(Mode::Mr140, "MR140", 320, 256, 256, 1, 548_300_000_000)]
    #[case(Mode::Mr175, "MR175", 320, 256, 256, 1, 684_300_000_000)]
    #[case(Mode::Mp73, "MP73", 320, 256, 256, 2, 570_000_000_000)]
    #[case(Mode::Mp115, "MP115", 320, 256, 256, 2, 902_000_000_000)]
    #[case(Mode::Mp140, "MP140", 320, 256, 256, 2, 1_090_000_000_000)]
    #[case(Mode::Mp175, "MP175", 320, 256, 256, 2, 1_370_000_000_000)]
    #[case(Mode::Ml180, "ML180", 640, 496, 496, 1, 363_300_000_000)]
    #[case(Mode::Ml240, "ML240", 640, 496, 496, 1, 483_300_000_000)]
    #[case(Mode::Ml280, "ML280", 640, 496, 496, 1, 565_300_000_000)]
    #[case(Mode::Ml320, "ML320", 640, 496, 496, 1, 645_300_000_000)]
    #[case(Mode::Robot24, "Robot 24", 320, 240, 240, 2, 200_000_000_000)]
    #[case(Mode::Bw8, "B/W 8", 320, 240, 240, 2, 66_897_090_000)]
    #[case(Mode::Bw12, "B/W 12", 320, 240, 240, 2, 100_000_000_000)]
    #[case(Mode::Mp73N, "MP73-N", 320, 256, 256, 2, 570_000_000_000)]
    #[case(Mode::Mp110N, "MP110-N", 320, 256, 256, 2, 858_000_000_000)]
    #[case(Mode::Mp140N, "MP140-N", 320, 256, 256, 2, 1_090_000_000_000)]
    #[case(Mode::Mc110N, "MC110-N", 320, 256, 256, 1, 428_500_000_000)]
    #[case(Mode::Mc140N, "MC140-N", 320, 256, 256, 1, 548_500_000_000)]
    #[case(Mode::Mc180N, "MC180-N", 320, 256, 256, 1, 704_500_000_000)]
    fn all_source_specs(
        #[case] mode: Mode,
        #[case] name: &str,
        #[case] width: u16,
        #[case] height: u16,
        #[case] active_rows: u16,
        #[case] rows: u8,
        #[case] period_ps: u64,
    ) {
        let spec = mode.spec();
        assert_eq!(spec.name(), name);
        assert_eq!((spec.width(), spec.height()), (width, height));
        assert_eq!(spec.active_rows(), active_rows);
        assert_eq!(spec.rows_per_raster_unit(), rows);
        assert_eq!(spec.period().as_picos(), period_ps);
    }

    #[test]
    fn all_has_every_discriminant_once() {
        assert_eq!(Mode::ALL.len(), 43);
        for (index, mode) in Mode::ALL.iter().enumerate() {
            assert_eq!(*mode as usize, index);
        }
    }

    #[test]
    fn initial_support_set_is_exact() {
        let supported = [
            Mode::Martin1,
            Mode::Martin2,
            Mode::Scottie1,
            Mode::Scottie2,
            Mode::ScottieDx,
            Mode::Robot36,
            Mode::Robot72,
            Mode::Pd50,
            Mode::Pd90,
            Mode::Pd120,
            Mode::Pd160,
            Mode::Pd180,
            Mode::Pd240,
            Mode::Pd290,
        ];
        for mode in Mode::ALL {
            let expected = if supported.contains(&mode) { SUP } else { NYI };
            assert_eq!(mode.spec().encode_support(), expected, "{mode:?}");
            assert_eq!(mode.spec().decode_support(), expected, "{mode:?}");
        }
    }

    #[test]
    fn supported_modes_have_source_raw_vis() {
        assert_eq!(Mode::Martin1.spec().raw_vis(), Some(0xac));
        assert_eq!(Mode::Robot36.spec().raw_vis(), Some(0x88));
        assert_eq!(Mode::Pd290.spec().raw_vis(), Some(0xde));
    }

    #[test]
    fn every_identifier_is_unique_and_round_trips() {
        for mode in Mode::ALL {
            match mode.spec().identification() {
                ModeIdentification::ConventionalVis { code, raw } => {
                    assert_eq!(Mode::from_conventional_vis(code), Some(mode), "{mode:?}");
                    assert_eq!(Mode::from_raw_vis(raw), Some(mode), "{mode:?}");
                    assert_eq!(raw & 0x7f, code, "{mode:?}");
                }
                ModeIdentification::ExtendedVis { prefix, code } => {
                    assert_eq!(prefix, Mode::EXTENDED_VIS_PREFIX, "{mode:?}");
                    assert_eq!(Mode::from_extended_vis(code), Some(mode), "{mode:?}");
                }
                ModeIdentification::NarrowVis { code } => {
                    assert_eq!(Mode::from_narrow_vis(code), Some(mode), "{mode:?}");
                }
            }
        }
    }

    #[test]
    fn only_b_w_12_carries_a_parity_violating_vis_byte() {
        let violating: Vec<_> = Mode::ALL
            .into_iter()
            .filter(|mode| {
                mode.spec()
                    .raw_vis()
                    .is_some_and(|raw| !raw.count_ones().is_multiple_of(2))
            })
            .collect();
        assert_eq!(violating, [Mode::Bw12]);
        assert_eq!(Mode::from_raw_vis(0x86), Some(Mode::Bw12));
        assert_eq!(Mode::from_raw_vis(0x06), None);
    }

    #[test]
    fn unknown_identifiers_are_rejected() {
        assert_eq!(Mode::from_conventional_vis(0x7f), None);
        assert_eq!(Mode::from_raw_vis(0x2c), None);
        assert_eq!(Mode::from_raw_vis(0xac), Some(Mode::Martin1));
        assert_eq!(Mode::from_extended_vis(0x00), None);
        assert_eq!(Mode::from_narrow_vis(0x3f), None);
        assert_eq!(Mode::Avt90.spec().vis_code(), Some(0x44));
        assert_eq!(Mode::Mr73.spec().vis_code(), None);
    }
}
