use rssstv_audio::{AudioHost, Capture, InputDevice};

use crate::receive::{Snapshot, Worker};

/// One second of queue at the preferred capture rate.
const QUEUE_CAPACITY_SAMPLES: usize = 48_000;

/// Device selection and the receive worker running on it.
///
/// The [`Capture`] handle stays here because the host stream is bound to the
/// thread that opened it; the reading half is moved into the worker.
pub struct AudioState {
    host: AudioHost,
    pub devices: Vec<InputDevice>,
    pub device: Option<InputDevice>,
    pub error: Option<String>,
    capture: Option<Capture>,
    worker: Option<Worker>,
    snapshot: Snapshot,
    slant: bool,
}

impl AudioState {
    pub fn new() -> Self {
        let host = AudioHost::new();
        let (devices, error) = match host.input_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        let device = host
            .default_input_device()
            .filter(|device| devices.contains(device))
            .or_else(|| devices.first().cloned());
        let mut state = Self {
            host,
            devices,
            device: device.clone(),
            error,
            capture: None,
            worker: None,
            snapshot: Snapshot::default(),
            slant: true,
        };
        if let Some(device) = device {
            state.open(&device);
        }
        state
    }

    /// Builds a state that never touches the host.
    ///
    /// Tests drive the interface without enumerating or opening devices, so
    /// they stay deterministic and do not depend on the machine's hardware.
    #[cfg(test)]
    pub fn disconnected() -> Self {
        Self {
            host: AudioHost::new(),
            devices: Vec::new(),
            device: None,
            error: None,
            capture: None,
            worker: None,
            snapshot: Snapshot::default(),
            slant: true,
        }
    }

    /// Replaces the observed snapshot without a running worker.
    #[cfg(test)]
    pub fn set_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
    }

    /// Switches capture to `device`, replacing any running session.
    pub fn select(&mut self, device: InputDevice) {
        self.device = Some(device.clone());
        self.open(&device);
    }

    fn open(&mut self, device: &InputDevice) {
        // The worker is stopped before the device is reopened so the previous
        // capture queue never outlives its producer.
        self.worker = None;
        self.capture = None;
        self.snapshot = Snapshot::default();
        match self.host.open_capture(device, QUEUE_CAPACITY_SAMPLES) {
            Ok((capture, reader)) => {
                self.worker = Some(Worker::spawn(reader, self.slant));
                self.capture = Some(capture);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Adopts the newest worker snapshot.
    ///
    /// Returns the decoded frame when one arrived, so the caller can refresh
    /// the raster without cloning pixels on every poll.
    pub fn poll(&mut self) -> Option<crate::receive::Frame> {
        let worker = self.worker.as_ref()?;
        let mut snapshot = worker.latest()?;
        let frame = snapshot.frame.take();
        self.snapshot = snapshot;
        frame
    }

    pub const fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn set_slant(&mut self, enabled: bool) {
        self.slant = enabled;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_slant(enabled);
        }
    }

    #[cfg(test)]
    pub const fn slant(&self) -> bool {
        self.slant
    }

    /// Returns the physical capture rate, if a device is open.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.capture.as_ref().map(Capture::sample_rate_hz)
    }

    /// Returns whether a device is currently delivering samples.
    pub const fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for AudioState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AudioState")
            .field("device", &self.device)
            .field("capturing", &self.is_capturing())
            .finish_non_exhaustive()
    }
}
