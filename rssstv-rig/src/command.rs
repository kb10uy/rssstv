use std::collections::BTreeMap;

use crate::{band::Band, error::RigError};

/// One `rigctld` command, held as the words it is sent as.
///
/// Split rather than kept as a line because the operator writes these in the
/// configuration file. A level name and its value are separate arguments, and
/// storing them apart means neither this crate nor the operator has to know a
/// quoting rule for the ones that contain spaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command(Vec<String>);

impl Command {
    /// Builds a command from its words, rejecting one that names nothing.
    pub fn new<I, S>(words: I) -> Result<Self, RigError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let words: Vec<String> = words
            .into_iter()
            .map(Into::into)
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() {
            return Err(RigError::EmptyCommand);
        }
        Ok(Self(words))
    }

    pub fn words(&self) -> &[String] {
        &self.0
    }

    /// The command as `rigctld` reads it, without the protocol's own framing.
    pub fn line(&self) -> String {
        self.0.join(" ")
    }
}

impl core::fmt::Display for Command {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.line())
    }
}

/// A moment in a session at which the operator's commands are sent.
///
/// The set is closed because each one is a point the application already knows
/// it has reached; an event nothing reaches would be a name in a file that
/// never fires.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Event {
    /// The connection has just been made.
    Open,
    /// The connection is about to be given up.
    Close,
    /// A transmission is starting, before any audio is sent.
    Transmit,
    /// A transmission has finished, after the last sample has been played.
    Receive,
}

impl Event {
    pub const ALL: [Self; 4] = [Self::Open, Self::Close, Self::Transmit, Self::Receive];

    /// The key this event is written under in the configuration file.
    pub const fn config_name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Close => "close",
            Self::Transmit => "transmit",
            Self::Receive => "receive",
        }
    }

    pub fn from_config(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|event| event.config_name() == name)
    }

    /// What the event sends when the operator has said nothing about it.
    ///
    /// Keying is the one thing rig control exists for here, so transmit and
    /// receive have to work before anything is configured. The rest of the
    /// events are the operator's own and start out empty.
    fn default_words(self) -> &'static [&'static [&'static str]] {
        match self {
            Self::Transmit => &[&["T", "1"]],
            Self::Receive => &[&["T", "0"]],
            Self::Open | Self::Close => &[],
        }
    }
}

/// What rig control sends, and when.
///
/// Every command is the operator's to change: rigs differ in what they need
/// around a transmission, from a monitor level to a data mode to an amplifier
/// on a separate port, and none of that belongs hard-coded in an SSTV
/// application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Script {
    events: BTreeMap<Event, Vec<Command>>,
    bands: BTreeMap<Band, Vec<Command>>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            events: Event::ALL
                .into_iter()
                .map(|event| {
                    let commands = event
                        .default_words()
                        .iter()
                        .map(|words| Command::new(words.iter().copied()).expect("a named default"))
                        .collect();
                    (event, commands)
                })
                .collect(),
            bands: BTreeMap::new(),
        }
    }
}

impl Script {
    /// Replaces what an event sends, including with nothing at all.
    ///
    /// An empty list is a decision rather than an omission: an operator whose
    /// rig is keyed by VOX wants the transmit event to send no command, and
    /// falling back on the default would key it anyway.
    pub fn set(&mut self, event: Event, commands: Vec<Command>) {
        self.events.insert(event, commands);
    }

    pub fn commands(&self, event: Event) -> &[Command] {
        self.events.get(&event).map_or(&[], Vec::as_slice)
    }

    /// Attaches commands to a band, sent whenever the rig arrives on it.
    pub fn set_band(&mut self, band: Band, commands: Vec<Command>) {
        self.bands.insert(band, commands);
    }

    pub fn band_commands(&self, band: Band) -> &[Command] {
        self.bands.get(&band).map_or(&[], Vec::as_slice)
    }

    pub fn bands(&self) -> impl Iterator<Item = (Band, &[Command])> {
        self.bands
            .iter()
            .map(|(band, commands)| (*band, commands.as_slice()))
    }

    /// Whether any band at all carries commands.
    ///
    /// Polling exists to keep the frequency current, but a script with band
    /// commands makes it the thing that fires them, so the two are asked about
    /// separately.
    pub fn has_band_commands(&self) -> bool {
        self.bands.values().any(|commands| !commands.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn a_command_keeps_the_words_it_was_given() {
        let command = Command::new(["L", "MONITOR_GAIN", "0.15"]).unwrap();
        assert_eq!(command.words(), ["L", "MONITOR_GAIN", "0.15"]);
        assert_eq!(command.line(), "L MONITOR_GAIN 0.15");
    }

    #[rstest]
    #[case::nothing(vec![])]
    #[case::only_empty_words(vec!["", ""])]
    fn a_command_that_names_nothing_is_refused(#[case] words: Vec<&str>) {
        assert_eq!(Command::new(words), Err(RigError::EmptyCommand));
    }

    /// Keying has to work out of the box, and everything else has to stay
    /// silent until the operator asks for it.
    #[test]
    fn keying_is_configured_by_default_and_nothing_else_is() {
        let script = Script::default();
        assert_eq!(
            script.commands(Event::Transmit),
            [Command::new(["T", "1"]).unwrap()]
        );
        assert_eq!(
            script.commands(Event::Receive),
            [Command::new(["T", "0"]).unwrap()]
        );
        assert!(script.commands(Event::Open).is_empty());
        assert!(script.commands(Event::Close).is_empty());
        assert!(!script.has_band_commands());
    }

    /// A VOX station wants no keying at all, which an empty list has to mean
    /// rather than falling back on the default.
    #[test]
    fn an_event_set_to_nothing_sends_nothing() {
        let mut script = Script::default();
        script.set(Event::Transmit, Vec::new());
        assert!(script.commands(Event::Transmit).is_empty());
    }

    #[test]
    fn band_commands_are_kept_under_the_band_they_were_written_for() {
        let mut script = Script::default();
        let command = Command::new(["\\set_ant", "1", "0"]).unwrap();
        script.set_band(Band::from_name("40m").unwrap(), vec![command.clone()]);

        assert_eq!(
            script.band_commands(Band::from_name("40m").unwrap()),
            [command]
        );
        assert!(
            script
                .band_commands(Band::from_name("20m").unwrap())
                .is_empty()
        );
        assert!(script.has_band_commands());
    }

    #[test]
    fn every_event_name_round_trips_through_the_configuration() {
        for event in Event::ALL {
            assert_eq!(Event::from_config(event.config_name()), Some(event));
        }
        assert_eq!(Event::from_config("keydown"), None);
    }
}
