//! What the application reads from and writes to disk.
//!
//! The directories it owns, the settings that survive a restart, the received
//! images it keeps, and the diagnostics it leaves behind.

pub mod config;
pub mod history;
pub mod log;
pub mod paths;
