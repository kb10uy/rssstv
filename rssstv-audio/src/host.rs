use std::{
    fmt,
    sync::{Arc, atomic::AtomicU64},
};

use cpal::{
    Host, SampleFormat, StreamConfig,
    traits::{DeviceTrait, HostTrait},
};
use ringbuf::{HeapRb, traits::Split};

use crate::{
    AudioError, Capture, CaptureReader, FaultKind, FaultSlot, InputDevice, MINIMUM_SAMPLE_RATE_HZ,
    OutputDevice, Playback, PlaybackWriter, StreamFault, capture,
    device::{describe, named, preferred_output_buffer_size, preferred_output_rate, preferred_rate},
    playback,
};

/// Enumerates and opens host audio devices.
pub struct AudioHost {
    host: Host,
}

impl AudioHost {
    /// Connects to the platform's default host API.
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Lists usable input devices.
    ///
    /// Devices that cannot report an identifier, a name, or a configuration
    /// are skipped rather than failing the whole enumeration, because one
    /// broken device should not hide the rest.
    pub fn input_devices(&self) -> Result<Vec<InputDevice>, AudioError> {
        let devices = self
            .host
            .input_devices()
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        let described = devices
            .filter(|device| device.default_input_config().is_ok())
            .filter_map(|device| describe(&device))
            .collect();
        Ok(named(described)
            .map(|(id, name)| InputDevice { id, name })
            .collect())
    }

    /// Returns the host's default input device.
    ///
    /// The default is looked up in the enumeration rather than described on
    /// its own, because a name is only distinct against the rest of the list
    /// and one described alone would not match the entry the operator sees.
    /// A default the crate cannot open is reported as no default at all.
    pub fn default_input_device(&self) -> Option<InputDevice> {
        let id = self.host.default_input_device()?.id().ok()?;
        self.input_devices()
            .ok()?
            .into_iter()
            .find(|device| device.id == id)
    }

    /// Lists usable playback devices.
    pub fn output_devices(&self) -> Result<Vec<OutputDevice>, AudioError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|error| AudioError::Backend(error.to_string()))?;
        let described = devices
            .filter(|device| device.default_output_config().is_ok())
            .filter_map(|device| describe(&device))
            .collect();
        Ok(named(described)
            .map(|(id, name)| OutputDevice { id, name })
            .collect())
    }

    /// Returns the host's default playback device.
    pub fn default_output_device(&self) -> Option<OutputDevice> {
        let id = self.host.default_output_device()?.id().ok()?;
        self.output_devices()
            .ok()?
            .into_iter()
            .find(|device| device.id == id)
    }

    /// Opens `device` for capture and starts delivery.
    ///
    /// `capacity_samples` bounds the queue between the audio callback and the
    /// reader. When the reader falls behind, samples are dropped and counted
    /// rather than allowed to grow without limit.
    pub fn open_capture(
        &self,
        device: &InputDevice,
        capacity_samples: usize,
    ) -> Result<(Capture, CaptureReader), AudioError> {
        if capacity_samples == 0 {
            return Err(AudioError::EmptyCapacity);
        }
        let target = self
            .host
            .device_by_id(&device.id)
            .ok_or_else(|| AudioError::DeviceNotFound(device.name.clone()))?;

        let supported = target
            .default_input_config()
            .map_err(|_| AudioError::UnsupportedConfiguration(device.name.clone()))?;
        let sample_format = supported.sample_format();
        let channels = supported.channels();
        let sample_rate = preferred_rate(&target, supported.sample_rate());
        if sample_rate < MINIMUM_SAMPLE_RATE_HZ || channels == 0 {
            return Err(AudioError::UnsupportedConfiguration(device.name.clone()));
        }

        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let (producer, consumer) = HeapRb::<f32>::new(capacity_samples).split();
        let dropped = Arc::new(AtomicU64::new(0));
        let faults = FaultSlot::default();
        let report = fault_reporter(&faults, &device.name);
        let lanes = channels as usize;

        let stream = match sample_format {
            SampleFormat::F32 => target.build_input_stream(
                config,
                capture::capture_callback::<f32>(lanes, producer, Arc::clone(&dropped)),
                report,
                None,
            ),
            SampleFormat::I16 => target.build_input_stream(
                config,
                capture::capture_callback::<i16>(lanes, producer, Arc::clone(&dropped)),
                report,
                None,
            ),
            SampleFormat::U16 => target.build_input_stream(
                config,
                capture::capture_callback::<u16>(lanes, producer, Arc::clone(&dropped)),
                report,
                None,
            ),
            SampleFormat::I32 => target.build_input_stream(
                config,
                capture::capture_callback::<i32>(lanes, producer, Arc::clone(&dropped)),
                report,
                None,
            ),
            _ => {
                return Err(AudioError::UnsupportedConfiguration(device.name.clone()));
            }
        }
        .map_err(|error| AudioError::Backend(error.to_string()))?;

        let capture = Capture::new(stream, sample_rate, channels, faults);
        capture.play()?;
        Ok((capture, CaptureReader::new(consumer, dropped, sample_rate)))
    }

    /// Opens `device` with a bounded mono queue, initially paused.
    pub fn open_playback(
        &self,
        device: &OutputDevice,
        capacity_samples: usize,
    ) -> Result<(Playback, PlaybackWriter), AudioError> {
        let target = self
            .host
            .device_by_id(&device.id)
            .ok_or_else(|| AudioError::DeviceNotFound(device.name.clone()))?;
        let supported = target
            .default_output_config()
            .map_err(|_| AudioError::UnsupportedConfiguration(device.name.clone()))?;
        let sample_format = supported.sample_format();
        let channels = supported.channels();
        let sample_rate =
            preferred_output_rate(&target, supported.sample_rate(), channels, sample_format);
        if sample_rate < MINIMUM_SAMPLE_RATE_HZ || channels == 0 {
            return Err(AudioError::UnsupportedConfiguration(device.name.clone()));
        }
        let (writer, consumer, state) = playback::queue(sample_rate, capacity_samples)?;
        let config = StreamConfig {
            channels,
            sample_rate,
            buffer_size: preferred_output_buffer_size(&target, sample_rate, channels, sample_format),
        };
        let faults = FaultSlot::default();
        let report = fault_reporter(&faults, &device.name);
        let lanes = channels as usize;
        let callback_state = Arc::clone(&state);
        let stream = match sample_format {
            SampleFormat::F32 => target.build_output_stream(
                config,
                playback::playback_callback::<f32>(lanes, consumer, callback_state),
                report,
                None,
            ),
            SampleFormat::I16 => target.build_output_stream(
                config,
                playback::playback_callback::<i16>(lanes, consumer, callback_state),
                report,
                None,
            ),
            SampleFormat::U16 => target.build_output_stream(
                config,
                playback::playback_callback::<u16>(lanes, consumer, callback_state),
                report,
                None,
            ),
            SampleFormat::I32 => target.build_output_stream(
                config,
                playback::playback_callback::<i32>(lanes, consumer, callback_state),
                report,
                None,
            ),
            _ => return Err(AudioError::UnsupportedConfiguration(device.name.clone())),
        }
        .map_err(|error| AudioError::Backend(error.to_string()))?;
        Ok((
            Playback::new(stream, sample_rate, channels, state, faults),
            writer,
        ))
    }
}

