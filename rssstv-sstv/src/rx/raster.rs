use crate::mode::Mode;

#[derive(Clone, Copy)]
pub(super) enum RasterFamily {
    Martin,
    Scottie,
    Robot36,
    Robot72,
    Pd,
}

#[derive(Clone, Copy)]
pub(super) struct RasterProfile {
    pub(super) family: RasterFamily,
    pub(super) period_ps: u64,
    pub(super) sync_center_ps: u64,
    pub(super) component_ps: u64,
}

impl RasterProfile {
    pub(super) fn for_mode(mode: Mode) -> Option<Self> {
        let (family, sync_center_ps, component_ps) = match mode {
            Mode::Martin1 => (RasterFamily::Martin, 2_431_000_000, 146_432_000_000),
            Mode::Martin2 => (RasterFamily::Martin, 2_431_000_000, 73_216_000_000),
            Mode::Scottie1 => (RasterFamily::Scottie, 283_980_000_000, 138_240_000_000),
            Mode::Scottie2 => (RasterFamily::Scottie, 183_628_000_000, 88_064_000_000),
            Mode::ScottieDx => (RasterFamily::Scottie, 698_700_000_000, 345_600_000_000),
            Mode::Robot36 => (RasterFamily::Robot36, 4_500_000_000, 44_000_000_000),
            Mode::Robot72 => (RasterFamily::Robot72, 4_500_000_000, 69_000_000_000),
            Mode::Pd50 => (RasterFamily::Pd, 10_000_000_000, 91_520_000_000),
            Mode::Pd90 => (RasterFamily::Pd, 10_000_000_000, 170_240_000_000),
            Mode::Pd120 => (RasterFamily::Pd, 10_000_000_000, 121_600_000_000),
            Mode::Pd160 => (RasterFamily::Pd, 10_000_000_000, 195_584_000_000),
            Mode::Pd180 => (RasterFamily::Pd, 10_000_000_000, 183_040_000_000),
            Mode::Pd240 => (RasterFamily::Pd, 10_000_000_000, 244_480_000_000),
            Mode::Pd290 => (RasterFamily::Pd, 10_000_000_000, 228_800_000_000),
            _ => return None,
        };
        Some(Self {
            family,
            period_ps: mode.spec().period().as_picos(),
            sync_center_ps,
            component_ps,
        })
    }

    pub(super) fn component_starts(self) -> [u64; 4] {
        match self.family {
            RasterFamily::Martin => [
                5_434_000_000,
                6_006_000_000 + self.component_ps,
                6_578_000_000 + 2 * self.component_ps,
                0,
            ],
            RasterFamily::Scottie => [
                1_500_000_000,
                3_000_000_000 + self.component_ps,
                13_500_000_000 + 2 * self.component_ps,
                0,
            ],
            RasterFamily::Robot36 => [12_000_000_000, 106_000_000_000, 0, 0],
            RasterFamily::Robot72 => [12_000_000_000, 156_000_000_000, 231_000_000_000, 0],
            RasterFamily::Pd => [
                22_080_000_000,
                22_080_000_000 + self.component_ps,
                22_080_000_000 + 2 * self.component_ps,
                22_080_000_000 + 3 * self.component_ps,
            ],
        }
    }
}
