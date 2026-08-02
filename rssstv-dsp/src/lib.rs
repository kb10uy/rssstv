#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod error;
pub mod fir;
pub mod iir;
pub mod resonator;

pub use error::DspError;
