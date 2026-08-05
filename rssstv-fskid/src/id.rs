use core::{fmt, str};

use crate::{FskEncoder, FskIdError, FskNumberError};

pub(crate) const MAX_ID_LEN: usize = 16;
/// How many number symbols the receiver reads before it gives up on the record.
pub(crate) const MAX_NUMBER_LEN: usize = 9;
/// The longest contest number a receiver still accepts, one short of that.
pub(crate) const MAX_NUMBER_TEXT_LEN: usize = MAX_NUMBER_LEN - 1;

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
    pub fn encoder(self) -> FskEncoder {
        FskEncoder::new(self)
    }

    /// Creates the same encoder with a contest number after the identifier.
    pub fn encoder_with_number(self, number: FskNumber) -> FskEncoder {
        FskEncoder::with_number(self, number)
    }

    /// Accepts identifier text the decoder has already validated against the
    /// protocol alphabet and length limit.
    pub(crate) const fn from_symbols(bytes: [u8; MAX_ID_LEN], len: u8) -> Self {
        Self { bytes, len }
    }
}

/// A validated MMSSTV contest number, from the record that may follow an
/// identifier.
///
/// The protocol carries the value either as text or as a twelve-bit count, and
/// both reach the receiver as the same thing: the number the other station is
/// giving out. So both forms decode into this one type, with the counted form
/// printed the way MMSSTV prints it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FskNumber {
    bytes: [u8; MAX_NUMBER_LEN],
    len: u8,
}

impl FskNumber {
    /// Validates and stores protocol-compatible contest number text unchanged.
    ///
    /// The record's alphabet begins where the identifier's digits do, so a
    /// number is uppercase and carries no spaces however it is spelled.
    pub fn new(text: &str) -> Result<Self, FskNumberError> {
        let source = text.as_bytes();
        if source.is_empty() {
            return Err(FskNumberError::Empty);
        }
        if source.len() > MAX_NUMBER_TEXT_LEN {
            return Err(FskNumberError::TooLong);
        }
        for (index, &byte) in source.iter().enumerate() {
            if !(0x30..=0x5f).contains(&byte) {
                return Err(FskNumberError::InvalidByte { index, byte });
            }
        }
        let mut bytes = [0; MAX_NUMBER_LEN];
        bytes[..source.len()].copy_from_slice(source);
        Ok(Self {
            bytes,
            len: source.len() as u8,
        })
    }

    /// Returns the decoded contest number as text.
    pub fn as_str(&self) -> &str {
        str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("FSKID symbols always decode to ASCII")
    }

    /// Accepts number text the decoder has already validated against the
    /// protocol alphabet and length limit.
    pub(crate) const fn from_symbols(bytes: [u8; MAX_NUMBER_LEN], len: u8) -> Self {
        Self { bytes, len }
    }

    /// Returns the value a counted record would carry, when this is a number
    /// MMSSTV would send in that form rather than as text.
    ///
    /// Three digits or more, and small enough for the twelve bits the counted
    /// form has. A fourth digit is only counted when the number really needs
    /// it, so `0012` travels as text where `1000` travels as a count.
    pub(crate) fn count(&self) -> Option<u16> {
        let text = self.as_str().as_bytes();
        if text.len() < 3 || !text.iter().all(u8::is_ascii_digit) {
            return None;
        }
        let count = text
            .iter()
            .fold(0_u32, |count, digit| count * 10 + u32::from(digit - b'0'));
        if count >= 4096 || (text.len() > 3 && count < 1000) {
            return None;
        }
        Some(count as u16)
    }

    /// Formats a counted number, padded to the three digits MMSSTV prints.
    pub(crate) fn from_count(count: u16) -> Self {
        let mut reversed = [0; MAX_NUMBER_LEN];
        let mut len = 0;
        let mut remaining = count;
        while remaining > 0 || len < 3 {
            reversed[len] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
            len += 1;
        }
        let mut bytes = [0; MAX_NUMBER_LEN];
        for (byte, digit) in bytes.iter_mut().zip(reversed[..len].iter().rev()) {
            *byte = *digit;
        }
        Self {
            bytes,
            len: len as u8,
        }
    }
}

impl fmt::Display for FskNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
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

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::string::ToString;
    use rstest::rstest;

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
}
