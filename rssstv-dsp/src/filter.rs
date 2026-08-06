//! Filters that shape a signal's spectrum while leaving its domain unchanged.

mod fir;
mod iir;
mod resonator;

pub use fir::{Fir, FirDesign, FirKind};
pub use iir::{Iir, IirLowPassDesign, IirResponse, SosCoefficients};
pub use resonator::Resonator;
