use rssstv_audio::{
    AudioHost, Capture, InputDevice, OutputDevice, Playback, PlaybackWriter, StreamFault,
};
use rssstv_demodulator::SyncStart;

use crate::worker::receive::{HistoryCandidate, Snapshot, Worker};

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
    pub output_devices: Vec<OutputDevice>,
    pub output_device: Option<OutputDevice>,
    pub error: Option<String>,
    capture: Option<Capture>,
    worker: Option<Worker>,
    snapshot: Snapshot,
    slant: bool,
    vis_restart: bool,
    sync_start: SyncStart,
}

impl AudioState {
    /// Opens `preferred` when the host still offers a device by that name.
    ///
    /// The name is matched rather than an identifier because the host assigns
    /// identifiers per run; a device that disappeared since the last session
    /// falls back to the host default.
    pub fn new(
        preferred: Option<&str>,
        preferred_output: Option<&str>,
        slant: bool,
        vis_restart: bool,
    ) -> Self {
        let host = AudioHost::new();
        let (devices, input_error) = match host.input_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        let (output_devices, output_error) = match host.output_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        let device = preferred
            .and_then(|name| devices.iter().find(|device| device.name() == name).cloned())
            .or_else(|| {
                host.default_input_device()
                    .filter(|device| devices.contains(device))
            })
            .or_else(|| devices.first().cloned());
        let output_device = preferred_output
            .and_then(|name| {
                output_devices
                    .iter()
                    .find(|device| device.name() == name)
                    .cloned()
            })
            .or_else(|| {
                host.default_output_device()
                    .filter(|device| output_devices.contains(device))
            })
            .or_else(|| output_devices.first().cloned());
        let error = [input_error, output_error]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let mut state = Self {
            host,
            devices,
            device: device.clone(),
            output_devices,
            output_device,
            error: (!error.is_empty()).then(|| error.join("; ")),
            capture: None,
            worker: None,
            snapshot: Snapshot::default(),
            slant,
            vis_restart,
            sync_start: SyncStart::default(),
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
            output_devices: Vec::new(),
            output_device: None,
            error: None,
            capture: None,
            worker: None,
            snapshot: Snapshot::default(),
            slant: true,
            vis_restart: true,
            sync_start: SyncStart::default(),
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

    pub fn select_output(&mut self, device: OutputDevice) {
        self.output_device = Some(device);
    }

    pub fn open_playback(
        &self,
        capacity_samples: usize,
    ) -> Result<(Playback, PlaybackWriter), String> {
        let device = self
            .output_device
            .as_ref()
            .ok_or_else(|| "no output device is selected".to_owned())?;
        self.host
            .open_playback(device, capacity_samples)
            .map_err(|error| error.to_string())
    }

    fn open(&mut self, device: &InputDevice) {
        // The worker is stopped before the device is reopened so the previous
        // capture queue never outlives its producer.
        self.worker = None;
        self.capture = None;
        self.snapshot = Snapshot::default();
        match self.host.open_capture(device, QUEUE_CAPACITY_SAMPLES) {
            Ok((capture, reader)) => {
                self.worker = Some(Worker::spawn(
                    reader,
                    self.slant,
                    self.vis_restart,
                    self.sync_start,
                ));
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
    pub fn poll(&mut self) -> Option<crate::worker::receive::Frame> {
        let worker = self.worker.as_ref()?;
        let mut snapshot = worker.latest()?;
        let frame = snapshot.frame.take();
        self.snapshot = snapshot;
        frame
    }

    pub const fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn take_history(&mut self) -> Option<HistoryCandidate> {
        self.snapshot.history.take()
    }

    pub fn set_slant(&mut self, enabled: bool) {
        self.slant = enabled;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_slant(enabled);
        }
    }

    /// Chooses whether a VIS header may start a reception over.
    pub fn set_vis_restart(&mut self, enabled: bool) {
        if self.vis_restart == enabled {
            return;
        }
        self.vis_restart = enabled;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_vis_restart(enabled);
        }
    }

    /// Chooses whether a reception may start without a VIS header.
    ///
    /// Kept here as well as pushed to the worker, because a device opened
    /// later spawns a worker that has to start with the same scope.
    pub fn set_sync_start(&mut self, scope: SyncStart) {
        if self.sync_start == scope {
            return;
        }
        self.sync_start = scope;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_sync_start(scope);
        }
    }

    #[cfg(test)]
    pub const fn sync_start(&self) -> SyncStart {
        self.sync_start
    }

    #[cfg(test)]
    pub const fn slant(&self) -> bool {
        self.slant
    }

    #[cfg(test)]
    pub const fn vis_restart(&self) -> bool {
        self.vis_restart
    }

    /// Returns the physical capture rate, if a device is open.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.capture.as_ref().map(Capture::sample_rate_hz)
    }

    /// Returns whether a device is currently delivering samples.
    pub const fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Takes the report the capture stream left if it stopped on its own.
    ///
    /// The stream is dropped along with the worker reading from it: a device
    /// that reported a fault will not deliver again, and leaving the handle in
    /// place would let the interface keep claiming to be capturing.
    pub fn take_capture_fault(&mut self) -> Option<StreamFault> {
        let fault = self.capture.as_ref()?.take_fault()?;
        self.worker = None;
        self.capture = None;
        self.snapshot = Snapshot::default();
        Some(fault)
    }

    /// Enumerates devices again, keeping the selection when it survived.
    ///
    /// Called after a device is lost, so the lists the operator picks from
    /// describe what is actually attached now.
    pub fn rescan(&mut self) {
        if let Ok(devices) = self.host.input_devices() {
            self.devices = devices;
        }
        if let Ok(devices) = self.host.output_devices() {
            self.output_devices = devices;
        }
        if !self
            .device
            .as_ref()
            .is_some_and(|device| self.devices.contains(device))
        {
            self.device = None;
        }
        if !self
            .output_device
            .as_ref()
            .is_some_and(|device| self.output_devices.contains(device))
        {
            self.output_device = None;
        }
    }

    /// Opens the selected device again after a fault.
    ///
    /// Returns whether capture is running afterwards, so the caller can leave
    /// the report in front of the operator when it is not.
    pub fn reopen(&mut self) -> bool {
        let Some(device) = self.device.clone() else {
            return false;
        };
        self.open(&device);
        self.is_capturing()
    }
}

impl Default for AudioState {
    fn default() -> Self {
        Self::new(None, None, true, true)
    }
}

impl core::fmt::Debug for AudioState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AudioState")
            .field("device", &self.device)
            .field("capturing", &self.is_capturing())
            .field("output_device", &self.output_device)
            .finish_non_exhaustive()
    }
}
