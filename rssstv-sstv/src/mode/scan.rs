//! Raster segment tables shared by the transmit encoder and the receive decoder.
//!
//! Segment order and durations are derived from the transmit line generators in
//! `Main.cpp` from MMSSTV 1.13A.

use crate::mode::Mode;
use crate::signal::{Frequency, TxComponent};
use crate::time::SstvDuration;

/// A picture channel swept across the raster width by one segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ScanChannel {
    /// Direct red.
    Red,
    /// Direct green.
    Green,
    /// Direct blue.
    Blue,
    /// Luminance.
    Luminance,
    /// R-Y chrominance.
    RedDifference,
    /// B-Y chrominance.
    BlueDifference,
}

/// The value carried by one raster segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ScanContent {
    /// A constant tone.
    Tone(Frequency),
    /// One picture channel swept across the raster width.
    Pixels {
        /// Channel carried by the segment.
        channel: ScanChannel,
        /// Image row within the raster unit.
        row_offset: u8,
    },
}

/// One timed segment of a raster unit.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScanSegment {
    component: TxComponent,
    content: ScanContent,
    duration: SstvDuration,
}

impl ScanSegment {
    /// Returns the protocol role of the segment.
    pub const fn component(self) -> TxComponent {
        self.component
    }

    /// Returns the segment content.
    pub const fn content(self) -> ScanContent {
        self.content
    }

    /// Returns the segment duration.
    pub const fn duration(self) -> SstvDuration {
        self.duration
    }
}

/// The segment tables describing one mode's raster unit.
///
/// Modes whose raster alternates between two forms, such as Robot 36, expose a
/// second table used for odd raster units. Both tables share the same segment
/// geometry, so receive timing may be derived from unit zero alone.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RasterScan {
    leading: &'static [ScanSegment],
    primary: &'static [ScanSegment],
    alternate: &'static [ScanSegment],
}

impl RasterScan {
    /// Returns the segments sent once before the first raster unit.
    pub const fn leading(self) -> &'static [ScanSegment] {
        self.leading
    }

    /// Returns the segments of raster unit `unit`.
    pub const fn unit(self, unit: usize) -> &'static [ScanSegment] {
        if !self.alternate.is_empty() && unit % 2 == 1 {
            self.alternate
        } else {
            self.primary
        }
    }

    /// Returns whether the mode has no implemented raster description.
    pub const fn is_empty(self) -> bool {
        self.primary.is_empty()
    }

    /// Returns each segment of `unit` with its offset from the raster unit start.
    pub fn offsets(self, unit: usize) -> impl Iterator<Item = (SstvDuration, ScanSegment)> {
        self.unit(unit)
            .iter()
            .scan(SstvDuration::ZERO, |offset, segment| {
                let start = *offset;
                *offset = offset.checked_add(segment.duration)?;
                Some((start, *segment))
            })
    }

    /// Returns the midpoint of the first synchronization segment of `unit`.
    pub fn sync_center(self, unit: usize) -> Option<SstvDuration> {
        self.offsets(unit).find_map(|(offset, segment)| {
            (segment.component == TxComponent::Sync).then(|| {
                SstvDuration::from_picos(offset.as_picos() + segment.duration.as_picos() / 2)
            })
        })
    }

    /// Returns the total duration of `unit`.
    pub fn duration(self, unit: usize) -> Option<SstvDuration> {
        self.unit(unit)
            .iter()
            .try_fold(SstvDuration::ZERO, |total, segment| {
                total.checked_add(segment.duration)
            })
    }
}

const fn tone(component: TxComponent, hz: u32, duration_ps: u64) -> ScanSegment {
    ScanSegment {
        component,
        content: ScanContent::Tone(Frequency::from_hz(hz)),
        duration: SstvDuration::from_picos(duration_ps),
    }
}

const fn pixels(
    component: TxComponent,
    channel: ScanChannel,
    row_offset: u8,
    duration_ps: u64,
) -> ScanSegment {
    ScanSegment {
        component,
        content: ScanContent::Pixels {
            channel,
            row_offset,
        },
        duration: SstvDuration::from_picos(duration_ps),
    }
}

const fn martin(component_ps: u64) -> [ScanSegment; 8] {
    [
        tone(TxComponent::Sync, 1200, 4_862_000_000),
        tone(TxComponent::Porch, 1500, 572_000_000),
        pixels(TxComponent::Green, ScanChannel::Green, 0, component_ps),
        tone(TxComponent::Porch, 1500, 572_000_000),
        pixels(TxComponent::Blue, ScanChannel::Blue, 0, component_ps),
        tone(TxComponent::Porch, 1500, 572_000_000),
        pixels(TxComponent::Red, ScanChannel::Red, 0, component_ps),
        tone(TxComponent::Porch, 1500, 572_000_000),
    ]
}

