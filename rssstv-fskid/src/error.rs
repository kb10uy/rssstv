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
