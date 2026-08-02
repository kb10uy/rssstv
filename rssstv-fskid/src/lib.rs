//! MMSSTV-compatible FSKID protocol decoding.

#![no_std]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

use core::fmt;
use core::str;

const GUARD_SECONDS: f64 = 0.100;
const SYMBOL_SECONDS: f64 = 0.022;
const MAX_ID_LEN: usize = 16;

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
    /// Returns the decoded identifier as modified-ASCII text.
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("FSKID symbols always decode to ASCII")
    }
}

impl fmt::Display for FskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
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
                    return Some(FskId {
                        bytes: trimmed,
                        len: (end - start) as u8,
                    });
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
