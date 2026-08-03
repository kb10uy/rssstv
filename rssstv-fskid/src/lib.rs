//! MMSSTV-compatible FSKID protocol encoding and decoding.

#![no_std]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use core::{fmt, str};

const GUARD_SECONDS: f64 = 0.100;
const SYMBOL_SECONDS: f64 = 0.022;
const MAX_ID_LEN: usize = 16;

/// An error returned when station identifier text cannot be represented by FSKID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FskIdError {
    /// The identifier is empty.
    Empty,
    /// The identifier is longer than the 16-byte protocol limit.
    TooLong,
    /// A byte is outside the protocol text alphabet.
    InvalidByte {
        /// The byte offset in the supplied text.
        index: usize,
        /// The invalid byte value.
        byte: u8,
    },
}

impl fmt::Display for FskIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("FSKID must not be empty"),
            Self::TooLong => formatter.write_str("FSKID must not exceed 16 bytes"),
            Self::InvalidByte { index, byte } => write!(
                formatter,
                "invalid FSKID byte 0x{byte:02x} at offset {index}"
            ),
        }
    }
}

impl core::error::Error for FskIdError {}

/// A classified FSK detector sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FskTone {
    /// The 1900 Hz mark tone, representing one.
    Mark,
    /// The 2100 Hz space tone, representing zero.
    Space,
    /// No sufficiently reliable tone decision.
    Ambiguous,
}

/// A validated MMSSTV station identifier.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FskId {
    bytes: [u8; MAX_ID_LEN],
    len: u8,
}

impl FskId {
    /// Validates and stores protocol-compatible station identifier text unchanged.
    pub fn new(text: &str) -> Result<Self, FskIdError> {
        let source = text.as_bytes();
        if source.is_empty() {
            return Err(FskIdError::Empty);
        }
        if source.len() > MAX_ID_LEN {
            return Err(FskIdError::TooLong);
        }
        for (index, &byte) in source.iter().enumerate() {
            if !(0x20..=0x5f).contains(&byte) || byte == b'!' {
                return Err(FskIdError::InvalidByte { index, byte });
            }
        }
        let mut bytes = [0; MAX_ID_LEN];
        bytes[..source.len()].copy_from_slice(source);
        Ok(Self {
            bytes,
            len: source.len() as u8,
        })
    }

    /// Returns the decoded identifier as modified-ASCII text.
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("FSKID symbols always decode to ASCII")
    }

    /// Creates a bounded encoder for this identifier's complete physical FSKID sequence.
    pub const fn encoder(self) -> FskEncoder {
        FskEncoder::new(self)
    }
}

impl str::FromStr for FskId {
    type Err = FskIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl TryFrom<&str> for FskId {
    type Error = FskIdError;

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        Self::new(text)
    }
}

