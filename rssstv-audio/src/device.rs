use std::{
    collections::{HashMap, HashSet},
    fmt,
};

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

/// A device as the host describes it, before it is told apart from the rest.
pub(crate) struct Described {
    pub(crate) id: cpal::DeviceId,
    pub(crate) label: Label,
}

/// What a device can be called.
///
/// The name is what the host would show; the qualifier is what the host calls
/// the device to itself, which is the ALSA PCM name (`hw:CARD=0,DEV=6`) or the
/// WASAPI interface. It is kept because names collide and identifiers do not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Label {
    name: String,
    qualifier: Option<String>,
}

pub(crate) fn describe(device: &cpal::Device) -> Option<Described> {
    let id = device.id().ok()?;
    let description = device.description().ok()?;
    let qualifier = description.driver().map(str::to_owned);
    // ALSA builds the name as "card, PCM" and leaves the comma behind when the
    // PCM has no name of its own, so half a laptop's devices arrive as
    // `sof-hda-dsp, `. A device with no name at all is called by its
    // identifier, which is the only thing left that names it.
    let mut name = tidy(description.name());
    if name.is_empty() {
        name = qualifier.clone().unwrap_or_else(|| id.id().to_owned());
    }
    Some(Described {
        id,
        label: Label { name, qualifier },
    })
}

/// Pairs every device with a name none of the others carry.
///
/// Devices are chosen by name throughout the application — the menu, the saved
/// configuration — so two devices sharing one make the second unreachable. A
/// Linux host makes that the common case rather than the exception: one sound
/// card is enumerated once per PCM and per plugin, and every one of those
/// entries is named after the card.
pub(crate) fn named(described: Vec<Described>) -> impl Iterator<Item = (cpal::DeviceId, String)> {
    let labels: Vec<_> = described
        .iter()
        .map(|device| device.label.clone())
        .collect();
    let names = distinguish(&labels);
    described.into_iter().map(|device| device.id).zip(names)
}

/// Trims the separator ALSA leaves behind on a nameless PCM.
fn tidy(name: &str) -> String {
    name.trim().trim_end_matches([',', ' ']).to_owned()
}

/// Names each of `labels` so that no two read alike.
///
/// A name shared by several devices takes the host identifier beside it. When
/// that is missing or shared as well the remaining namesakes are numbered,
/// which says nothing about the device but does let the operator reach the
/// second one.
fn distinguish(labels: &[Label]) -> Vec<String> {
    let mut shared: HashMap<&str, usize> = HashMap::new();
    for label in labels {
        *shared.entry(label.name.as_str()).or_default() += 1;
    }
    let mut names: Vec<String> = labels
        .iter()
        .map(|label| match &label.qualifier {
            Some(qualifier)
                if shared[label.name.as_str()] > 1 && qualifier.as_str() != label.name =>
            {
                format!("{} ({qualifier})", label.name)
            }
            _ => label.name.clone(),
        })
        .collect();

    let mut counted: HashMap<&str, usize> = HashMap::new();
    for name in &names {
        *counted.entry(name.as_str()).or_default() += 1;
    }
    let still_shared: HashSet<String> = counted
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name.to_owned())
        .collect();
    let mut ordinals: HashMap<String, usize> = HashMap::new();
    for name in &mut names {
        if !still_shared.contains(name.as_str()) {
            continue;
        }
        let ordinal = ordinals.entry(name.clone()).or_default();
        *ordinal += 1;
        *name = format!("{name} #{ordinal}");
    }
    names
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn label(name: &str, qualifier: Option<&str>) -> Label {
        Label {
            name: name.to_owned(),
            qualifier: qualifier.map(str::to_owned),
        }
    }

    /// ALSA writes "card, PCM" and keeps the separator when the PCM is
    /// nameless, so the name arrives with punctuation the operator never
    /// asked to read.
    #[rstest]
    #[case("sof-hda-dsp, ", "sof-hda-dsp")]
    #[case("sof-hda-dsp, HDMI 1", "sof-hda-dsp, HDMI 1")]
    #[case("  USB Audio CODEC  ", "USB Audio CODEC")]
    #[case(", ", "")]
    fn a_name_loses_its_trailing_separator(#[case] reported: &str, #[case] expected: &str) {
        assert_eq!(tidy(reported), expected);
    }

    /// The case this exists for: one sound card enumerated once per PCM and
    /// per plugin, every entry named after the card.
    #[test]
    fn namesakes_are_told_apart_by_identifier() {
        let labels = [
            label("sof-hda-dsp", Some("hw:CARD=0,DEV=6")),
            label("sof-hda-dsp", Some("plughw:CARD=0,DEV=6")),
            label("sof-hda-dsp", Some("hw:CARD=0,DEV=7")),
        ];

        assert_eq!(
            distinguish(&labels),
            [
                "sof-hda-dsp (hw:CARD=0,DEV=6)",
                "sof-hda-dsp (plughw:CARD=0,DEV=6)",
                "sof-hda-dsp (hw:CARD=0,DEV=7)",
            ]
        );
    }

    /// A name nothing else shares is left as the host gave it: the identifier
    /// would only be noise beside it.
    #[test]
    fn a_name_of_its_own_is_left_alone() {
        let labels = [
            label("PipeWire Sound Server", Some("pipewire")),
            label("USB Audio CODEC", Some("hw:CARD=1,DEV=0")),
        ];

        assert_eq!(
            distinguish(&labels),
            ["PipeWire Sound Server", "USB Audio CODEC"]
        );
    }

    /// A device the host cannot tell apart still has to be reachable, because
    /// the menu and the saved configuration both choose by name.
    #[test]
    fn devices_beyond_naming_are_numbered() {
        let labels = [
            label("Line In", None),
            label("Line In", None),
            label("Line In", Some("Line In")),
        ];

        assert_eq!(
            distinguish(&labels),
            ["Line In #1", "Line In #2", "Line In #3"]
        );
    }

    /// Numbering is the last resort, so it may not touch the devices an
    /// identifier already separated.
    #[test]
    fn numbering_reaches_only_what_is_still_shared() {
        let labels = [
            label("Codec", Some("hw:CARD=1,DEV=0")),
            label("Codec", None),
            label("Codec", None),
        ];

        assert_eq!(
            distinguish(&labels),
            ["Codec (hw:CARD=1,DEV=0)", "Codec #1", "Codec #2"]
        );
    }

    /// Two devices may report the same identifier as their name; a name
    /// repeated inside its own parentheses says nothing.
    #[test]
    fn an_identifier_is_not_repeated_after_the_name_it_matches() {
        let labels = [
            label("default", Some("default")),
            label("default", Some("sysdefault")),
        ];

        assert_eq!(distinguish(&labels), ["default", "default (sysdefault)"]);
    }
}
