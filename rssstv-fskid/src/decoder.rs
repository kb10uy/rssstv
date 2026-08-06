use core::num::NonZeroU32;

use crate::{
    FskId, FskNumber, FskTone,
    id::{MAX_ID_LEN, MAX_NUMBER_LEN},
};

const GUARD_SECONDS: f64 = 0.100;
const SYMBOL_SECONDS: f64 = 0.022;
/// The symbol a contest number in counted form begins with.
const COUNTED_NUMBER: u8 = 0x02;
/// The symbol every record ends with.
const END_OF_RECORD: u8 = 0x01;
/// The lowest symbol a contest number may spell itself with.
const NUMBER_FLOOR: u8 = 0x10;

/// One complete record read off the air.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FskRecord {
    /// The station identifier every FSKID transmission opens with.
    Id(FskId),
    /// The contest number that may follow that identifier.
    Number(FskNumber),
}

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
    /// The contest number a station may append, before its form is known.
    Number {
        bytes: [u8; MAX_NUMBER_LEN],
        len: u8,
        checksum: u8,
    },
    NumberChecksum {
        bytes: [u8; MAX_NUMBER_LEN],
        len: u8,
        checksum: u8,
    },
    Counted {
        count: u16,
        len: u8,
        checksum: u8,
    },
    CountedChecksum {
        count: u16,
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
    /// Creates a decoder for a physical sample rate.
    pub fn new(sample_rate_hz: NonZeroU32) -> Self {
        Self {
            sample_rate_hz: f64::from(sample_rate_hz.get()),
            state: State::Search,
            frame: Frame::Header,
            elapsed: 0,
            next_sample: 0.0,
            bit_count: 0,
            symbol: 0,
        }
    }

    /// Processes one classified detector sample and returns a completed record.
    pub fn process(&mut self, tone: FskTone) -> Option<FskRecord> {
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

    fn process_symbol(&mut self, symbol: u8) -> Option<FskRecord> {
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
            } if symbol == END_OF_RECORD && len > 0 => {
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
                if symbol != checksum & 0x3f {
                    self.reset();
                    return None;
                }
                // A contest number follows the identifier directly, with no
                // guard or header of its own, so the decoder stays in the data
                // it is already reading rather than searching again.
                self.frame = Frame::Number {
                    bytes: [0; MAX_NUMBER_LEN],
                    len: 0,
                    checksum: 0,
                };
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
                    return Some(FskRecord::Id(FskId::from_symbols(
                        trimmed,
                        (end - start) as u8,
                    )));
                }
            }
            Frame::Number {
                bytes,
                len,
                checksum,
            } if symbol == END_OF_RECORD && len > 0 => {
                self.frame = Frame::NumberChecksum {
                    bytes,
                    len,
                    checksum,
                };
            }
            Frame::Number { .. } if symbol == COUNTED_NUMBER => {
                self.frame = Frame::Counted {
                    count: 0,
                    len: 0,
                    checksum: COUNTED_NUMBER,
                };
            }
            Frame::Number {
                mut bytes,
                len,
                checksum,
            } if symbol >= NUMBER_FLOOR => {
                bytes[usize::from(len)] = symbol + 0x20;
                if usize::from(len) + 1 >= MAX_NUMBER_LEN {
                    self.reset();
                } else {
                    self.frame = Frame::Number {
                        bytes,
                        len: len + 1,
                        checksum: checksum ^ symbol,
                    };
                }
            }
            Frame::Number { .. } => self.reset(),
            Frame::NumberChecksum {
                bytes,
                len,
                checksum,
            } => {
                self.reset();
                if symbol == checksum & 0x3f {
                    return Some(FskRecord::Number(FskNumber::from_symbols(bytes, len)));
                }
            }
            Frame::Counted {
                count,
                len,
                checksum,
            } => {
                let count = (count << 6) | u16::from(symbol);
                let checksum = checksum ^ symbol;
                self.frame = if len == 0 {
                    Frame::Counted {
                        count,
                        len: 1,
                        checksum,
                    }
                } else {
                    Frame::CountedChecksum { count, checksum }
                };
            }
            Frame::CountedChecksum { count, checksum } => {
                self.reset();
                if symbol == checksum & 0x3f {
                    return Some(FskRecord::Number(FskNumber::from_count(count)));
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
    use super::*;
    use rstest::rstest;

    const JL1HIS: [u8; 9] = [0x2a, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x01, 0x25];

    fn feed_tone(
        decoder: &mut FskDecoder,
        tone: FskTone,
        seconds: f64,
        rate: u32,
        written: &mut u64,
        deadline: &mut f64,
    ) -> Option<FskRecord> {
        *deadline += f64::from(rate) * seconds;
        let mut event = None;
        while *written < *deadline as u64 {
            event = decoder.process(tone).or(event);
            *written += 1;
        }
        event
    }

    /// Decodes every record in one symbol vector, keeping the first of each
    /// kind, because a callsign may be followed by a contest number.
    fn decode(symbols: &[u8], rate: u32) -> (Option<FskId>, Option<FskNumber>) {
        let mut decoder = FskDecoder::new(NonZeroU32::new(rate).unwrap());
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
        let mut id = None;
        let mut number = None;
        for &symbol in symbols {
            for bit in 0..6 {
                let tone = if symbol & (1 << bit) == 0 {
                    FskTone::Space
                } else {
                    FskTone::Mark
                };
                match feed_tone(
                    &mut decoder,
                    tone,
                    SYMBOL_SECONDS,
                    rate,
                    &mut written,
                    &mut deadline,
                ) {
                    Some(FskRecord::Id(decoded)) => id = id.or(Some(decoded)),
                    Some(FskRecord::Number(decoded)) => number = number.or(Some(decoded)),
                    None => {}
                }
            }
        }
        (id, number)
    }

    fn decode_id(symbols: &[u8], rate: u32) -> Option<FskId> {
        decode(symbols, rate).0
    }

    fn decode_number(symbols: &[u8], rate: u32) -> Option<FskNumber> {
        decode(symbols, rate).1
    }

    #[rstest]
    #[case(8_000)]
    #[case(11_025)]
    #[case(44_100)]
    #[case(48_000)]
    fn decodes_jl1his_with_fractional_symbol_timing(#[case] rate: u32) {
        assert_eq!(decode_id(&JL1HIS, rate).unwrap().as_str(), "JL1HIS");
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut symbols = JL1HIS;
        symbols[8] ^= 1;
        assert_eq!(decode_id(&symbols, 8_000), None);
    }

    #[test]
    fn trims_identifier_whitespace() {
        let symbols = [
            0x2a, 0x00, 0x2a, 0x2c, 0x11, 0x28, 0x29, 0x33, 0x00, 0x01, 0x25,
        ];
        assert_eq!(decode_id(&symbols, 8_000).unwrap().as_str(), "JL1HIS");
    }

    /// A contest number follows the identifier directly, so both records come
    /// out of one acquisition.
    #[rstest]
    #[case::text(&[0x10, 0x10, 0x11, 0x01, 0x11], "001")]
    #[case::text_letters(&[0x11, 0x13, 0x28, 0x01, 0x2a], "13H")]
    #[case::counted(&[0x02, 0x00, 0x01, 0x03], "001")]
    #[case::counted_wide(&[0x02, 0x3f, 0x3f, 0x02], "4095")]
    fn decodes_a_contest_number_after_the_identifier(
        #[case] number: &[u8],
        #[case] expected: &str,
    ) {
        let mut symbols = [0; 16];
        symbols[..JL1HIS.len()].copy_from_slice(&JL1HIS);
        symbols[JL1HIS.len()..JL1HIS.len() + number.len()].copy_from_slice(number);
        let symbols = &symbols[..JL1HIS.len() + number.len()];

        let (id, decoded) = decode(symbols, 8_000);

        assert_eq!(id.unwrap().as_str(), "JL1HIS");
        assert_eq!(decoded.unwrap().as_str(), expected);
    }

    #[rstest]
    #[case::text(&[0x10, 0x10, 0x11, 0x01, 0x10])]
    #[case::counted(&[0x02, 0x00, 0x01, 0x02])]
    #[case::below_the_alphabet(&[0x10, 0x0f, 0x01, 0x1f])]
    fn rejects_a_malformed_contest_number(#[case] number: &[u8]) {
        let mut symbols = [0; 16];
        symbols[..JL1HIS.len()].copy_from_slice(&JL1HIS);
        symbols[JL1HIS.len()..JL1HIS.len() + number.len()].copy_from_slice(number);

        assert_eq!(
            decode_number(&symbols[..JL1HIS.len() + number.len()], 8_000),
            None
        );
    }

    /// A transmission that sends no contest number is the ordinary case, and
    /// the silence after the identifier must not produce one.
    #[test]
    fn an_identifier_alone_yields_no_contest_number() {
        assert_eq!(decode_number(&JL1HIS, 8_000), None);
    }

    #[test]
    fn rejects_short_guard() {
        let mut decoder = FskDecoder::new(NonZeroU32::new(8_000).unwrap());
        for _ in 0..399 {
            decoder.process(FskTone::Space);
        }
        for _ in 0..10_000 {
            decoder.process(FskTone::Mark);
        }
        assert_eq!(decoder.process(FskTone::Space), None);
    }
}
