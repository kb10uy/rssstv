//! The encoder for a whole transmission: VOX, the image, and the identifier.

use crate::{
    SstvError,
    image::RgbImage,
    mode::Mode,
    signal::{Frequency, LEADER_HZ, PORCH_HZ, TimedTone, TxComponent},
    time::{SstvDuration, TxInstant},
    tx::{PS_PER_MS, TxEncoder, VIS_END_PS},
};
use rssstv_fskid::{FskEncoder, FskId, FskNumber};

/// How long MMSSTV's built-in VOX sequence lasts.
const VOX_DURATION_PS: u64 = 800 * PS_PER_MS;

const VOX: [(u32, u64); 8] = [
    (LEADER_HZ, 100),
    (PORCH_HZ, 100),
    (LEADER_HZ, 100),
    (PORCH_HZ, 100),
    (2300, 100),
    (PORCH_HZ, 100),
    (2300, 100),
    (PORCH_HZ, 100),
];

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
    encoder: TxEncoder,
    fsk: Option<FskEncoder>,
    stage: TransmissionStage,
    voice_activation: usize,
    deadline_ps: u64,
}

impl TransmissionEncoder {
    /// Validates the mode and image and constructs a complete transmission.
    pub fn new(mode: Mode, image: RgbImage, station_id: FskId) -> Result<Self, SstvError> {
        Ok(Self {
            encoder: TxEncoder::new(mode, image)?,
            fsk: Some(station_id.encoder()),
            stage: TransmissionStage::VoiceActivation,
            voice_activation: 0,
            deadline_ps: 0,
        })
    }

    /// Constructs the same transmission with a contest number after the
    /// identifier.
    ///
    /// The number rides on the identifier rather than being announced on its
    /// own, so a transmission that sends no identifier sends no number either.
    pub fn with_contest_number(
        mode: Mode,
        image: RgbImage,
        station_id: FskId,
        number: FskNumber,
    ) -> Result<Self, SstvError> {
        Ok(Self {
            encoder: TxEncoder::new(mode, image)?,
            fsk: Some(station_id.encoder_with_number(number)),
            stage: TransmissionStage::VoiceActivation,
            voice_activation: 0,
            deadline_ps: 0,
        })
    }

    /// Constructs the same transmission with no station identifier.
    ///
    /// The identifier footer goes with it: the 1500 Hz tone exists to introduce
    /// the FSK symbols, so sending it alone would announce an identifier that
    /// never arrives.
    pub fn without_identifier(mode: Mode, image: RgbImage) -> Result<Self, SstvError> {
        Ok(Self {
            encoder: TxEncoder::new(mode, image)?,
            fsk: None,
            stage: TransmissionStage::VoiceActivation,
            voice_activation: 0,
            deadline_ps: 0,
        })
    }

    /// Returns the exact duration of this complete transmission.
    ///
    /// Only before the first tone has been pulled: the identifier is summed
    /// from the tones still to come, so on an encoder already being iterated
    /// the answer shrinks with it. Ask before iterating.
    pub fn duration(&self) -> SstvDuration {
        let identification = self.fsk.clone().map_or(0, |fsk| {
            300 * PS_PER_MS
                + fsk
                    .map(|event| u64::from(event.duration_micros()) * 1_000_000)
                    .sum::<u64>()
        });
        SstvDuration::from_picos(
            self.raster_start().as_picos()
                + self.raster_duration().as_picos()
                + identification
                + 500 * PS_PER_MS,
        )
    }

    /// Returns the offset at which the first scanned row begins.
    ///
    /// The VOX sequence, conventional VIS framing, and the mode's leading
    /// segments precede the raster and carry no image rows.
    pub fn raster_start(&self) -> SstvDuration {
        let leading = self
            .encoder
            .mode()
            .scan()
            .leading()
            .iter()
            .map(|segment| segment.duration().as_picos())
            .sum::<u64>();
        SstvDuration::from_picos(VOX_DURATION_PS + VIS_END_PS + leading)
    }

