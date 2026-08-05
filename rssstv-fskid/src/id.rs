use core::{fmt, str};

use crate::{FskEncoder, FskIdError};

pub(crate) const MAX_ID_LEN: usize = 16;
pub(crate) const MAX_NUMBER_LEN: usize = 9;

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
