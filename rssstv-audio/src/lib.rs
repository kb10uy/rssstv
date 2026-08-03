//! Host audio adapters for RSSSTV.
//!
//! This crate owns device enumeration, stream formats, and callback
//! scheduling. It exposes normalized mono `f32` samples with stream positions
//! and nothing from the underlying host API, so the SSTV core and the
//! application never depend on a platform audio type.

mod capture;
mod error;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Host, SampleFormat, StreamConfig};
use ringbuf::HeapRb;
use ringbuf::traits::Split;

pub use capture::{Capture, CaptureFeed, CaptureReader, Reading, synthetic_capture};
pub use error::AudioError;

/// Capture rate preferred by the rest of the project.
pub const PREFERRED_SAMPLE_RATE_HZ: u32 = 48_000;

/// Lowest rate the SSTV receive front end accepts.
pub const MINIMUM_SAMPLE_RATE_HZ: u32 = 6_000;

/// One selectable capture device.
///
/// Devices are identified by the host's own identifier rather than by name, so
/// selection survives two devices sharing a display name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputDevice {
    id: cpal::DeviceId,
    name: String,
}

impl InputDevice {
    /// Returns the human-readable device name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for InputDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

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
        Ok(devices
            .filter(|device| device.default_input_config().is_ok())
            .filter_map(|device| describe(&device))
            .collect())
    }

    /// Returns the host's default input device.
    pub fn default_input_device(&self) -> Option<InputDevice> {
        describe(&self.host.default_input_device()?)
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
        let report = |error: cpal::Error| eprintln!("audio capture error: {error}");
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

        let capture = Capture::new(stream, sample_rate, channels);
        capture.play()?;
        Ok((capture, CaptureReader::new(consumer, dropped, sample_rate)))
    }
}

impl Default for AudioHost {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for AudioHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AudioHost").finish_non_exhaustive()
    }
}

fn describe(device: &cpal::Device) -> Option<InputDevice> {
    Some(InputDevice {
        id: device.id().ok()?,
        name: device.description().ok()?.name().to_owned(),
    })
}

/// Chooses [`PREFERRED_SAMPLE_RATE_HZ`] when the device supports it.
fn preferred_rate(device: &cpal::Device, fallback: u32) -> u32 {
    let Ok(configs) = device.supported_input_configs() else {
        return fallback;
    };
    let supported = configs.into_iter().any(|range| {
        range.min_sample_rate() <= PREFERRED_SAMPLE_RATE_HZ
            && PREFERRED_SAMPLE_RATE_HZ <= range.max_sample_rate()
    });
    if supported {
        PREFERRED_SAMPLE_RATE_HZ
    } else {
        fallback
    }
}
