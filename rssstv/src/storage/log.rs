//! Diagnostics that survive a release build.
//!
//! A release build is linked as a Windows GUI subsystem executable, which
//! leaves the process with no console: anything written to standard error is
//! discarded, and the reports that matter most — a device that vanished, a
//! menu that could not be installed — are exactly the ones nobody would see.
//! Messages are appended to a file instead, and echoed to standard error when
//! there is a console to read it.
//!
//! The sink is process-wide because the reports come from wherever they
//! happen rather than from one owner, and because a diagnostic must never be
//! the reason a call site has to thread state through itself. Losing a message
//! is always preferable to failing the work that produced it, so every
//! operation here is best-effort and silent about its own failures.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write as _},
    path::Path,
    sync::{Mutex, OnceLock},
};

use jiff::Zoned;

/// How large the file may grow before it is started again, in bytes.
///
/// A session that runs for weeks with a misbehaving device would otherwise
/// write without bound. One turnover is kept, so the file that filled up is
/// still readable after the rotation.
const MAX_BYTES: u64 = 1 << 20;

static SINK: OnceLock<Mutex<File>> = OnceLock::new();

/// Opens the log file, replacing it when the last session filled it.
///
/// Returns the error rather than reporting it, because at the point this is
/// called there is nowhere to report it to yet.
pub fn open(path: &Path) -> io::Result<()> {
    if path.metadata().is_ok_and(|data| data.len() >= MAX_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }

    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // A second call would mean two sinks for one process; the first wins and
    // the caller is told nothing changed, which is what it asked for anyway.
    let _ = SINK.set(Mutex::new(file));
    note(&format!(
        "RSSSTV {} starting on {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS
    ));
    Ok(())
}

/// Records `message`, timestamped in the operator's own time zone.
pub fn note(message: &str) {
    let line = format!("{} {message}", timestamp());

    // A console is worth writing to when there is one: a developer running the
    // debug build should not have to open the file to see what happened.
    eprintln!("{line}");

    let Some(sink) = SINK.get() else {
        return;
    };
    // A poisoned lock means another thread panicked mid-write. The file is
    // still usable and the message is still worth keeping, so the guard is
    // taken either way.
    let mut file = match sink.lock() {
        Ok(file) => file,
        Err(poisoned) => poisoned.into_inner(),
    };
    let _ = writeln!(file, "{line}");
    let _ = file.flush();
}

/// The current local time, or the offset from the epoch when the platform has
/// no time zone this can be resolved against.
fn timestamp() -> String {
    Zoned::now().strftime("%Y-%m-%d %H:%M:%S%:z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_dated_and_offset() {
        let stamp = timestamp();
        assert_eq!(stamp.len(), "2026-08-04 21:43:07+09:00".len(), "{stamp}");
        assert_eq!(stamp.as_bytes()[4], b'-');
        assert_eq!(stamp.as_bytes()[13], b':');
    }

    /// Nothing may panic before the sink exists: the earliest reports arrive
    /// while the application is still working out where to put the file.
    #[test]
    fn a_note_without_a_sink_is_discarded() {
        note("this should not panic");
    }
}
