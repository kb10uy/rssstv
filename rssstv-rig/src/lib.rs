//! The rig transport for RSSSTV.
//!
//! This crate is how a rig is reached, not what it is told. What to send is the
//! operator's own script, hosted by the application, because what a rig wants
//! around a transmission is a property of the rig and the station around it.
//!
//! There is one way to reach a rig and it is `rigctld`: Hamlib already speaks
//! CI-V and drives DTR and RTS, so the keying arrangements that other SSTV
//! software configures separately are all one socket here.
//!
//! Talking to a `rigctld` the operator already has running, rather than to
//! `libhamlib` linked in, keeps a C build out of every platform's toolchain and
//! leaves the serial port free for the logger and everything else the station
//! runs at the same time, because sharing one rig between programs is what
//! `rigctld` is for.

mod error;
mod rigctld;

pub use error::RigError;
pub use rigctld::{DEFAULT_ADDRESS, DEFAULT_TIMEOUT, Response, Rigctld};