impl Default for AudioHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds the error callback a stream reports through.
///
/// The device name is captured because the callback outlives the borrow of
/// the device, and because a report is only useful if it says which device
/// stopped.
fn fault_reporter(faults: &FaultSlot, device: &str) -> impl FnMut(cpal::Error) + Send + 'static {
    let faults = faults.clone();
    let device = device.to_owned();
    move |error| {
        let Some(kind) = classify(error.kind()) else {
            return;
        };
        faults.report(StreamFault {
            device: device.clone(),
            kind,
            detail: error.to_string(),
        });
    }
}

/// Decides whether an error ends the stream, and how.
///
/// Returning `None` means the stream is still running: a reroute is handled
/// by the host and an overrun costs samples but not the device, so neither is
/// worth interrupting the operator for. The samples an overrun dropped are
/// already counted and reported separately.
const fn classify(kind: cpal::ErrorKind) -> Option<FaultKind> {
    match kind {
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::Xrun => None,
        cpal::ErrorKind::DeviceNotAvailable => Some(FaultKind::Disconnected),
        cpal::ErrorKind::StreamInvalidated => Some(FaultKind::Invalidated),
        _ => Some(FaultKind::Backend),
    }
}

impl fmt::Debug for AudioHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AudioHost").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A reroute and an overrun leave the stream running, so neither may be
    /// reported as a fault: the interface would tell the operator the device
    /// was lost while it was still delivering samples.
    #[rstest]
    #[case(cpal::ErrorKind::DeviceChanged, None)]
    #[case(cpal::ErrorKind::Xrun, None)]
    #[case(cpal::ErrorKind::DeviceNotAvailable, Some(FaultKind::Disconnected))]
    #[case(cpal::ErrorKind::StreamInvalidated, Some(FaultKind::Invalidated))]
    #[case(cpal::ErrorKind::HostUnavailable, Some(FaultKind::Backend))]
    #[case(cpal::ErrorKind::PermissionDenied, Some(FaultKind::Backend))]
    fn stream_errors_are_classified(
        #[case] kind: cpal::ErrorKind,
        #[case] expected: Option<FaultKind>,
    ) {
        assert_eq!(classify(kind), expected);
    }

    #[test]
    fn a_reporter_names_the_device_that_stopped() {
        let faults = FaultSlot::default();
        let mut report = fault_reporter(&faults, "USB Audio CODEC");

        report(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable));

        let fault = faults.take().expect("the fault should have been recorded");
        assert_eq!(fault.device, "USB Audio CODEC");
        assert_eq!(fault.kind, FaultKind::Disconnected);
        assert!(faults.take().is_none(), "the report should be taken once");
    }

    /// A failing stream keeps failing; the first report is the one that says
    /// why, so a later one must not push it out.
    #[test]
    fn the_first_report_is_the_one_kept() {
        let faults = FaultSlot::default();
        let mut report = fault_reporter(&faults, "device");

        report(cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable));
        report(cpal::Error::new(cpal::ErrorKind::BackendError));

        assert_eq!(
            faults.take().map(|fault| fault.kind),
            Some(FaultKind::Disconnected)
        );
    }

    /// An error the stream survives must leave nothing behind for the
    /// interface to find later.
    #[test]
    fn a_survivable_error_records_nothing() {
        let faults = FaultSlot::default();
        let mut report = fault_reporter(&faults, "device");

        report(cpal::Error::new(cpal::ErrorKind::Xrun));

        assert!(faults.take().is_none());
    }
}
