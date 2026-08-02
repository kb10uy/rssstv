//! Streaming receive decoding for supported conventional SSTV raster families.

mod acquisition;
mod clock;
mod config;
mod decoder;
mod event;
mod input;
mod raster;
mod slant;
mod sync;

pub use config::{RxConfig, Staging};
pub use decoder::{RefinementResult, RxDecoder};
pub use event::{RxEvent, RxOutcome, RxProcess, RxState, StopReason};
pub use input::DemodulatedBlock;
pub use slant::{SlantEstimate, SlantEstimator};
pub use sync::SyncObservation;
