//! Allocation-backed, `no_std` signal-processing primitives for RSSSTV.
//!
//! Processors own their state and allocate only during construction or explicit
//! reconfiguration. Per-sample and in-place block processing do not allocate.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

/// Errors reported while validating DSP configurations.
pub mod error;
/// Discrete Fourier transforms and windowed spectra.
pub mod fft;
/// Finite impulse response filters and Hilbert transforms.
pub mod fir;
/// Frequency measurement primitives.
pub mod frequency;
/// Infinite impulse response filter design and processing.
pub mod iir;
/// Oscillators and voltage-controlled oscillators.
pub mod oscillator;
/// Narrow-band resonators.
pub mod resonator;

pub use error::DspError;