const fn scottie(component_ps: u64) -> [ScanSegment; 7] {
    [
        tone(TxComponent::Porch, 1500, 1_500_000_000),
        pixels(TxComponent::Green, ScanChannel::Green, 0, component_ps),
        tone(TxComponent::Porch, 1500, 1_500_000_000),
        pixels(TxComponent::Blue, ScanChannel::Blue, 0, component_ps),
        tone(TxComponent::Sync, 1200, 9_000_000_000),
        tone(TxComponent::Porch, 1500, 1_500_000_000),
        pixels(TxComponent::Red, ScanChannel::Red, 0, component_ps),
    ]
}

const fn robot36(
    selector_hz: u32,
    chroma_component: TxComponent,
    chroma: ScanChannel,
) -> [ScanSegment; 6] {
    [
        tone(TxComponent::Sync, 1200, 9_000_000_000),
        tone(TxComponent::Porch, 1500, 3_000_000_000),
        pixels(
            TxComponent::Luminance,
            ScanChannel::Luminance,
            0,
            88_000_000_000,
        ),
        tone(TxComponent::ChrominanceSelector, selector_hz, 4_500_000_000),
        tone(TxComponent::Porch, 1900, 1_500_000_000),
        pixels(chroma_component, chroma, 0, 44_000_000_000),
    ]
}

const fn pd(component_ps: u64) -> [ScanSegment; 6] {
    [
        tone(TxComponent::Sync, 1200, 20_000_000_000),
        tone(TxComponent::Porch, 1500, 2_080_000_000),
        pixels(
            TxComponent::Luminance,
            ScanChannel::Luminance,
            0,
            component_ps,
        ),
        pixels(
            TxComponent::RedDifference,
            ScanChannel::RedDifference,
            0,
            component_ps,
        ),
        pixels(
            TxComponent::BlueDifference,
            ScanChannel::BlueDifference,
            0,
            component_ps,
        ),
        pixels(
            TxComponent::Luminance,
            ScanChannel::Luminance,
            1,
            component_ps,
        ),
    ]
}

const fn wide(primary: &'static [ScanSegment]) -> RasterScan {
    RasterScan {
        leading: &[],
        primary,
        alternate: &[],
    }
}

const SCOTTIE_LEADING: [ScanSegment; 1] = [tone(TxComponent::Sync, 1200, 9_000_000_000)];

const fn scottie_scan(primary: &'static [ScanSegment]) -> RasterScan {
    RasterScan {
        leading: &SCOTTIE_LEADING,
        primary,
        alternate: &[],
    }
}

const MARTIN1_UNIT: [ScanSegment; 8] = martin(146_432_000_000);
const MARTIN2_UNIT: [ScanSegment; 8] = martin(73_216_000_000);
const SCOTTIE1_UNIT: [ScanSegment; 7] = scottie(138_240_000_000);
const SCOTTIE2_UNIT: [ScanSegment; 7] = scottie(88_064_000_000);
const SCOTTIE_DX_UNIT: [ScanSegment; 7] = scottie(345_600_000_000);
const ROBOT36_UNIT: [ScanSegment; 6] =
    robot36(1500, TxComponent::RedDifference, ScanChannel::RedDifference);
const ROBOT36_ALTERNATE: [ScanSegment; 6] = robot36(
    2300,
    TxComponent::BlueDifference,
    ScanChannel::BlueDifference,
);
const ROBOT72_UNIT: [ScanSegment; 9] = [
    tone(TxComponent::Sync, 1200, 9_000_000_000),
    tone(TxComponent::Porch, 1500, 3_000_000_000),
    pixels(
        TxComponent::Luminance,
        ScanChannel::Luminance,
        0,
        138_000_000_000,
    ),
    tone(TxComponent::Porch, 1500, 4_500_000_000),
    tone(TxComponent::Porch, 1900, 1_500_000_000),
    pixels(
        TxComponent::RedDifference,
        ScanChannel::RedDifference,
        0,
        69_000_000_000,
    ),
    tone(TxComponent::Porch, 2300, 4_500_000_000),
    tone(TxComponent::Porch, 1900, 1_500_000_000),
    pixels(
        TxComponent::BlueDifference,
        ScanChannel::BlueDifference,
        0,
        69_000_000_000,
    ),
];
const PD50_UNIT: [ScanSegment; 6] = pd(91_520_000_000);
const PD90_UNIT: [ScanSegment; 6] = pd(170_240_000_000);
const PD120_UNIT: [ScanSegment; 6] = pd(121_600_000_000);
const PD160_UNIT: [ScanSegment; 6] = pd(195_584_000_000);
const PD180_UNIT: [ScanSegment; 6] = pd(183_040_000_000);
const PD240_UNIT: [ScanSegment; 6] = pd(244_480_000_000);
const PD290_UNIT: [ScanSegment; 6] = pd(228_800_000_000);

