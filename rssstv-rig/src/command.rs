use std::collections::BTreeMap;

use crate::{band::Band, error::RigError};

/// One `rigctld` command, as the line it is sent as.
///
/// A line rather than a list of arguments, because that is what `rigctld`
/// reads and what the operator writes: the protocol splits the line on
/// whitespace itself, so an argument this crate could keep separate is not one
/// the far end could tell apart anyway.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command(String);

impl Command {
    /// Reads one command, rejecting a line that names nothing.
    ///
    /// Whitespace is normalized on the way in, so a line spaced out to read
    /// well in the configuration file is sent as the protocol wants it.
    pub fn parse(line: &str) -> Result<Self, RigError> {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            return Err(RigError::EmptyCommand);
        }
        Ok(Self(line))
    }

    /// Reads a command per line, skipping the ones that name nothing.
    ///
    /// A blank line is spacing rather than a command, so it is passed over
    /// instead of being refused: the operator is writing a script, and how it
    /// is laid out is theirs.
    pub fn parse_script(text: &str) -> Vec<Self> {
        text.lines()
            .filter_map(|line| Self::parse(line).ok())
            .collect()
    }

    /// Writes `commands` back out as the script they were read from.
    pub fn script(commands: &[Self]) -> String {
        commands
            .iter()
            .map(Self::line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The command as `rigctld` reads it, without the protocol's own framing.
    pub fn line(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for Command {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
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
    const fn default_script(self) -> &'static str {
        match self {
            Self::Transmit => "T 1",
            Self::Receive => "T 0",
            Self::Open | Self::Close => "",
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
                .map(|event| (event, Command::parse_script(event.default_script())))
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

    /// A line laid out to read in the configuration file has to reach the rig
    /// as the protocol wants it, which is one space between words.
    #[rstest]
    #[case("L MONITOR_GAIN 0.15", "L MONITOR_GAIN 0.15")]
    #[case("  T   1  ", "T 1")]
    #[case("\\set_ptt 1", "\\set_ptt 1")]
    fn a_command_is_read_as_the_line_it_is_sent_as(#[case] written: &str, #[case] expected: &str) {
        assert_eq!(Command::parse(written).unwrap().line(), expected);
    }

    #[rstest]
    #[case::nothing("")]
    #[case::only_spacing("   \t ")]
    fn a_command_that_names_nothing_is_refused(#[case] written: &str) {
        assert_eq!(Command::parse(written), Err(RigError::EmptyCommand));
    }

    /// The operator is writing a script, so how it is laid out is theirs: a
    /// blank line between two commands is spacing rather than a third command.
    #[test]
    fn a_script_is_one_command_per_line_and_survives_a_round_trip() {
        let script = "M PKTUSB 0\n\nL MONITOR_GAIN 0.15\nT 1\n";
        let commands = Command::parse_script(script);

        assert_eq!(commands.len(), 3);
        assert_eq!(
            Command::script(&commands),
            "M PKTUSB 0\nL MONITOR_GAIN 0.15\nT 1"
        );
    }

    #[test]
    fn a_script_with_nothing_in_it_holds_no_commands() {
        assert!(Command::parse_script("").is_empty());
        assert!(Command::parse_script("\n \n").is_empty());
    }

    /// Keying has to work out of the box, and everything else has to stay
    /// silent until the operator asks for it.
    #[test]
    fn keying_is_configured_by_default_and_nothing_else_is() {
        let script = Script::default();
        assert_eq!(
            script.commands(Event::Transmit),
            [Command::parse("T 1").unwrap()]
        );
        assert_eq!(
            script.commands(Event::Receive),
            [Command::parse("T 0").unwrap()]
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
        let command = Command::parse("\\set_ant 1 0").unwrap();
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