    /// Returns the duration of the image raster alone.
    ///
    /// The raster spans [`crate::mode::ModeSpec::active_rows`] rows at a
    /// uniform rate, so an offset into it maps linearly onto the row being
    /// transmitted.
    pub fn raster_duration(&self) -> SstvDuration {
        let spec = self.encoder.mode().spec();
        let scan = self.encoder.mode().scan();
        let units = usize::from(spec.active_rows() / u16::from(spec.rows_per_raster_unit()));
        SstvDuration::from_picos(
            (0..units)
                .map(|unit| {
                    scan.duration(unit)
                        .expect("supported transmit modes have raster segments")
                        .as_picos()
                })
                .sum::<u64>(),
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
                    if let Some(tone) = self.encoder.next() {
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
                    if self.fsk.is_none() {
                        self.stage = TransmissionStage::Silence;
                        continue;
                    }
                    self.stage = TransmissionStage::StationIdentification;
                    return Some(self.emit(TxComponent::Footer, PORCH_HZ, 300 * PS_PER_MS));
                }
                TransmissionStage::StationIdentification => {
                    if let Some(event) = self.fsk.as_mut().and_then(Iterator::next) {
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
    use crate::{image::Rgb8, tx::test_image as image};

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

    /// A contest number is four more symbols on the end of the identifier, and
    /// nothing else about the transmission moves.
    #[test]
    fn a_contest_number_lengthens_only_the_identifier() {
        let mode = Mode::Robot36;
        let station_id = FskId::new("N0CALL").unwrap();
        let plain =
            TransmissionEncoder::new(mode, image(mode, Rgb8::default()), station_id).unwrap();
        let numbered = TransmissionEncoder::with_contest_number(
            mode,
            image(mode, Rgb8::default()),
            station_id,
            FskNumber::new("001").unwrap(),
        )
        .unwrap();

        assert_eq!(numbered.raster_start(), plain.raster_start());
        assert_eq!(
            numbered.duration().as_picos() - plain.duration().as_picos(),
            4 * 6 * 22 * PS_PER_MS,
            "the counted record is four symbols of six bits"
        );
    }

    /// An operator who turns the identifier off gets neither the FSK symbols
    /// nor the footer that introduces them, and the transmission is exactly
    /// that much shorter.
    #[test]
    fn a_transmission_without_an_identifier_ends_after_the_raster() {
        let mode = Mode::Robot36;
        let station_id = FskId::new("N0CALL").unwrap();
        let identified =
            TransmissionEncoder::new(mode, image(mode, Rgb8::default()), station_id).unwrap();
        let anonymous =
            TransmissionEncoder::without_identifier(mode, image(mode, Rgb8::default())).unwrap();

        assert!(anonymous.duration() < identified.duration());
        assert_eq!(
            anonymous.raster_start(),
            identified.raster_start(),
            "the picture itself is unchanged"
        );
        let components: Vec<TxComponent> = anonymous.map(|tone| tone.component()).collect();
        assert!(!components.contains(&TxComponent::Footer));
        assert!(!components.contains(&TxComponent::StationIdentification));
        assert_eq!(components.last(), Some(&TxComponent::Silence));
    }

    /// Progress reported by row needs the raster's exact place in the stream:
    /// the leader before it and the identifier after it carry no image rows.
    #[rstest]
    #[case(Mode::Martin1, 0)]
    #[case(Mode::Scottie1, 9_000_000_000)]
    #[case(Mode::Robot36, 0)]
    #[case(Mode::Pd290, 0)]
    fn the_raster_window_brackets_exactly_the_scanned_rows(
        #[case] mode: Mode,
        #[case] leading_ps: u64,
    ) {
        let station_id = FskId::new("N0CALL").unwrap();
        let mut transmission =
            TransmissionEncoder::new(mode, image(mode, Rgb8::default()), station_id).unwrap();
        let start = transmission.raster_start().as_picos();
        assert_eq!(start, VOX_DURATION_PS + VIS_END_PS + leading_ps);

        let units =
            u64::from(mode.spec().active_rows() / u16::from(mode.spec().rows_per_raster_unit()));
        assert_eq!(
            transmission.raster_duration().as_picos(),
            units * mode.spec().period().as_picos()
        );

        // The image stage ends where the raster does, so the footer that opens
        // the trailing framing is the first tone past the window.
        let end = start + transmission.raster_duration().as_picos();
        let footer = transmission
            .find(|tone| tone.component() == TxComponent::Footer)
            .unwrap();
        assert_eq!(footer.until().as_picos(), end + 300 * PS_PER_MS);
    }
}
