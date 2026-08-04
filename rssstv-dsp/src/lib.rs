//! Allocation-backed, `no_std` signal-processing primitives for RSSSTV.
//!
//! Processors own their state and allocate only during construction or explicit
//! reconfiguration. Per-sample and in-place block processing do not allocate.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

/// Errors reported while validating DSP configurations.
pub mod error;
/// Finite and infinite impulse response filters, and narrow-band resonators.
pub mod filter;
/// Zero-crossing and phase-locked frequency measurement.
pub mod frequency;
/// Oscillators and voltage-controlled oscillators.
pub mod oscillator;
/// Discrete Fourier and Hilbert transforms.
pub mod transform;

mod validate;

pub use error::DspError;
