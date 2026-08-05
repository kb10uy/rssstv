use crate::{
    FskId, FskNumber,
    id::{MAX_ID_LEN, MAX_NUMBER_LEN},
};

/// The longest symbol sequence a record pair can produce: an identifier with
/// its header, terminator, and checksum, then a number with its own two.
const MAX_SYMBOLS: usize = MAX_ID_LEN + 3 + MAX_NUMBER_LEN + 2;

/// A physical tone emitted by an FSKID encoder.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FskTxTone {
    /// The 1900 Hz mark tone.
    Mark,
    /// The 2100 Hz space tone.
    Space,
}

impl FskTxTone {
    /// Returns the physical frequency in hertz.
    pub const fn frequency_hz(self) -> u16 {
        match self {
            Self::Mark => 1900,
            Self::Space => 2100,
        }
    }
}

/// One exact-duration physical tone in an encoded FSKID sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FskTxEvent {
    tone: FskTxTone,
    duration_micros: u32,
}

impl FskTxEvent {
    /// Returns the physical tone.
    pub const fn tone(self) -> FskTxTone {
        self.tone
    }

    /// Returns the exact event duration in microseconds.
    pub const fn duration_micros(self) -> u32 {
        self.duration_micros
    }
}

/// A bounded, pull-based encoder for a complete physical FSKID sequence.
///
/// The symbols are laid out once rather than derived per event, because a
/// transmission may carry two records and the second one's framing depends on
/// what the number in it is.
#[derive(Clone, Debug)]
pub struct FskEncoder {
    symbols: [u8; MAX_SYMBOLS],
    len: u8,
    event: u16,
}

impl FskEncoder {
    /// Creates an encoder for a validated station identifier.
    pub fn new(id: FskId) -> Self {
        Self::build(id, None)
    }

    /// Creates an encoder for an identifier followed by a contest number.
    ///
    /// The number record follows the identifier's checksum directly, with no
    /// guard or header of its own, which is where the receiver looks for it.
    pub fn with_number(id: FskId, number: FskNumber) -> Self {
        Self::build(id, Some(number))
    }

    fn build(id: FskId, number: Option<FskNumber>) -> Self {
        let mut symbols = [0; MAX_SYMBOLS];
        let mut len = 0;
        {
            let mut push = |symbol: u8| {
                symbols[len] = symbol;
                len += 1;
            };
            push(0x2a);
            let mut checksum = 0;
            for &byte in id.as_str().as_bytes() {
                checksum ^= byte - 0x20;
                push(byte - 0x20);
            }
            push(0x01);
            push(checksum & 0x3f);
            match number.map(|number| (number, number.count())) {
                Some((_, Some(count))) => {
                    let high = ((count >> 6) & 0x3f) as u8;
                    let low = (count & 0x3f) as u8;
                    push(0x02);
                    push(high);
                    push(low);
                    push(0x02 ^ high ^ low);
                }
                Some((number, None)) => {
                    let mut checksum = 0;
                    for &byte in number.as_str().as_bytes() {
                        checksum ^= byte - 0x20;
                        push(byte - 0x20);
                    }
                    push(0x01);
                    push(checksum & 0x3f);
                }
                None => {}
            }
        }
        Self {
            symbols,
            len: len as u8,
            event: 0,
        }
    }

    const fn event(tone: FskTxTone, duration_micros: u32) -> FskTxEvent {
        FskTxEvent {
            tone,
            duration_micros,
        }
    }

    fn total_events(&self) -> usize {
        usize::from(self.len) * 6 + 3
    }
}

impl Iterator for FskEncoder {
    type Item = FskTxEvent;

