//! The encoder for one image: conventional VIS framing and the raster.

use crate::{
    SstvError,
    color::rgb_to_y_cr_cb,
    image::{ImageSize, RgbImage},
    mode::{Mode, ScanChannel, ScanContent, Support},
    signal::{Frequency, LEADER_HZ, SYNC_HZ, TimedTone, TxComponent, VIS_MARK_HZ, VIS_SPACE_HZ},
    time::TxInstant,
    tx::{PS_PER_MS, VIS_END_PS},
};

/// A bounded, pull-based encoder for one owned SSTV image.
///
/// Iteration emits conventional VIS framing, the mode's leading segments when
/// applicable, and the image raster described by [`Mode::scan`]. It does not
/// emit VOX framing, footers, or station identification. Image rows beyond the
/// mode's active row count are not transmitted.
#[derive(Debug)]
pub struct TxEncoder {
    mode: Mode,
    image: RgbImage,
    deadline_ps: u64,
    vis_index: u8,
    leading: usize,
    unit: usize,
    segment: usize,
    pixel: usize,
    segment_start_ps: u64,
}

impl TxEncoder {
    /// Validates `image` against `mode` and constructs a streaming encoder.
    pub fn new(mode: Mode, image: RgbImage) -> Result<Self, SstvError> {
        let spec = mode.spec();
        if spec.encode_support() != Support::Supported
            || mode.scan().is_empty()
            || spec.raw_vis().is_none()
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
        Ok(Self {
            mode,
            image,
            deadline_ps: 0,
            vis_index: 0,
            leading: 0,
            unit: 0,
            segment: 0,
            pixel: 0,
            segment_start_ps: VIS_END_PS,
        })
    }

    /// Returns the mode the image is being encoded in.
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    fn emit(
        &mut self,
        component: TxComponent,
        frequency: Frequency,
        duration_ps: u64,
    ) -> TimedTone {
        self.deadline_ps += duration_ps;
        TimedTone::new(
            component,
            frequency,
            TxInstant::from_picos(self.deadline_ps),
        )
    }

    fn next_vis(&mut self) -> Option<TimedTone> {
        if self.vis_index >= 13 {
            return None;
        }
        let raw = self.mode.spec().raw_vis().expect("supported modes use VIS");
        let (component, hz, duration_ms) = match self.vis_index {
            0 => (TxComponent::Leader, LEADER_HZ, 300),
            1 => (TxComponent::Leader, SYNC_HZ, 10),
            2 => (TxComponent::Leader, LEADER_HZ, 300),
            3 => (TxComponent::Identification, SYNC_HZ, 30),
            4..=11 => {
                let bit = self.vis_index - 4;
                (
                    TxComponent::Identification,
                    if raw & (1 << bit) != 0 {
                        VIS_MARK_HZ
                    } else {
                        VIS_SPACE_HZ
                    },
                    30,
                )
            }
            12 => (TxComponent::Identification, SYNC_HZ, 30),
            _ => unreachable!(),
        };
        self.vis_index += 1;
        Some(self.emit(component, Frequency::from_hz(hz), duration_ms * PS_PER_MS))
    }

    fn raster_units(&self) -> usize {
        let spec = self.mode.spec();
        spec.active_rows() as usize / spec.rows_per_raster_unit() as usize
    }

    fn pixel_level(&self, channel: ScanChannel, row_offset: u8, x: usize) -> u8 {
        let row =
            self.unit * self.mode.spec().rows_per_raster_unit() as usize + row_offset as usize;
        let pixel = self.image.get(x, row).expect("validated image coordinates");
        match channel {
            ScanChannel::Red => pixel.r,
            ScanChannel::Green => pixel.g,
            ScanChannel::Blue => pixel.b,
            ScanChannel::Luminance => rgb_to_y_cr_cb(pixel).y,
            ScanChannel::RedDifference => rgb_to_y_cr_cb(pixel).cr,
            ScanChannel::BlueDifference => rgb_to_y_cr_cb(pixel).cb,
        }
    }
}

impl Iterator for TxEncoder {
    type Item = TimedTone;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(tone) = self.next_vis() {
            return Some(tone);
        }
        let scan = self.mode.scan();
        if let Some(segment) = scan.leading().get(self.leading).copied() {
            self.leading += 1;
            if let ScanContent::Tone(frequency) = segment.content() {
                let duration_ps = segment.duration().as_picos();
                self.segment_start_ps += duration_ps;
                return Some(self.emit(segment.component(), frequency, duration_ps));
            }
        }
        loop {
            if self.unit >= self.raster_units() {
                return None;
            }
            let Some(segment) = scan.unit(self.unit).get(self.segment).copied() else {
                self.unit += 1;
                self.segment = 0;
                self.pixel = 0;
                continue;
            };
            let duration_ps = segment.duration().as_picos();
            match segment.content() {
                ScanContent::Tone(frequency) => {
                    self.segment += 1;
                    self.segment_start_ps += duration_ps;
                    return Some(self.emit(segment.component(), frequency, duration_ps));
                }
                ScanContent::Pixels {
                    channel,
                    row_offset,
                } => {
                    let width = self.image.size().width();
                    let x = self.pixel;
                    self.pixel += 1;
                    self.deadline_ps =
                        self.segment_start_ps + duration_ps * self.pixel as u64 / width as u64;
                    let level = self.pixel_level(channel, row_offset, x);
                    let tone = TimedTone::new(
                        segment.component(),
                        self.mode.spec().signal_band().level_to_frequency(level),
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
    use crate::{image::Rgb8, mode::ModeFamily, tx::test_image as image};

    #[test]
    fn vis_is_exact_and_lsb_first() {
        let tones: Vec<_> = TxEncoder::new(Mode::Martin1, image(Mode::Martin1, Rgb8::default()))
            .unwrap()
            .take(13)
            .collect();
        assert_eq!(tones.len(), 13);
        assert_eq!(tones[0].frequency().as_hz(), 1900);
        assert_eq!(tones[1].until().as_picos(), 310_000_000_000);
        let bits: Vec<_> = tones[4..12]
            .iter()
            .map(|tone| tone.frequency().as_hz())
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
                raw | u8::from(tone.frequency().as_hz() == 1100) << bit
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
        assert_eq!(tones[width + 2].frequency().as_hz(), 1500);
        assert_eq!(tones[width + 4].component(), TxComponent::RedDifference);
        assert_eq!(tones[line_events + width + 2].frequency().as_hz(), 2300);
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
        assert_eq!(tones[320].frequency().as_hz(), 2246);
        assert_eq!(tones[640].component(), TxComponent::BlueDifference);
        assert_eq!(tones[640].frequency().as_hz(), 1781);
        assert_eq!(tones[960].component(), TxComponent::Luminance);
        assert_eq!(tones[960].frequency().as_hz(), 1625);
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
