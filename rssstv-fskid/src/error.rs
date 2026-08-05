use thiserror::Error;

/// An error returned when station identifier text cannot be represented by FSKID.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
pub enum FskIdError {
    /// The identifier is empty.
    #[error("FSKID must not be empty")]
    Empty,
    /// The identifier is longer than the 16-byte protocol limit.
    #[error("FSKID must not exceed 16 bytes")]
    TooLong,
    /// A byte is outside the protocol text alphabet.
    #[error("invalid FSKID byte 0x{byte:02x} at offset {index}")]
    InvalidByte {
        /// The byte offset in the supplied text.
        index: usize,
        /// The invalid byte value.
        byte: u8,
    },
}

/// An error returned when contest number text cannot be represented by FSKID.
///
/// The number record has an alphabet and a length of its own, both narrower
/// than the identifier's, so it reports against its own limits.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
pub enum FskNumberError {
    /// The number is empty.
    #[error("an FSKID number must not be empty")]
    Empty,
    /// The number is longer than the 8-byte protocol limit.
    #[error("an FSKID number must not exceed 8 bytes")]
    TooLong,
    /// A byte is outside the protocol number alphabet.
    #[error("invalid FSKID number byte 0x{byte:02x} at offset {index}")]
    InvalidByte {
        /// The byte offset in the supplied text.
        index: usize,
        /// The invalid byte value.
        byte: u8,
    },
}
