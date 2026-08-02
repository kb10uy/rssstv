//! Streaming encoders for the conventional-VIS SSTV modes supported by RSSSTV.

use crate::SstvError;
use crate::color::{LevelFrequencyBand, rgb_to_y_cr_cb};
use crate::image::{ImageSize, Rgb8, RgbImage};
use crate::mode::{Mode, ModeFamily, Support};
use crate::signal::{Frequency, TimedTone, Tone, TxComponent};
use crate::time::TxInstant;

const PS_PER_MS: u64 = 1_000_000_000;
const VIS_END_PS: u64 = 910 * PS_PER_MS;

#[derive(Clone, Copy)]
enum Family {
    Martin { component_ps: u64 },
    Scottie { component_ps: u64 },
    Robot36,
    Robot72,
    Pd { component_ps: u64 },
}

#[derive(Clone, Copy)]
enum Segment {
    Fixed(TxComponent, u32, u64),
    Pixels(TxComponent, PixelSource, u64),
}

#[derive(Clone, Copy)]
enum PixelSource {
    Red,
    Green,
    Blue,
    Y(usize),
    Cr(usize),
    Cb(usize),
}

/// A bounded, pull-based encoder for one owned SSTV image.
///
/// Iteration emits conventional VIS framing, Scottie's initial synchronization
/// tone when applicable, and the image raster. It does not emit VOX framing,
/// footers, or station identification.
pub struct TxEncoder {
    mode: Mode,
    family: Family,
    image: RgbImage,
    deadline_ps: u64,
    vis_index: u8,
    initial_sync_pending: bool,
    line: usize,
    segment: u8,
    pixel: usize,
    segment_start_ps: u64,
}

impl TxEncoder {
    /// Validates `image` against `mode` and constructs a streaming encoder.
    pub fn new(mode: Mode, image: RgbImage) -> Result<Self, SstvError> {
        let spec = mode.spec();
        if spec.encode_support() != Support::Supported
            || !matches!(
                spec.family(),
                ModeFamily::Martin | ModeFamily::Scottie | ModeFamily::Robot | ModeFamily::Pd
            )
            || matches!(mode, Mode::Robot24 | Mode::Bw8 | Mode::Bw12)
        {
            return Err(SstvError::UnsupportedTxMode(mode));
        }
        let expected = ImageSize::new(spec.width() as usize, spec.height() as usize)
            .expect("mode dimensions are valid");
        if image.size() != expected {
            return Err(SstvError::TxImageSizeMismatch {
                expected,
                actual: image.size(),
            });
        }
        let family = match mode {
            Mode::Martin1 => Family::Martin {
                component_ps: 146_432_000_000,
            },
            Mode::Martin2 => Family::Martin {
                component_ps: 73_216_000_000,
            },
            Mode::Scottie1 => Family::Scottie {
                component_ps: 138_240_000_000,
            },
            Mode::Scottie2 => Family::Scottie {
                component_ps: 88_064_000_000,
            },
            Mode::ScottieDx => Family::Scottie {
                component_ps: 345_600_000_000,
            },
            Mode::Robot36 => Family::Robot36,
            Mode::Robot72 => Family::Robot72,
            Mode::Pd50 => Family::Pd {
                component_ps: 91_520_000_000,
            },
            Mode::Pd90 => Family::Pd {
                component_ps: 170_240_000_000,
            },
            Mode::Pd120 => Family::Pd {
                component_ps: 121_600_000_000,
            },
            Mode::Pd160 => Family::Pd {
                component_ps: 195_584_000_000,
            },
            Mode::Pd180 => Family::Pd {
                component_ps: 183_040_000_000,
            },
            Mode::Pd240 => Family::Pd {
                component_ps: 244_480_000_000,
            },
            Mode::Pd290 => Family::Pd {
                component_ps: 228_800_000_000,
            },
            _ => return Err(SstvError::UnsupportedTxMode(mode)),
        };
        Ok(Self {
            mode,
            family,
            image,
            deadline_ps: 0,
            vis_index: 0,
            initial_sync_pending: matches!(family, Family::Scottie { .. }),
            line: 0,
            segment: 0,
            pixel: 0,
            segment_start_ps: VIS_END_PS,
        })
    }

