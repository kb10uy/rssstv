//! Streaming encoders for the conventional-VIS SSTV modes supported by RSSSTV.

use crate::{
    SstvError,
    color::rgb_to_y_cr_cb,
    image::{ImageSize, RgbImage},
    mode::{Mode, ScanChannel, ScanContent, Support},
    signal::{Frequency, TimedTone, TxComponent},
    time::{SstvDuration, TxInstant},
};
use rssstv_fskid::{FskEncoder, FskId};

const PS_PER_MS: u64 = 1_000_000_000;
const VIS_END_PS: u64 = 910 * PS_PER_MS;
const VOX_DURATION_PS: u64 = 800 * PS_PER_MS;

const VOX: [(u32, u64); 8] = [
    (1900, 100),
    (1500, 100),
    (1900, 100),
    (1500, 100),
    (2300, 100),
    (1500, 100),
    (2300, 100),
    (1500, 100),
];

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
        Some(self.emit(component, Frequency::from_hz(hz), duration_ms * PS_PER_MS))
    }

    fn unit_count(&self) -> usize {
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
            if self.unit >= self.unit_count() {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransmissionStage {
    VoiceActivation,
    Image,
    Footer,
    StationIdentification,
    Silence,
    Complete,
}

/// A complete pull-based conventional SSTV transmission.
///
/// The stream contains MMSSTV's built-in VOX sequence, conventional VIS and
/// image raster, the FSKID footer and station identifier, and 500 milliseconds
/// of trailing silence.
#[derive(Debug)]
pub struct TransmissionEncoder {
    image: TxEncoder,
    fsk: FskEncoder,
    stage: TransmissionStage,
    voice_activation: usize,
    deadline_ps: u64,
}

impl TransmissionEncoder {
    /// Validates the mode and image and constructs a complete transmission.
    pub fn new(mode: Mode, image: RgbImage, station_id: FskId) -> Result<Self, SstvError> {
        Ok(Self {
            image: TxEncoder::new(mode, image)?,
            fsk: station_id.encoder(),
            stage: TransmissionStage::VoiceActivation,
            voice_activation: 0,
            deadline_ps: 0,
        })
    }

    /// Returns the exact duration of this complete transmission.
    pub fn duration(&self) -> SstvDuration {
        let spec = self.image.mode.spec();
        let scan = self.image.mode.scan();
        let units = usize::from(spec.active_rows() / u16::from(spec.rows_per_raster_unit()));
        let leading = scan
            .leading()
            .iter()
            .map(|segment| segment.duration().as_picos())
            .sum::<u64>();
        let raster = (0..units)
            .map(|unit| {
                scan.duration(unit)
                    .expect("supported transmit modes have raster segments")
                    .as_picos()
            })
            .sum::<u64>();
        let fsk = self
            .fsk
            .clone()
            .map(|event| u64::from(event.duration_micros()) * 1_000_000)
            .sum::<u64>();
        SstvDuration::from_picos(
            VOX_DURATION_PS
                + VIS_END_PS
                + leading
                + raster
                + 300 * PS_PER_MS
                + fsk
                + 500 * PS_PER_MS,
        )
    }

    fn emit(&mut self, component: TxComponent, frequency_hz: u32, duration_ps: u64) -> TimedTone {
        self.deadline_ps += duration_ps;
        TimedTone::new(
            component,
            Frequency::from_hz(frequency_hz),
            TxInstant::from_picos(self.deadline_ps),
        )
    }
}

impl Iterator for TransmissionEncoder {
    type Item = TimedTone;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.stage {
                TransmissionStage::VoiceActivation => {
                    if let Some((frequency_hz, duration_ms)) =
                        VOX.get(self.voice_activation).copied()
                    {
                        self.voice_activation += 1;
                        return Some(self.emit(
                            TxComponent::VoiceActivation,
                            frequency_hz,
                            duration_ms * PS_PER_MS,
                        ));
                    }
                    debug_assert_eq!(self.deadline_ps, VOX_DURATION_PS);
                    self.stage = TransmissionStage::Image;
                }
                TransmissionStage::Image => {
                    if let Some(tone) = self.image.next() {
                        self.deadline_ps = VOX_DURATION_PS + tone.until().as_picos();
                        return Some(TimedTone::new(
                            tone.component(),
                            tone.frequency(),
                            TxInstant::from_picos(self.deadline_ps),
                        ));
                    }
                    self.stage = TransmissionStage::Footer;
                }
                TransmissionStage::Footer => {
                    self.stage = TransmissionStage::StationIdentification;
                    return Some(self.emit(TxComponent::Footer, 1500, 300 * PS_PER_MS));
                }
                TransmissionStage::StationIdentification => {
                    if let Some(event) = self.fsk.next() {
                        return Some(self.emit(
                            TxComponent::StationIdentification,
                            u32::from(event.tone().frequency_hz()),
                            u64::from(event.duration_micros()) * 1_000_000,
                        ));
                    }
                    self.stage = TransmissionStage::Silence;
                }
                TransmissionStage::Silence => {
                    self.stage = TransmissionStage::Complete;
                    return Some(self.emit(TxComponent::Silence, 0, 500 * PS_PER_MS));
                }
                TransmissionStage::Complete => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use rstest::rstest;

    use super::*;
    use crate::{image::Rgb8, mode::ModeFamily};

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

    #[test]
    fn complete_transmission_frames_image_and_station_id() {
        let mode = Mode::Robot36;
        let fill = Rgb8::new(80, 140, 200);
        let image_end = TxEncoder::new(mode, image(mode, fill))
            .unwrap()
            .last()
            .unwrap()
            .until()
            .as_picos();
        let station_id = FskId::new("N0CALL").unwrap();
        let mut transmission =
            TransmissionEncoder::new(mode, image(mode, fill), station_id).unwrap();
        let duration = transmission.duration();

        let voice_activation: Vec<_> = transmission.by_ref().take(8).collect();
        assert_eq!(
            voice_activation
                .iter()
                .map(|tone| tone.frequency().as_hz())
                .collect::<Vec<_>>(),
            [1900, 1500, 1900, 1500, 2300, 1500, 2300, 1500]
        );
        assert!(
            voice_activation
                .iter()
                .all(|tone| tone.component() == TxComponent::VoiceActivation)
        );
        assert_eq!(voice_activation[7].until().as_picos(), VOX_DURATION_PS);

        let first_image = transmission.next().unwrap();
        assert_eq!(first_image.component(), TxComponent::Leader);
        assert_eq!(first_image.frequency().as_hz(), 1900);
        assert_eq!(first_image.until().as_picos(), 1_100 * PS_PER_MS);

        let footer = transmission
            .by_ref()
            .find(|tone| tone.component() == TxComponent::Footer)
            .unwrap();
        assert_eq!(footer.frequency().as_hz(), 1500);
        assert_eq!(
            footer.until().as_picos(),
            VOX_DURATION_PS + image_end + 300 * PS_PER_MS
        );

        let first_guard = transmission.next().unwrap();
        assert_eq!(first_guard.component(), TxComponent::StationIdentification);
        assert_eq!(first_guard.frequency().as_hz(), 2100);
        assert_eq!(
            first_guard.until().as_picos() - footer.until().as_picos(),
            100 * PS_PER_MS
        );
        let start = transmission.next().unwrap();
        assert_eq!(start.frequency().as_hz(), 1900);
        assert_eq!(
            start.until().as_picos() - first_guard.until().as_picos(),
            22 * PS_PER_MS
        );

        let silence = transmission
            .by_ref()
            .find(|tone| tone.component() == TxComponent::Silence)
            .unwrap();
        assert_eq!(silence.frequency().as_hz(), 0);
        assert_eq!(
            silence.until().as_picos(),
            footer.until().as_picos() + 1_410 * PS_PER_MS + 500 * PS_PER_MS
        );
        assert_eq!(duration.as_picos(), silence.until().as_picos());
        assert_eq!(transmission.next(), None);
    }
}
