use crate::image::Rgb8;
use crate::signal::Frequency;

/// MMSSTV's byte representation of Y, R-Y, and B-Y.
///
/// The `cr` and `cb` names correspond to MMSSTV's stored `RY` and `BY` values,
/// respectively. Their neutral value is 128.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct YCrCb8 {
    /// Studio-range luminance.
    pub y: u8,
    /// R-Y chroma with 128 as zero.
    pub cr: u8,
    /// B-Y chroma with 128 as zero.
    pub cb: u8,
}

/// The image-frequency range used by an SSTV mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LevelFrequencyBand {
    /// Ordinary 1500 Hz black through 2300 Hz white.
    Wide,
    /// MMSSTV narrow 2044 Hz black through 2300 Hz white.
    Narrow,
}

impl LevelFrequencyBand {
    /// Returns the nominal low endpoint.
    pub const fn low(self) -> Frequency {
        match self {
            Self::Wide => Frequency::from_hz(1500),
            Self::Narrow => Frequency::from_hz(2044),
        }
    }

    /// Returns the nominal high endpoint.
    pub const fn high(self) -> Frequency {
        Frequency::from_hz(2300)
    }

    /// Converts a byte level using MMSSTV's integer multiply/divide truncation.
    pub const fn level_to_frequency(self, level: u8) -> Frequency {
        let low = self.low().as_hz();
        let width = self.high().as_hz() - low;
        Frequency::from_hz(low + level as u32 * width / 256)
    }

    /// Converts and clamps a frequency to the nearest byte level represented on air.
    pub const fn frequency_to_level(self, frequency: Frequency) -> u8 {
        let low = self.low().as_hz();
        let high = self.high().as_hz();
        let hz = frequency.as_hz();
        if hz <= low {
            0
        } else if hz >= high {
            255
        } else {
            let width = high - low;
            (((hz - low) * 256 + width / 2) / width) as u8
        }
    }
}

fn clamp_truncated(value: f64) -> u8 {
    (value as i32).clamp(0, 255) as u8
}

/// Converts RGB using the active `GetRY` coefficients and C++ truncation used by MMSSTV.
pub fn rgb_to_y_cr_cb(rgb: Rgb8) -> YCrCb8 {
    let r = f64::from(rgb.r);
    let g = f64::from(rgb.g);
    let b = f64::from(rgb.b);
    YCrCb8 {
        y: clamp_truncated(16.0 + 0.256773 * r + 0.504097 * g + 0.097900 * b),
        cr: clamp_truncated(128.0 + 0.439187 * r - 0.367766 * g - 0.071421 * b),
        cb: clamp_truncated(128.0 - 0.148213 * r - 0.290974 * g + 0.439187 * b),
    }
}

/// Converts MMSSTV Y/R-Y/B-Y bytes using `YCtoRGB` coefficients and C++ truncation.
pub fn y_cr_cb_to_rgb(color: YCrCb8) -> Rgb8 {
    let y = f64::from(color.y) - 16.0;
    let ry = f64::from(color.cr) - 128.0;
    let by = f64::from(color.cb) - 128.0;
    Rgb8 {
        r: clamp_truncated(1.164457 * y + 1.596128 * ry),
        g: clamp_truncated(1.164457 * y - 0.813022 * ry - 0.391786 * by),
        b: clamp_truncated(1.164457 * y + 2.017364 * by),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(Rgb8::new(0, 0, 0), YCrCb8 { y: 16, cr: 128, cb: 128 })]
    #[case(Rgb8::new(255, 255, 255), YCrCb8 { y: 234, cr: 128, cb: 127 })]
    #[case(Rgb8::new(255, 0, 0), YCrCb8 { y: 81, cr: 239, cb: 90 })]
    #[case(Rgb8::new(0, 255, 0), YCrCb8 { y: 144, cr: 34, cb: 53 })]
    #[case(Rgb8::new(0, 0, 255), YCrCb8 { y: 40, cr: 109, cb: 239 })]
    fn forward_golden_points(#[case] rgb: Rgb8, #[case] expected: YCrCb8) {
        assert_eq!(rgb_to_y_cr_cb(rgb), expected);
    }

    #[rstest]
    #[case(YCrCb8 { y: 16, cr: 128, cb: 128 }, Rgb8::new(0, 0, 0))]
    #[case(YCrCb8 { y: 235, cr: 128, cb: 128 }, Rgb8::new(255, 255, 255))]
    #[case(YCrCb8 { y: 81, cr: 239, cb: 90 }, Rgb8::new(252, 0, 0))]
    fn inverse_golden_points(#[case] color: YCrCb8, #[case] expected: Rgb8) {
        assert_eq!(y_cr_cb_to_rgb(color), expected);
    }

    #[rstest]
    #[case(LevelFrequencyBand::Wide, 0, 1500)]
    #[case(LevelFrequencyBand::Wide, 128, 1900)]
    #[case(LevelFrequencyBand::Wide, 255, 2296)]
    #[case(LevelFrequencyBand::Narrow, 0, 2044)]
    #[case(LevelFrequencyBand::Narrow, 128, 2172)]
    #[case(LevelFrequencyBand::Narrow, 255, 2299)]
    fn source_frequency_endpoints(
        #[case] band: LevelFrequencyBand,
        #[case] level: u8,
        #[case] hz: u32,
    ) {
        assert_eq!(band.level_to_frequency(level), Frequency::from_hz(hz));
    }

    #[test]
    fn inverse_frequency_conversion_clamps() {
        assert_eq!(
            LevelFrequencyBand::Wide.frequency_to_level(Frequency::from_hz(1000)),
            0
        );
        assert_eq!(
            LevelFrequencyBand::Wide.frequency_to_level(Frequency::from_hz(2300)),
            255
        );
        assert_eq!(
            LevelFrequencyBand::Narrow.frequency_to_level(Frequency::from_hz(2172)),
            128
        );
    }

    #[rstest]
    #[case(LevelFrequencyBand::Wide)]
    #[case(LevelFrequencyBand::Narrow)]
    fn every_transmitted_level_roundtrips(#[case] band: LevelFrequencyBand) {
        for level in 0..=u8::MAX {
            assert_eq!(
                band.frequency_to_level(band.level_to_frequency(level)),
                level
            );
        }
    }
}