    fn emit_fixed(&mut self, component: TxComponent, hz: u32, duration_ps: u64) -> TimedTone {
        self.deadline_ps += duration_ps;
        TimedTone::new(
            component,
            Tone::new(Frequency::from_hz(hz)),
            TxInstant::from_picos(self.deadline_ps),
        )
    }

    fn next_vis(&mut self) -> Option<TimedTone> {
        if self.vis_index >= 13 {
            return None;
        }
        let raw = self.mode.spec().raw_vis().expect("supported modes use VIS");
        let (component, hz, duration_ms) = match self.vis_index {
            0 => (TxComponent::Leader, 1900, 300),
            1 => (TxComponent::Leader, 1200, 10),
            2 => (TxComponent::Leader, 1900, 300),
            3 => (TxComponent::Identification, 1200, 30),
            4..=11 => {
                let bit = self.vis_index - 4;
                (
                    TxComponent::Identification,
                    if raw & (1 << bit) != 0 { 1100 } else { 1300 },
                    30,
                )
            }
            12 => (TxComponent::Identification, 1200, 30),
            _ => unreachable!(),
        };
        self.vis_index += 1;
        Some(self.emit_fixed(component, hz, duration_ms * PS_PER_MS))
    }

    fn line_count(&self) -> usize {
        let rows = self.mode.spec().active_rows() as usize;
        if matches!(self.family, Family::Pd { .. }) {
            rows / 2
        } else {
            rows
        }
    }

    fn segment(&self) -> Option<Segment> {
        match self.family {
            Family::Martin { component_ps } => match self.segment {
                0 => Some(Segment::Fixed(TxComponent::Sync, 1200, 4_862_000_000)),
                1 => Some(Segment::Fixed(TxComponent::Porch, 1500, 572_000_000)),
                2 => Some(Segment::Pixels(
                    TxComponent::Green,
                    PixelSource::Green,
                    component_ps,
                )),
                3 => Some(Segment::Fixed(TxComponent::Porch, 1500, 572_000_000)),
                4 => Some(Segment::Pixels(
                    TxComponent::Blue,
                    PixelSource::Blue,
                    component_ps,
                )),
                5 => Some(Segment::Fixed(TxComponent::Porch, 1500, 572_000_000)),
                6 => Some(Segment::Pixels(
                    TxComponent::Red,
                    PixelSource::Red,
                    component_ps,
                )),
                7 => Some(Segment::Fixed(TxComponent::Porch, 1500, 572_000_000)),
                _ => None,
            },
            Family::Scottie { component_ps } => match self.segment {
                0 => Some(Segment::Fixed(TxComponent::Porch, 1500, 1_500_000_000)),
                1 => Some(Segment::Pixels(
                    TxComponent::Green,
                    PixelSource::Green,
                    component_ps,
                )),
                2 => Some(Segment::Fixed(TxComponent::Porch, 1500, 1_500_000_000)),
                3 => Some(Segment::Pixels(
                    TxComponent::Blue,
                    PixelSource::Blue,
                    component_ps,
                )),
                4 => Some(Segment::Fixed(TxComponent::Sync, 1200, 9_000_000_000)),
                5 => Some(Segment::Fixed(TxComponent::Porch, 1500, 1_500_000_000)),
                6 => Some(Segment::Pixels(
                    TxComponent::Red,
                    PixelSource::Red,
                    component_ps,
                )),
                _ => None,
            },
            Family::Robot36 => match self.segment {
                0 => Some(Segment::Fixed(TxComponent::Sync, 1200, 9_000_000_000)),
                1 => Some(Segment::Fixed(TxComponent::Porch, 1500, 3_000_000_000)),
                2 => Some(Segment::Pixels(
                    TxComponent::Luminance,
                    PixelSource::Y(0),
                    88_000_000_000,
                )),
                3 => Some(Segment::Fixed(
                    TxComponent::ChrominanceSelector,
                    if self.line & 1 == 0 { 1500 } else { 2300 },
                    4_500_000_000,
                )),
                4 => Some(Segment::Fixed(TxComponent::Porch, 1900, 1_500_000_000)),
                5 if self.line & 1 == 0 => Some(Segment::Pixels(
                    TxComponent::RedDifference,
                    PixelSource::Cr(0),
                    44_000_000_000,
                )),
                5 => Some(Segment::Pixels(
                    TxComponent::BlueDifference,
                    PixelSource::Cb(0),
                    44_000_000_000,
                )),
                _ => None,
            },
            Family::Robot72 => match self.segment {
                0 => Some(Segment::Fixed(TxComponent::Sync, 1200, 9_000_000_000)),
                1 => Some(Segment::Fixed(TxComponent::Porch, 1500, 3_000_000_000)),
                2 => Some(Segment::Pixels(
                    TxComponent::Luminance,
                    PixelSource::Y(0),
                    138_000_000_000,
                )),
                3 => Some(Segment::Fixed(TxComponent::Porch, 1500, 4_500_000_000)),
                4 => Some(Segment::Fixed(TxComponent::Porch, 1900, 1_500_000_000)),
                5 => Some(Segment::Pixels(
                    TxComponent::RedDifference,
                    PixelSource::Cr(0),
                    69_000_000_000,
                )),
                6 => Some(Segment::Fixed(TxComponent::Porch, 2300, 4_500_000_000)),
                7 => Some(Segment::Fixed(TxComponent::Porch, 1900, 1_500_000_000)),
                8 => Some(Segment::Pixels(
                    TxComponent::BlueDifference,
                    PixelSource::Cb(0),
                    69_000_000_000,
                )),
                _ => None,
            },
            Family::Pd { component_ps } => match self.segment {
                0 => Some(Segment::Fixed(TxComponent::Sync, 1200, 20_000_000_000)),
                1 => Some(Segment::Fixed(TxComponent::Porch, 1500, 2_080_000_000)),
                2 => Some(Segment::Pixels(
                    TxComponent::Luminance,
                    PixelSource::Y(0),
                    component_ps,
                )),
                3 => Some(Segment::Pixels(
                    TxComponent::RedDifference,
                    PixelSource::Cr(0),
                    component_ps,
                )),
                4 => Some(Segment::Pixels(
                    TxComponent::BlueDifference,
                    PixelSource::Cb(0),
                    component_ps,
                )),
                5 => Some(Segment::Pixels(
                    TxComponent::Luminance,
                    PixelSource::Y(1),
                    component_ps,
                )),
                _ => None,
            },
        }
    }

