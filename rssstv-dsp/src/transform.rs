//! Conversions of a signal into another representation of itself.

mod fft;
mod hilbert;

pub use fft::{Complex, Fft, FftDirection, RealSpectrum, SpectrumWindow};
pub use hilbert::{AnalyticSample, HilbertTransformer, hilbert_coefficients};
