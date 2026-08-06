use rssstv_audio::{
    AudioHost, Capture, InputDevice, OutputDevice, Playback, PlaybackWriter, StreamFault,
};
use rssstv_demodulator::SyncStart;

use crate::{
    error::AppError,
    worker::{
        Waker,
        receive::{HistoryCandidate, RxSnapshot, RxWorker},
        transmit::{TxSnapshot, TxWorker},
    },
};

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
    pub error: Option<AppError>,
    capture: Option<Capture>,
    worker: Option<RxWorker>,
    snapshot: RxSnapshot,
    /// Counts the receive sessions this state has started.
    ///
    /// A new worker starts its decoded lists over, so anything following those
    /// lists has to know it is looking at a new session rather than at the
    /// previous one grown or shrunk.
    session: u64,
    muted_for_transmit: bool,
    live_slant: bool,
    vis_restart: bool,
    sync_start: SyncStart,
    /// Handed to every worker opened here, so a decoded row reaches the canvas
    /// without the interface having to redraw on the chance that one has.
    waker: Waker,
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
        live_slant: bool,
        vis_restart: bool,
        waker: Waker,
    ) -> Self {
        let host = AudioHost::new();
        let (devices, input_error) = match host.input_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.into())),
        };
        let (output_devices, output_error) = match host.output_devices() {
            Ok(devices) => (devices, None),
            Err(error) => (Vec::new(), Some(error.into())),
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
        // Both halves of the host failing at once says the same thing twice,
        // so the first answer stands for the pair.
        let error = input_error.or(output_error);
        let mut state = Self {
            host,
            devices,
            device: device.clone(),
            output_devices,
            output_device,
            error,
            capture: None,
            worker: None,
            snapshot: RxSnapshot::default(),
            session: 0,
            muted_for_transmit: false,
            live_slant,
            vis_restart,
            sync_start: SyncStart::default(),
            waker,
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
            snapshot: RxSnapshot::default(),
            session: 0,
            muted_for_transmit: false,
            live_slant: true,
            vis_restart: true,
            sync_start: SyncStart::default(),
            waker: Waker::default(),
        }
    }

    /// Identifies the receive session the current snapshot belongs to.
    pub const fn session(&self) -> u64 {
        self.session
    }

    /// Replaces the observed snapshot without a running worker.
    #[cfg(test)]
    pub fn set_snapshot(&mut self, snapshot: RxSnapshot) {
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
    ) -> Result<(Playback, PlaybackWriter), AppError> {
        let device = self
            .output_device
            .as_ref()
            .ok_or(AppError::NoOutputDevice)?;
        Ok(self.host.open_playback(device, capacity_samples)?)
    }

    fn open(&mut self, device: &InputDevice) {
        // The worker is stopped before the device is reopened so the previous
        // capture queue never outlives its producer.
        self.worker = None;
        self.capture = None;
        self.snapshot = RxSnapshot::default();
        self.session += 1;
        match self.host.open_capture(device, QUEUE_CAPACITY_SAMPLES) {
            Ok((capture, reader)) => {
                let worker = RxWorker::spawn(
                    reader,
                    self.live_slant,
                    self.vis_restart,
                    self.sync_start,
                    self.waker.clone(),
                );
                // A device opened while the station is transmitting is muted
                // exactly like the one it replaces.
                worker.set_muted_for_transmit(self.muted_for_transmit);
                self.worker = Some(worker);
                self.capture = Some(capture);
                self.error = None;
            }
            Err(error) => self.error = Some(error.into()),
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

    pub const fn snapshot(&self) -> &RxSnapshot {
        &self.snapshot
    }

    pub fn take_history(&mut self) -> Option<HistoryCandidate> {
        self.snapshot.history.take()
    }

    /// Stops or resumes decoding without closing the device.
    ///
    /// Reception is muted while the station transmits: its own signal comes
    /// straight back off the antenna, and what would be decoded from it is the
    /// picture it is sending. Kept here as well as pushed to the worker,
    /// because a device opened later has to start out the same way.
    pub fn set_muted_for_transmit(&mut self, muted: bool) {
        if self.muted_for_transmit == muted {
            return;
        }
        self.muted_for_transmit = muted;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_muted_for_transmit(muted);
        }
    }

    /// Returns whether reception is currently stopped.
    pub const fn is_muted_for_transmit(&self) -> bool {
        self.muted_for_transmit
    }

    pub fn set_live_slant(&mut self, enabled: bool) {
        self.live_slant = enabled;
        if let Some(worker) = self.worker.as_ref() {
            worker.set_live_slant(enabled);
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
    pub const fn live_slant(&self) -> bool {
        self.live_slant
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
        self.snapshot = RxSnapshot::default();
        self.session += 1;
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

/// The playback device and worker one transmission runs on.
///
/// The receive half keeps its device and worker together in [`AudioState`];
/// this is the transmit mirror, so the orchestrator holds one facade per
/// direction instead of spreading the transmit device across its own fields.
#[derive(Default)]
pub struct TxState {
    playback: Option<Playback>,
    /// Whether the device was told to start consuming the queue.
    started: bool,
    worker: Option<TxWorker>,
}

impl TxState {
    /// Adopts the stream a transmission is about to fill.
    pub fn begin(&mut self, playback: Playback) {
        self.playback = Some(playback);
        self.started = false;
    }

    /// Adopts the worker filling that stream.
    pub fn attach_worker(&mut self, worker: TxWorker) {
        self.worker = Some(worker);
    }

    /// Reads the newest worker snapshot, if a worker is running.
    pub fn latest(&self) -> Option<TxSnapshot> {
        self.worker.as_ref().map(TxWorker::latest)
    }

    /// Asks the device to start consuming the queue.
    pub fn start_playback(&mut self) -> Result<(), AppError> {
        self.playback
            .as_ref()
            .ok_or(AppError::PlaybackClosed)?
            .play()?;
        self.started = true;
        Ok(())
    }

    /// Whether the device has been told to start.
    pub const fn is_started(&self) -> bool {
        self.started
    }

    /// Whether the device ran the queue dry while a picture was going out.
    pub fn has_underrun(&self) -> bool {
        self.started
            && self
                .playback
                .as_ref()
                .is_some_and(|playback| playback.underrun_samples() > 0)
    }

    /// Whether the queue was closed by the worker and played to its end.
    pub fn is_drained(&self) -> bool {
        self.playback.as_ref().is_some_and(Playback::is_complete)
    }

    /// Returns how many samples the device has actually played.
    pub fn played_samples(&self) -> u64 {
        self.playback.as_ref().map_or(0, Playback::played_samples)
    }

    /// Returns the physical playback rate, if a stream is open.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.playback.as_ref().map(Playback::sample_rate_hz)
    }

    /// Releases the stream and the worker, however the transmission ended.
    pub fn stop(&mut self) {
        self.playback = None;
        self.worker = None;
        self.started = false;
    }
}

impl core::fmt::Debug for TxState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxState")
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for AudioState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("AudioState")
            .field("device", &self.device)
            .field("capturing", &self.is_capturing())
            .field("muted_for_transmit", &self.muted_for_transmit)
            .field("output_device", &self.output_device)
            .finish_non_exhaustive()
    }
}
