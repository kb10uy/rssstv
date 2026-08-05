use core::fmt;

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

/// An error returned when contest number text cannot be represented by FSKID.
///
/// The number record has an alphabet and a length of its own, both narrower
/// than the identifier's, so it reports against its own limits.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FskNumberError {
    /// The number is empty.
    Empty,
    /// The number is longer than the 8-byte protocol limit.
    TooLong,
    /// A byte is outside the protocol number alphabet.
    InvalidByte {
        /// The byte offset in the supplied text.
        index: usize,
        /// The invalid byte value.
        byte: u8,
    },
}

impl fmt::Display for FskNumberError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("an FSKID number must not be empty"),
            Self::TooLong => formatter.write_str("an FSKID number must not exceed 8 bytes"),
            Self::InvalidByte { index, byte } => write!(
                formatter,
                "invalid FSKID number byte 0x{byte:02x} at offset {index}"
            ),
        }
    }
}

impl core::error::Error for FskNumberError {}
