use std::fmt;

use cpal::{SampleFormat, traits::DeviceTrait};

use crate::PREFERRED_SAMPLE_RATE_HZ;

/// One selectable capture device.
///
/// Devices are identified by the host's own identifier rather than by name, so
/// selection survives two devices sharing a display name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InputDevice {
    pub(crate) id: cpal::DeviceId,
    pub(crate) name: String,
}

/// One selectable playback device.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutputDevice {
    pub(crate) id: cpal::DeviceId,
    pub(crate) name: String,
}

impl OutputDevice {
    /// Returns the human-readable device name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for OutputDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
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

pub(crate) fn describe(device: &cpal::Device) -> Option<InputDevice> {
    Some(InputDevice {
        id: device.id().ok()?,
        name: device.description().ok()?.name().to_owned(),
    })
}

pub(crate) fn describe_output(device: &cpal::Device) -> Option<OutputDevice> {
    Some(OutputDevice {
        id: device.id().ok()?,
        name: device.description().ok()?.name().to_owned(),
    })
}

/// Chooses [`PREFERRED_SAMPLE_RATE_HZ`] when the device supports it.
pub(crate) fn preferred_rate(device: &cpal::Device, fallback: u32) -> u32 {
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

pub(crate) fn preferred_output_rate(
    device: &cpal::Device,
    fallback: u32,
    channels: u16,
    sample_format: SampleFormat,
) -> u32 {
    let Ok(configs) = device.supported_output_configs() else {
        return fallback;
    };
    let supported = configs.into_iter().any(|range| {
        range.channels() == channels
            && range.sample_format() == sample_format
            && range.min_sample_rate() <= PREFERRED_SAMPLE_RATE_HZ
            && PREFERRED_SAMPLE_RATE_HZ <= range.max_sample_rate()
    });
    if supported {
        PREFERRED_SAMPLE_RATE_HZ
    } else {
        fallback
    }
}
