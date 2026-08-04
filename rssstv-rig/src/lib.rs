//! Rig control adapters for RSSSTV.
//!
//! Control runs through a `rigctld` the operator already has running, rather
//! than through Hamlib linked into this application. That keeps a C library
//! out of the build on every platform, and it leaves the serial port free for
//! the logger and everything else the station runs at the same time, because
//! sharing one rig between programs is what `rigctld` is for.
//!
//! What gets sent is the operator's to write. This crate knows the moments a
//! transmission passes through and the band the rig is on; the commands those
//! moments send come from the configuration file, because what a rig needs
//! around a transmission differs by rig and by station.

mod band;
mod command;
mod error;
mod rigctld;
mod session;

pub use band::Band;
pub use command::{Command, Event, Script};
pub use error::RigError;
pub use rigctld::{DEFAULT_ADDRESS, DEFAULT_TIMEOUT, Response, Rigctld};
pub use session::{Reading, Session};