    fn next(&mut self) -> Option<Self::Item> {
        let event = usize::from(self.event);
        let bit_count = usize::from(self.len) * 6;
        let output = match event {
            0 => Self::event(FskTxTone::Space, 100_000),
            1 => Self::event(FskTxTone::Mark, 22_000),
            event if event < bit_count + 2 => {
                let bit = event - 2;
                let symbol = self.symbols[bit / 6];
                let tone = if symbol & (1 << (bit % 6)) == 0 {
                    FskTxTone::Space
                } else {
                    FskTxTone::Mark
                };
                Self::event(tone, 22_000)
            }
            event if event == bit_count + 2 => Self::event(FskTxTone::Space, 100_000),
            _ => return None,
        };
        self.event += 1;
        Some(output)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.total_events().saturating_sub(usize::from(self.event));
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FskEncoder {}

impl core::iter::FusedIterator for FskEncoder {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FskNumberError;
    use rstest::rstest;

    const JL1HIS: [u8; 9] = [0x2a, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25];

    fn assert_encoding(text: &str, symbols: &[u8]) {
        assert_events(FskId::new(text).unwrap().encoder(), symbols);
    }

    fn assert_events(mut encoder: FskEncoder, symbols: &[u8]) {
        assert_eq!(
            encoder.next(),
            Some(FskTxEvent {
                tone: FskTxTone::Space,
                duration_micros: 100_000,
            })
        );
        assert_eq!(
            encoder.next(),
            Some(FskTxEvent {
                tone: FskTxTone::Mark,
                duration_micros: 22_000,
            })
        );
        for &symbol in symbols {
            for bit in 0..6 {
                let tone = if symbol & (1 << bit) == 0 {
                    FskTxTone::Space
                } else {
                    FskTxTone::Mark
                };
                assert_eq!(
                    encoder.next(),
                    Some(FskTxEvent {
                        tone,
                        duration_micros: 22_000,
                    })
                );
            }
        }
        assert_eq!(
            encoder.next(),
            Some(FskTxEvent {
                tone: FskTxTone::Space,
                duration_micros: 100_000,
            })
        );
        assert_eq!(encoder.next(), None);
        assert_eq!(encoder.next(), None);
    }

    #[rstest]
    #[case("JL1HIS", &JL1HIS)]
    #[case(
        "N0CALL",
        &[0x2a, 0x2e, 0x10, 0x23, 0x21, 0x2c, 0x2c, 0x01, 0x3c]
    )]
    fn encodes_physical_protocol_vector(#[case] text: &str, #[case] symbols: &[u8]) {
        assert_encoding(text, symbols);
    }

    /// The contest record follows the identifier's checksum with no framing of
    /// its own, and takes the counted form only for a number that fits it.
    #[rstest]
    #[case::counted("001", &[0x02, 0x00, 0x01, 0x03])]
    #[case::counted_wide("4095", &[0x02, 0x3f, 0x3f, 0x02])]
    #[case::text_short("12", &[0x11, 0x12, 0x01, 0x03])]
    #[case::text_padded("0012", &[0x10, 0x10, 0x11, 0x12, 0x01, 0x03])]
    #[case::text_letters("13H", &[0x11, 0x13, 0x28, 0x01, 0x2a])]
    #[case::text_beyond_the_count("5000", &[0x15, 0x10, 0x10, 0x10, 0x01, 0x05])]
    fn encodes_a_contest_number_after_the_identifier(#[case] number: &str, #[case] record: &[u8]) {
        let mut symbols = [0; 16];
        symbols[..JL1HIS.len()].copy_from_slice(&JL1HIS);
        symbols[JL1HIS.len()..JL1HIS.len() + record.len()].copy_from_slice(record);
        let id = FskId::new("JL1HIS").unwrap();
        let number = FskNumber::new(number).unwrap();

        assert_events(
            id.encoder_with_number(number),
            &symbols[..JL1HIS.len() + record.len()],
        );
    }

    #[rstest]
    #[case("", FskNumberError::Empty)]
    #[case("123456789", FskNumberError::TooLong)]
    #[case("00 1", FskNumberError::InvalidByte { index: 2, byte: b' ' })]
    #[case("001a", FskNumberError::InvalidByte { index: 3, byte: b'a' })]
    fn rejects_invalid_contest_number(#[case] text: &str, #[case] expected: FskNumberError) {
        assert_eq!(FskNumber::new(text), Err(expected));
    }

    #[test]
    fn exposes_physical_tone_frequencies_and_exact_size() {
        assert_eq!(FskTxTone::Mark.frequency_hz(), 1900);
        assert_eq!(FskTxTone::Space.frequency_hz(), 2100);
        let mut encoder = FskId::new("N0CALL").unwrap().encoder();
        assert_eq!(encoder.len(), 57);
        let first = encoder.next().unwrap();
        assert_eq!(first.tone(), FskTxTone::Space);
        assert_eq!(first.duration_micros(), 100_000);
        assert_eq!(encoder.len(), 56);
    }
}
