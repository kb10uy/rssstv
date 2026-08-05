//! MMSSTV-compatible FSKID protocol encoding and decoding.

#![no_std]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

mod decoder;
mod encoder;
mod error;
mod id;

pub use decoder::{FskDecoder, FskRecord};
pub use encoder::{FskEncoder, FskTxEvent, FskTxTone};
pub use error::FskIdError;
pub use id::{FskId, FskNumber, FskTone};