const UNSUPPORTED: RasterScan = wide(&[]);
const MARTIN1: RasterScan = wide(&MARTIN1_UNIT);
const MARTIN2: RasterScan = wide(&MARTIN2_UNIT);
const SCOTTIE1: RasterScan = scottie_scan(&SCOTTIE1_UNIT);
const SCOTTIE2: RasterScan = scottie_scan(&SCOTTIE2_UNIT);
const SCOTTIE_DX: RasterScan = scottie_scan(&SCOTTIE_DX_UNIT);
const ROBOT36: RasterScan = RasterScan {
    leading: &[],
    primary: &ROBOT36_UNIT,
    alternate: &ROBOT36_ALTERNATE,
};
const ROBOT72: RasterScan = wide(&ROBOT72_UNIT);
const PD50: RasterScan = wide(&PD50_UNIT);
const PD90: RasterScan = wide(&PD90_UNIT);
const PD120: RasterScan = wide(&PD120_UNIT);
const PD160: RasterScan = wide(&PD160_UNIT);
const PD180: RasterScan = wide(&PD180_UNIT);
const PD240: RasterScan = wide(&PD240_UNIT);
const PD290: RasterScan = wide(&PD290_UNIT);

pub(super) const fn for_mode(mode: Mode) -> RasterScan {
    match mode {
        Mode::Martin1 => MARTIN1,
        Mode::Martin2 => MARTIN2,
        Mode::Scottie1 => SCOTTIE1,
        Mode::Scottie2 => SCOTTIE2,
        Mode::ScottieDx => SCOTTIE_DX,
        Mode::Robot36 => ROBOT36,
        Mode::Robot72 => ROBOT72,
        Mode::Pd50 => PD50,
        Mode::Pd90 => PD90,
        Mode::Pd120 => PD120,
        Mode::Pd160 => PD160,
        Mode::Pd180 => PD180,
        Mode::Pd240 => PD240,
        Mode::Pd290 => PD290,
        _ => UNSUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use rstest::rstest;

    use super::*;
    use crate::mode::Support;

    #[test]
    fn every_supported_mode_has_a_raster_scan() {
        for mode in Mode::ALL {
            let supported = mode.spec().encode_support() == Support::Supported;
            assert_eq!(!mode.scan().is_empty(), supported, "{mode:?}");
        }
    }

    #[test]
    fn raster_unit_duration_matches_the_mode_period() {
        for mode in Mode::ALL.into_iter().filter(|mode| !mode.scan().is_empty()) {
            for unit in [0, 1] {
                assert_eq!(
                    mode.scan().duration(unit),
                    Some(mode.spec().period()),
                    "{mode:?} unit {unit}"
                );
            }
        }
    }

    #[rstest]
    #[case(Mode::Martin1, 2_431_000_000)]
    #[case(Mode::Martin2, 2_431_000_000)]
    #[case(Mode::Scottie1, 283_980_000_000)]
    #[case(Mode::Scottie2, 183_628_000_000)]
    #[case(Mode::ScottieDx, 698_700_000_000)]
    #[case(Mode::Robot36, 4_500_000_000)]
    #[case(Mode::Robot72, 4_500_000_000)]
    #[case(Mode::Pd50, 10_000_000_000)]
    #[case(Mode::Pd290, 10_000_000_000)]
    fn sync_center_matches_source_timing(#[case] mode: Mode, #[case] expected_ps: u64) {
        for unit in [0, 1] {
            assert_eq!(
                mode.scan().sync_center(unit).map(SstvDuration::as_picos),
                Some(expected_ps)
            );
        }
    }

    #[test]
    fn only_scottie_sends_leading_segments() {
        for mode in Mode::ALL {
            let expected = matches!(mode, Mode::Scottie1 | Mode::Scottie2 | Mode::ScottieDx);
            assert_eq!(!mode.scan().leading().is_empty(), expected, "{mode:?}");
        }
    }

    #[test]
    fn alternating_modes_keep_identical_segment_geometry() {
        let scan = Mode::Robot36.scan();
        let primary: Vec<_> = scan
            .offsets(0)
            .map(|(offset, segment)| (offset, segment.duration()))
            .collect();
        let alternate: Vec<_> = scan
            .offsets(1)
            .map(|(offset, segment)| (offset, segment.duration()))
            .collect();
        assert_eq!(primary, alternate);
    }
}