    fn pixel(&self, source: PixelSource, x: usize) -> Rgb8 {
        let row = self.line
            * if matches!(self.family, Family::Pd { .. }) {
                2
            } else {
                1
            }
            + match source {
                PixelSource::Y(offset) | PixelSource::Cr(offset) | PixelSource::Cb(offset) => {
                    offset
                }
                _ => 0,
            };
        self.image.get(x, row).expect("validated image coordinates")
    }

    fn pixel_level(&self, source: PixelSource, x: usize) -> u8 {
        let pixel = self.pixel(source, x);
        match source {
            PixelSource::Red => pixel.r,
            PixelSource::Green => pixel.g,
            PixelSource::Blue => pixel.b,
            PixelSource::Y(_) => rgb_to_y_cr_cb(pixel).y,
            PixelSource::Cr(_) => rgb_to_y_cr_cb(pixel).cr,
            PixelSource::Cb(_) => rgb_to_y_cr_cb(pixel).cb,
        }
    }
}

impl Iterator for TxEncoder {
    type Item = TimedTone;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(tone) = self.next_vis() {
            return Some(tone);
        }
        if self.initial_sync_pending {
            self.initial_sync_pending = false;
            self.segment_start_ps += 9_000_000_000;
            return Some(self.emit_fixed(TxComponent::Sync, 1200, 9_000_000_000));
        }
        loop {
            if self.line >= self.line_count() {
                return None;
            }
            let Some(segment) = self.segment() else {
                self.line += 1;
                self.segment = 0;
                self.pixel = 0;
                continue;
            };
            match segment {
                Segment::Fixed(component, hz, duration_ps) => {
                    self.segment += 1;
                    self.segment_start_ps += duration_ps;
                    return Some(self.emit_fixed(component, hz, duration_ps));
                }
                Segment::Pixels(component, source, duration_ps) => {
                    let width = self.image.size().width();
                    let x = self.pixel;
                    self.pixel += 1;
                    self.deadline_ps =
                        self.segment_start_ps + duration_ps * self.pixel as u64 / width as u64;
                    let tone = TimedTone::new(
                        component,
                        Tone::new(
                            LevelFrequencyBand::Wide
                                .level_to_frequency(self.pixel_level(source, x)),
                        ),
                        TxInstant::from_picos(self.deadline_ps),
                    );
                    if self.pixel == width {
                        self.pixel = 0;
                        self.segment += 1;
                        self.segment_start_ps += duration_ps;
                    }
                    return Some(tone);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use rstest::rstest;

    use super::*;

    fn image(mode: Mode, fill: Rgb8) -> RgbImage {
        let spec = mode.spec();
        RgbImage::new(
            ImageSize::new(spec.width() as usize, spec.height() as usize).unwrap(),
            fill,
        )
    }

    #[test]
    fn vis_is_exact_and_lsb_first() {
        let tones: Vec<_> = TxEncoder::new(Mode::Martin1, image(Mode::Martin1, Rgb8::default()))
            .unwrap()
            .take(13)
            .collect();
        assert_eq!(tones.len(), 13);
        assert_eq!(tones[0].tone().frequency().as_hz(), 1900);
        assert_eq!(tones[1].until().as_picos(), 310_000_000_000);
        let bits: Vec<_> = tones[4..12]
            .iter()
            .map(|tone| tone.tone().frequency().as_hz())
            .collect();
        assert_eq!(bits, [1300, 1300, 1100, 1100, 1300, 1100, 1300, 1100]);
        assert_eq!(tones[12].until().as_picos(), VIS_END_PS);
    }

    #[rstest]
    #[case(Mode::Robot36, 0x88)]
    #[case(Mode::Robot72, 0x0c)]
    #[case(Mode::Scottie1, 0x3c)]
    #[case(Mode::Scottie2, 0xb8)]
    #[case(Mode::ScottieDx, 0xcc)]
    #[case(Mode::Martin1, 0xac)]
    #[case(Mode::Martin2, 0x28)]
    #[case(Mode::Pd50, 0xdd)]
    #[case(Mode::Pd90, 0x63)]
    #[case(Mode::Pd120, 0x5f)]
    #[case(Mode::Pd160, 0xe2)]
    #[case(Mode::Pd180, 0x60)]
    #[case(Mode::Pd240, 0xe1)]
    #[case(Mode::Pd290, 0xde)]
    fn supported_modes_transmit_source_raw_vis(#[case] mode: Mode, #[case] expected: u8) {
        let raw = TxEncoder::new(mode, image(mode, Rgb8::default()))
            .unwrap()
            .skip(4)
            .take(8)
            .enumerate()
            .fold(0, |raw, (bit, tone)| {
                raw | u8::from(tone.tone().frequency().as_hz() == 1100) << bit
            });
        assert_eq!(raw, expected);
    }

    #[rstest]
    #[case(Mode::Martin1)]
    #[case(Mode::Martin2)]
    #[case(Mode::Scottie1)]
    #[case(Mode::Scottie2)]
    #[case(Mode::ScottieDx)]
    #[case(Mode::Robot36)]
    #[case(Mode::Robot72)]
    #[case(Mode::Pd50)]
    #[case(Mode::Pd90)]
    #[case(Mode::Pd120)]
    #[case(Mode::Pd160)]
    #[case(Mode::Pd180)]
    #[case(Mode::Pd240)]
    #[case(Mode::Pd290)]
    fn all_supported_modes_construct(#[case] mode: Mode) {
        assert!(TxEncoder::new(mode, image(mode, Rgb8::default())).is_ok());
    }

    #[test]
    fn constructor_reports_mode_and_size_errors() {
        assert!(matches!(
            TxEncoder::new(Mode::Avt90, image(Mode::Avt90, Rgb8::default())),
            Err(SstvError::UnsupportedTxMode(Mode::Avt90))
        ));
        let wrong = RgbImage::new(ImageSize::new(1, 1).unwrap(), Rgb8::default());
        assert!(matches!(
            TxEncoder::new(Mode::Martin1, wrong),
            Err(SstvError::TxImageSizeMismatch { .. })
        ));
    }

    #[rstest]
    #[case(Mode::Martin1, 0, 446_446_000_000)]
    #[case(Mode::Martin2, 0, 226_798_000_000)]
    #[case(Mode::Scottie1, 9_000_000_000, 428_220_000_000)]
    #[case(Mode::Scottie2, 9_000_000_000, 277_692_000_000)]
    #[case(Mode::ScottieDx, 9_000_000_000, 1_050_300_000_000)]
    #[case(Mode::Robot36, 0, 150_000_000_000)]
    #[case(Mode::Robot72, 0, 300_000_000_000)]
    #[case(Mode::Pd50, 0, 388_160_000_000)]
    #[case(Mode::Pd90, 0, 703_040_000_000)]
    #[case(Mode::Pd120, 0, 508_480_000_000)]
    #[case(Mode::Pd160, 0, 804_416_000_000)]
    #[case(Mode::Pd180, 0, 754_240_000_000)]
    #[case(Mode::Pd240, 0, 1_000_000_000_000)]
    #[case(Mode::Pd290, 0, 937_280_000_000)]
    fn first_line_has_exact_end(
        #[case] mode: Mode,
        #[case] initial_ps: u64,
        #[case] period_ps: u64,
    ) {
        let width = mode.spec().width() as usize;
        let line_events = match mode.spec().family() {
            ModeFamily::Martin => 5 + 3 * width,
            ModeFamily::Scottie => 4 + 3 * width,
            ModeFamily::Robot if mode == Mode::Robot36 => 4 + 2 * width,
            ModeFamily::Robot => 6 + 3 * width,
            ModeFamily::Pd => 2 + 4 * width,
            _ => unreachable!(),
        };
        let last = TxEncoder::new(mode, image(mode, Rgb8::default()))
            .unwrap()
            .nth(13 + usize::from(initial_ps != 0) + line_events - 1)
            .unwrap();
        assert_eq!(last.until().as_picos(), VIS_END_PS + initial_ps + period_ps);
    }

    #[test]
    fn family_component_orders_match_source() {
        let martin: Vec<_> = TxEncoder::new(Mode::Martin1, image(Mode::Martin1, Rgb8::default()))
            .unwrap()
            .skip(13)
            .map(TimedTone::component)
            .take(323)
            .collect();
        assert_eq!(
            martin[0..3],
            [TxComponent::Sync, TxComponent::Porch, TxComponent::Green]
        );
        assert_eq!(martin[322], TxComponent::Porch);

        let scottie: Vec<_> =
            TxEncoder::new(Mode::Scottie1, image(Mode::Scottie1, Rgb8::default()))
                .unwrap()
                .skip(13)
                .map(TimedTone::component)
                .take(645)
                .collect();
        assert_eq!(scottie[0], TxComponent::Sync);
        assert_eq!(scottie[1], TxComponent::Porch);
        assert_eq!(scottie[2], TxComponent::Green);
        assert_eq!(scottie[322], TxComponent::Porch);
        assert_eq!(scottie[323], TxComponent::Blue);
        assert_eq!(scottie[643], TxComponent::Sync);
        assert_eq!(scottie[644], TxComponent::Porch);

        let robot72: Vec<_> = TxEncoder::new(Mode::Robot72, image(Mode::Robot72, Rgb8::default()))
            .unwrap()
            .skip(13)
            .map(TimedTone::component)
            .take(649)
            .collect();
        assert_eq!(
            robot72[0..3],
            [
                TxComponent::Sync,
                TxComponent::Porch,
                TxComponent::Luminance
            ]
        );
        assert_eq!(
            robot72[322..325],
            [
                TxComponent::Porch,
                TxComponent::Porch,
                TxComponent::RedDifference
            ]
        );
        assert_eq!(
            robot72[644..647],
            [
                TxComponent::Porch,
                TxComponent::Porch,
                TxComponent::BlueDifference
            ]
        );

        let pd: Vec<_> = TxEncoder::new(Mode::Pd50, image(Mode::Pd50, Rgb8::default()))
            .unwrap()
            .skip(13)
            .map(TimedTone::component)
            .take(963)
            .collect();
        assert_eq!(
            pd[0..3],
            [
                TxComponent::Sync,
                TxComponent::Porch,
                TxComponent::Luminance
            ]
        );
        assert_eq!(pd[322], TxComponent::RedDifference);
        assert_eq!(pd[642], TxComponent::BlueDifference);
        assert_eq!(pd[962], TxComponent::Luminance);
    }

    #[test]
    fn robot36_alternates_tcs_and_chroma() {
        let width = 320;
        let line_events = 4 + 2 * width;
        let tones: Vec<_> = TxEncoder::new(Mode::Robot36, image(Mode::Robot36, Rgb8::default()))
            .unwrap()
            .skip(13)
            .take(line_events * 2)
            .collect();
        assert_eq!(tones[width + 2].tone().frequency().as_hz(), 1500);
        assert_eq!(tones[width + 4].component(), TxComponent::RedDifference);
        assert_eq!(
            tones[line_events + width + 2].tone().frequency().as_hz(),
            2300
        );
        assert_eq!(
            tones[line_events + width + 4].component(),
            TxComponent::BlueDifference
        );
    }

    #[test]
    fn pd_chroma_uses_first_row_and_second_luminance_uses_second() {
        let mut image = image(Mode::Pd50, Rgb8::new(255, 0, 0));
        for pixel in &mut image.pixels_mut()[320..640] {
            *pixel = Rgb8::new(0, 0, 255);
        }
        let tones: Vec<_> = TxEncoder::new(Mode::Pd50, image)
            .unwrap()
            .skip(13 + 2)
            .take(4 * 320)
            .collect();
        assert_eq!(tones[320].component(), TxComponent::RedDifference);
        assert_eq!(tones[320].tone().frequency().as_hz(), 2246);
        assert_eq!(tones[640].component(), TxComponent::BlueDifference);
        assert_eq!(tones[640].tone().frequency().as_hz(), 1781);
        assert_eq!(tones[960].component(), TxComponent::Luminance);
        assert_eq!(tones[960].tone().frequency().as_hz(), 1625);
    }

    #[test]
    fn deadlines_are_strictly_monotonic() {
        let mut previous = TxInstant::ZERO;
        for tone in TxEncoder::new(Mode::Pd50, image(Mode::Pd50, Rgb8::default())).unwrap() {
            assert!(tone.until() > previous);
            previous = tone.until();
        }
    }

    #[rstest]
    #[case(Mode::Martin1)]
    #[case(Mode::Martin2)]
    #[case(Mode::Scottie1)]
    #[case(Mode::Scottie2)]
    #[case(Mode::ScottieDx)]
    #[case(Mode::Robot36)]
    #[case(Mode::Robot72)]
    #[case(Mode::Pd50)]
    #[case(Mode::Pd90)]
    #[case(Mode::Pd120)]
    #[case(Mode::Pd160)]
    #[case(Mode::Pd180)]
    #[case(Mode::Pd240)]
    #[case(Mode::Pd290)]
    fn final_body_duration_is_exact(#[case] mode: Mode) {
        let initial = if mode.spec().family() == ModeFamily::Scottie {
            9_000_000_000
        } else {
            0
        };
        let units = mode.spec().active_rows() as u64 / mode.spec().rows_per_raster_unit() as u64;
        let expected = VIS_END_PS + initial + units * mode.spec().period().as_picos();
        let last = TxEncoder::new(mode, image(mode, Rgb8::default()))
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(last.until().as_picos(), expected);
    }

    #[test]
    fn cumulative_pixel_boundaries_end_at_component_duration() {
        let tones: Vec<_> = TxEncoder::new(Mode::Martin1, image(Mode::Martin1, Rgb8::default()))
            .unwrap()
            .skip(15)
            .take(320)
            .collect();
        assert_eq!(
            tones[0].until().as_picos(),
            VIS_END_PS + 5_434_000_000 + 457_600_000
        );
        assert_eq!(tones[319].until().as_picos(), VIS_END_PS + 151_866_000_000);
    }
}
