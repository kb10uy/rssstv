//! Rig transports for RSSSTV.
//!
//! This crate is how a rig is reached, not what it is told. A station keys its
//! rig in more ways than one protocol covers — a Hamlib socket, raw CI-V bytes
//! on a serial line, an external plugin driving DTR — and the three share no
//! operation worth modelling once. So the transports live here and the policy
//! lives in the operator's own script, which the application hosts.
//!
//! The Hamlib transport talks to a `rigctld` the operator already has running
//! rather than to `libhamlib` linked in. That keeps a C build out of every
//! platform's toolchain, and it leaves the serial port free for the logger and
//! everything else the station runs at the same time, because sharing one rig
//! between programs is what `rigctld` is for.

mod band;
mod error;
mod rigctld;

pub use band::{Band, Reading};
pub use error::RigError;
pub use rigctld::{DEFAULT_ADDRESS, DEFAULT_TIMEOUT, Response, Rigctld};
