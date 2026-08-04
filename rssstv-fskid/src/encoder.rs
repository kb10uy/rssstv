use crate::FskId;

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
#[derive(Clone, Debug)]
pub struct FskEncoder {
    id: FskId,
    event: u16,
}

impl FskEncoder {
    /// Creates an encoder for a validated station identifier.
    pub const fn new(id: FskId) -> Self {
        Self { id, event: 0 }
    }

    fn symbol(&self, index: usize) -> u8 {
        let bytes = self.id.as_str().as_bytes();
        match index {
            0 => 0x2a,
            index if index <= bytes.len() => bytes[index - 1] - 0x20,
            index if index == bytes.len() + 1 => 0x01,
            _ => bytes
                .iter()
                .fold(0, |checksum, byte| checksum ^ (byte - 0x20)),
        }
    }

    const fn event(tone: FskTxTone, duration_micros: u32) -> FskTxEvent {
        FskTxEvent {
            tone,
            duration_micros,
        }
    }
}

impl Iterator for FskEncoder {
    type Item = FskTxEvent;

    fn next(&mut self) -> Option<Self::Item> {
        let event = usize::from(self.event);
        let bit_count = (self.id.as_str().len() + 3) * 6;
        let output = match event {
            0 => Self::event(FskTxTone::Space, 100_000),
            1 => Self::event(FskTxTone::Mark, 22_000),
            event if event < bit_count + 2 => {
                let bit = event - 2;
                let symbol = self.symbol(bit / 6);
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
        let total = (self.id.as_str().len() + 3) * 6 + 3;
        let remaining = total.saturating_sub(usize::from(self.event));
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FskEncoder {}

impl core::iter::FusedIterator for FskEncoder {}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const JL1HIS: [u8; 9] = [0x2a, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25];

    fn assert_encoding(text: &str, symbols: &[u8]) {
        let id = FskId::new(text).unwrap();
        let mut encoder = id.encoder();
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
