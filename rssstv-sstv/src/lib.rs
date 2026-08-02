//! Allocation-backed, `no_std` SSTV protocol primitives for RSSSTV.
//!
//! This crate contains mode, image, color, timing, signal, and streaming
//! transmit and receive types. It deliberately does not depend on audio or
//! user-interface implementations.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

/// MMSSTV-compatible color conversion.
pub mod color;
/// Errors produced while validating SSTV values.
pub mod error;
/// Allocation-backed RGB images.
pub mod image;
/// SSTV mode inventory and protocol metadata.
pub mod mode;
/// Streaming raster acquisition and receive decoding.
pub mod rx;
/// Frequencies and timed transmit tones.
pub mod signal;
/// Exact protocol durations and transmit deadlines.
pub mod time;
/// Pull-based SSTV transmit encoding.
pub mod tx;

pub use error::SstvError;
pub use rx::RxDecoder;
pub use tx::TxEncoder;
