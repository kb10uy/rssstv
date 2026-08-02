use rssstv_audio::{AudioHost, Capture, InputDevice};

/// Samples drained from the capture queue per pass.
const READ_BUFFER_SAMPLES: usize = 4_096;

/// One second of queue at the preferred capture rate.
const QUEUE_CAPACITY_SAMPLES: usize = 48_000;

/// Fraction of the previous level retained when the signal falls.
///
/// The meter follows a rising signal immediately and decays gradually, so
/// short peaks stay readable instead of flickering.
const RELEASE: f32 = 0.88;

/// Live capture session driving the input level meter.
///
/// Decoding is not connected yet; this only proves the capture path and gives
/// the operator a real level reading.
pub struct AudioState {
    host: AudioHost,
    pub devices: Vec<InputDevice>,
    pub device: Option<InputDevice>,
    pub error: Option<String>,
    capture: Option<Capture>,
    buffer: Vec<f32>,
    level: f32,
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
            buffer: vec![0.0; READ_BUFFER_SAMPLES],
            level: 0.0,
        };
        if let Some(device) = device {
            state.open(&device);
        }
        state
    }

    /// Switches capture to `device`, replacing any existing session.
    pub fn select(&mut self, device: InputDevice) {
        self.device = Some(device.clone());
        self.open(&device);
    }

    fn open(&mut self, device: &InputDevice) {
        self.capture = None;
        self.level = 0.0;
        match self.host.open_capture(device, QUEUE_CAPACITY_SAMPLES) {
            Ok(capture) => {
                self.capture = Some(capture);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Drains everything the device has produced and updates the meter.
    pub fn poll(&mut self) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        let mut peak = 0.0_f32;
        loop {
            let reading = capture.read(&mut self.buffer);
            if reading.count == 0 {
                break;
            }
            peak = peak.max(block_peak(&self.buffer[..reading.count]));
        }
        self.level = follow_peak(self.level, peak, RELEASE);
    }

    /// Returns the current meter value in `0.0..=1.0`.
    pub const fn level(&self) -> f32 {
        self.level
    }

    /// Returns the physical capture rate, if a device is open.
    pub fn sample_rate_hz(&self) -> Option<u32> {
        self.capture.as_ref().map(Capture::sample_rate_hz)
    }

    /// Returns the number of samples lost to queue overrun.
    pub fn dropped_samples(&self) -> u64 {
        self.capture.as_ref().map_or(0, Capture::dropped_samples)
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

fn block_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

/// Tracks `peak` instantly upward and decays toward it by `release`.
fn follow_peak(current: f32, peak: f32, release: f32) -> f32 {
    if peak >= current {
        peak.clamp(0.0, 1.0)
    } else {
        (current * release).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn peak_uses_absolute_amplitude() {
        assert_eq!(block_peak(&[0.1, -0.8, 0.3]), 0.8);
        assert_eq!(block_peak(&[]), 0.0);
    }

    #[test]
    fn rising_signals_are_followed_immediately() {
        assert_eq!(follow_peak(0.2, 0.9, RELEASE), 0.9);
    }

    #[test]
    fn falling_signals_decay_gradually() {
        let decayed = follow_peak(0.8, 0.0, 0.5);
        assert_eq!(decayed, 0.4);
        assert!(decayed < 0.8);
    }

    #[rstest]
    #[case(2.0, 0.0)]
    #[case(0.5, 3.0)]
    fn meter_values_stay_normalized(#[case] current: f32, #[case] peak: f32) {
        let level = follow_peak(current, peak, RELEASE);
        assert!((0.0..=1.0).contains(&level), "{level} is out of range");
    }

    #[test]
    fn silence_decays_toward_zero() {
        let mut level = 1.0;
        for _ in 0..200 {
            level = follow_peak(level, 0.0, RELEASE);
        }
        assert!(level < 1.0e-6, "{level} did not decay");
    }
}