impl fmt::Display for FskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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
        let len = usize::from(self.id.len);
        match index {
            0 => 0x2a,
            index if index <= len => self.id.bytes[index - 1] - 0x20,
            index if index == len + 1 => 0x01,
            _ => self.id.bytes[..len]
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
        let bit_count = (usize::from(self.id.len) + 3) * 6;
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
        let total = (usize::from(self.id.len) + 3) * 6 + 3;
        let remaining = total.saturating_sub(usize::from(self.event));
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for FskEncoder {}

impl core::iter::FusedIterator for FskEncoder {}

#[derive(Clone, Copy, Debug)]
enum State {
    Search,
    Guard { remaining: u64 },
    Start { remaining: u64 },
    Midpoint { remaining: u64 },
    Data,
}

#[derive(Clone, Copy, Debug)]
enum Frame {
    Header,
    Call {
        bytes: [u8; MAX_ID_LEN],
        len: u8,
        checksum: u8,
    },
    Checksum {
        bytes: [u8; MAX_ID_LEN],
        len: u8,
        checksum: u8,
    },
}

/// A sample-driven decoder for MMSSTV six-bit FSKID records.
#[derive(Clone, Debug)]
pub struct FskDecoder {
    sample_rate_hz: f64,
    state: State,
    frame: Frame,
    elapsed: u64,
    next_sample: f64,
    bit_count: u8,
    symbol: u8,
}

impl FskDecoder {
    /// Creates a decoder for a positive physical sample rate.
    pub fn new(sample_rate_hz: u32) -> Self {
        assert!(sample_rate_hz > 0, "FSKID sample rate must be positive");
        Self {
            sample_rate_hz: f64::from(sample_rate_hz),
            state: State::Search,
            frame: Frame::Header,
            elapsed: 0,
            next_sample: 0.0,
            bit_count: 0,
            symbol: 0,
        }
    }

    /// Processes one classified detector sample and returns a completed ID.
    pub fn process(&mut self, tone: FskTone) -> Option<FskId> {
        match self.state {
            State::Search => {
                if tone == FskTone::Space {
                    self.state = State::Guard {
                        remaining: self.samples(GUARD_SECONDS / 2.0),
                    };
                }
            }
            State::Guard { remaining } => {
                if tone != FskTone::Space {
                    self.reset();
                } else if remaining <= 1 {
                    self.state = State::Start {
                        remaining: self.samples(GUARD_SECONDS),
                    };
                } else {
                    self.state = State::Guard {
                        remaining: remaining - 1,
                    };
                }
            }
            State::Start { remaining } => {
                if remaining <= 1 {
                    self.reset();
                } else if tone == FskTone::Mark {
                    self.state = State::Midpoint {
                        remaining: self.samples(SYMBOL_SECONDS / 2.0),
                    };
                } else {
                    self.state = State::Start {
                        remaining: remaining - 1,
                    };
                }
            }
            State::Midpoint { remaining } => {
                if remaining <= 1 {
                    if tone == FskTone::Mark {
                        self.state = State::Data;
                        self.elapsed = 0;
                        self.next_sample = self.sample_rate_hz * SYMBOL_SECONDS;
                        self.bit_count = 0;
                        self.symbol = 0;
                        self.frame = Frame::Header;
                    } else {
                        self.reset();
                    }
                } else {
                    self.state = State::Midpoint {
                        remaining: remaining - 1,
                    };
                }
            }
            State::Data => {
                self.elapsed += 1;
                if self.elapsed >= self.next_sample as u64 {
                    if tone == FskTone::Ambiguous {
                        self.reset();
                        return None;
                    }
                    self.next_sample += self.sample_rate_hz * SYMBOL_SECONDS;
                    self.symbol >>= 1;
                    if tone == FskTone::Mark {
                        self.symbol |= 0x20;
                    }
                    self.bit_count += 1;
                    if self.bit_count == 6 {
                        let symbol = self.symbol;
                        self.bit_count = 0;
                        self.symbol = 0;
                        return self.process_symbol(symbol);
                    }
                }
            }
        }
        None
    }

    fn samples(&self, seconds: f64) -> u64 {
        (self.sample_rate_hz * seconds) as u64
    }

    fn process_symbol(&mut self, symbol: u8) -> Option<FskId> {
        match self.frame {
            Frame::Header if symbol == 0x2a => {
                self.frame = Frame::Call {
                    bytes: [0; MAX_ID_LEN],
                    len: 0,
                    checksum: 0,
                };
            }
            Frame::Header => self.reset(),
            Frame::Call {
                bytes,
                len,
                checksum,
            } if symbol == 0x01 && len > 0 => {
                self.frame = Frame::Checksum {
                    bytes,
                    len,
                    checksum,
                };
            }
            Frame::Call {
                mut bytes,
                len,
                checksum,
            } if usize::from(len) < MAX_ID_LEN => {
                bytes[usize::from(len)] = symbol + 0x20;
                self.frame = Frame::Call {
                    bytes,
                    len: len + 1,
                    checksum: checksum ^ symbol,
                };
            }
            Frame::Call { .. } => self.reset(),
            Frame::Checksum {
                bytes,
                len,
                checksum,
            } => {
                self.reset();
                if symbol == checksum & 0x3f {
                    let mut start = 0;
                    let mut end = usize::from(len);
                    while start < end && matches!(bytes[start], b' ' | b'\t') {
                        start += 1;
                    }
                    while end > start && matches!(bytes[end - 1], b' ' | b'\t') {
                        end -= 1;
                    }
                    let mut trimmed = [0; MAX_ID_LEN];
                    trimmed[..end - start].copy_from_slice(&bytes[start..end]);
                    if end > start {
                        return Some(FskId {
                            bytes: trimmed,
                            len: (end - start) as u8,
                        });
                    }
                }
            }
        }
        None
    }

    fn reset(&mut self) {
        self.state = State::Search;
        self.frame = Frame::Header;
        self.elapsed = 0;
        self.next_sample = 0.0;
        self.bit_count = 0;
        self.symbol = 0;
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::string::ToString;
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

    fn feed_tone(
        decoder: &mut FskDecoder,
        tone: FskTone,
        seconds: f64,
        rate: u32,
        written: &mut u64,
        deadline: &mut f64,
    ) -> Option<FskId> {
        *deadline += f64::from(rate) * seconds;
        let mut event = None;
        while *written < *deadline as u64 {
            event = decoder.process(tone).or(event);
            *written += 1;
        }
        event
    }

    fn decode(symbols: &[u8], rate: u32) -> Option<FskId> {
        let mut decoder = FskDecoder::new(rate);
        let mut written = 0;
        let mut deadline = 0.0;
        feed_tone(
            &mut decoder,
            FskTone::Space,
            GUARD_SECONDS,
            rate,
            &mut written,
            &mut deadline,
        );
        feed_tone(
            &mut decoder,
            FskTone::Mark,
            SYMBOL_SECONDS,
            rate,
            &mut written,
            &mut deadline,
        );
        let mut event = None;
        for &symbol in symbols {
            for bit in 0..6 {
                let tone = if symbol & (1 << bit) == 0 {
                    FskTone::Space
                } else {
                    FskTone::Mark
                };
                event = feed_tone(
                    &mut decoder,
                    tone,
                    SYMBOL_SECONDS,
                    rate,
                    &mut written,
                    &mut deadline,
                )
                .or(event);
            }
        }
        event
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(44_100)]
    #[case(48_000)]
    fn decodes_jl1his_with_fractional_symbol_timing(#[case] rate: u32) {
        assert_eq!(decode(&JL1HIS, rate).unwrap().as_str(), "JL1HIS");
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

    #[rstest]
    #[case("", FskIdError::Empty)]
    #[case("ABCDEFGHIJKLMNOPQ", FskIdError::TooLong)]
    #[case("N0CALL!", FskIdError::InvalidByte { index: 6, byte: b'!' })]
    #[case("n0call", FskIdError::InvalidByte { index: 0, byte: b'n' })]
    #[case("N0\n", FskIdError::InvalidByte { index: 2, byte: b'\n' })]
    #[case("N\0", FskIdError::InvalidByte { index: 1, byte: 0 })]
    fn rejects_invalid_identifier(#[case] text: &str, #[case] expected: FskIdError) {
        assert_eq!(FskId::new(text), Err(expected));
    }

    #[test]
    fn preserves_and_displays_protocol_text() {
        let id: FskId = " N0_CALL ".parse().unwrap();
        assert_eq!(id.as_str(), " N0_CALL ");
        assert_eq!(id.to_string(), " N0_CALL ");
        assert_eq!(FskIdError::Empty.to_string(), "FSKID must not be empty");
        assert_eq!(
            FskIdError::InvalidByte {
                index: 2,
                byte: b'!',
            }
            .to_string(),
            "invalid FSKID byte 0x21 at offset 2"
        );
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut symbols = JL1HIS;
        symbols[8] ^= 1;
        assert_eq!(decode(&symbols, 8_000), None);
    }

    #[test]
    fn trims_identifier_whitespace() {
        let symbols = [
            0x2a, 0x00, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x00, 0x01, 0x25,
        ];
        assert_eq!(decode(&symbols, 8_000).unwrap().as_str(), "JL1HIS");
    }

    #[test]
    fn rejects_short_guard() {
        let mut decoder = FskDecoder::new(8_000);
        for _ in 0..399 {
            decoder.process(FskTone::Space);
        }
        for _ in 0..10_000 {
            decoder.process(FskTone::Mark);
        }
        assert_eq!(decoder.process(FskTone::Space), None);
    }
}
