use std::time::Duration;

use crate::{
    band::Band,
    command::{Command, Event, Script},
    error::RigError,
    rigctld::Rigctld,
};

/// What the rig was found to be tuned to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reading {
    pub frequency_hz: u64,
    /// The band the frequency falls in, if it falls in one.
    pub band: Option<Band>,
}

/// A connection to `rigctld` together with what the operator asked it to send.
///
/// The session is what knows that a moment has been reached; the script is
/// what knows which commands that moment sends. Keeping the two apart is what
/// lets the operator change any of it without the application knowing what a
/// particular rig needs.
#[derive(Debug)]
pub struct Session {
    rig: Rigctld,
    script: Script,
    reading: Option<Reading>,
}

impl Session {
    /// Connects and sends whatever the operator attached to [`Event::Open`].
    pub fn open(address: &str, timeout: Duration, script: Script) -> Result<Self, RigError> {
        let rig = Rigctld::connect(address, timeout)?;
        let mut session = Self {
            rig,
            script,
            reading: None,
        };
        session.fire(Event::Open)?;
        Ok(session)
    }

    pub fn address(&self) -> &str {
        self.rig.address()
    }

    pub fn script(&self) -> &Script {
        &self.script
    }

    /// The last frequency read, if the session has read one.
    pub const fn reading(&self) -> Option<Reading> {
        self.reading
    }

    /// Sends what the operator attached to `event`.
    ///
    /// The commands run in the order they were written and stop at the first
    /// one the rig refuses. A sequence that sets a data mode before keying
    /// only means anything if the keying does not happen when the mode change
    /// failed.
    pub fn fire(&mut self, event: Event) -> Result<(), RigError> {
        run_all(&mut self.rig, self.script.commands(event))
    }

    /// Reads the frequency, sending a band's commands on arriving at it.
    ///
    /// Connecting while already on a band counts as arriving on it, so a
    /// session that selects an antenna per band selects one without waiting
    /// for the operator to tune somewhere else first.
    pub fn poll(&mut self) -> Result<Reading, RigError> {
        let frequency_hz = self.rig.frequency_hz()?;
        let reading = Reading {
            frequency_hz,
            band: Band::for_frequency(frequency_hz),
        };
        let arrived = reading.band.filter(|band| {
            self.reading
                .and_then(|previous| previous.band)
                .is_none_or(|previous| previous != *band)
        });
        self.reading = Some(reading);
        if let Some(band) = arrived {
            run_all(&mut self.rig, self.script.band_commands(band))?;
        }
        Ok(reading)
    }

    /// Sends what the operator attached to [`Event::Close`].
    ///
    /// Errors are the caller's to ignore: a session being given up because the
    /// connection already failed cannot be tidied up over that connection.
    pub fn close(&mut self) -> Result<(), RigError> {
        self.fire(Event::Close)
    }
}

fn run_all(rig: &mut Rigctld, commands: &[Command]) -> Result<(), RigError> {
    for command in commands {
        rig.run(command)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, BufWriter, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
    };

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// A stand-in for `rigctld` that accepts anything and answers in order.
    struct FakeRig {
        address: String,
        received: Arc<Mutex<Vec<String>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl FakeRig {
        fn spawn(answers: &[&str]) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let received = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&received);
            let answers: Vec<String> = answers.iter().map(|answer| (*answer).to_owned()).collect();
            let thread = thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                serve(&stream, &recorder, answers);
            });
            Self {
                address,
                received,
                thread: Some(thread),
            }
        }

        fn received(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }
    }

    impl Drop for FakeRig {
        fn drop(&mut self) {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn serve(stream: &TcpStream, recorder: &Arc<Mutex<Vec<String>>>, answers: Vec<String>) {
        let mut writer = BufWriter::new(stream.try_clone().unwrap());
        let reader = BufReader::new(stream.try_clone().unwrap());
        for (request, answer) in reader.lines().map_while(Result::ok).zip(answers) {
            recorder.lock().unwrap().push(request);
            if writer.write_all(answer.as_bytes()).is_err() || writer.flush().is_err() {
                return;
            }
        }
    }

    const NO_VFO: &str = "chk_vfo:\nChkVFO: 0\nRPRT 0\n";
    const DONE: &str = "RPRT 0\n";

    fn script_with(event: Event, words: &[&[&str]]) -> Script {
        let mut script = Script::default();
        script.set(
            event,
            words
                .iter()
                .map(|command| Command::new(command.iter().copied()).unwrap())
                .collect(),
        );
        script
    }

    #[test]
    fn opening_sends_what_the_operator_attached_to_it() {
        let fake = FakeRig::spawn(&[NO_VFO, DONE, DONE]);
        let script = script_with(Event::Open, &[&["M", "USB", "0"], &["L", "RFPOWER", "0.4"]]);

        let session = Session::open(&fake.address, TEST_TIMEOUT, script).unwrap();
        drop(session);

        assert_eq!(
            fake.received(),
            ["+\\chk_vfo", "+M USB 0", "+L RFPOWER 0.4"]
        );
    }

    #[test]
    fn keying_sends_the_default_commands_when_nothing_was_configured() {
        let fake = FakeRig::spawn(&[NO_VFO, DONE, DONE]);
        let mut session = Session::open(&fake.address, TEST_TIMEOUT, Script::default()).unwrap();

        session.fire(Event::Transmit).unwrap();
        session.fire(Event::Receive).unwrap();

        assert_eq!(fake.received(), ["+\\chk_vfo", "+T 1", "+T 0"]);
    }

    /// A sequence that prepares the rig before keying it only means anything
    /// if the keying is abandoned when the preparation failed.
    #[test]
    fn a_refused_command_abandons_the_rest_of_the_event() {
        let fake = FakeRig::spawn(&[NO_VFO, "RPRT -1\n", DONE]);
        let script = script_with(Event::Transmit, &[&["M", "PKTUSB", "0"], &["T", "1"]]);
        let mut session = Session::open(&fake.address, TEST_TIMEOUT, script).unwrap();

        let error = session.fire(Event::Transmit).unwrap_err();

        assert!(
            matches!(error, RigError::Refused { code: -1, .. }),
            "{error:?}"
        );
        assert_eq!(fake.received(), ["+\\chk_vfo", "+M PKTUSB 0"]);
    }

    #[test]
    fn polling_reports_the_frequency_and_the_band_it_sits_in() {
        let fake = FakeRig::spawn(&[NO_VFO, "get_freq:\nFreq: 14230000\nRPRT 0\n"]);
        let mut session = Session::open(&fake.address, TEST_TIMEOUT, Script::default()).unwrap();

        let reading = session.poll().unwrap();

        assert_eq!(reading.frequency_hz, 14_230_000);
        assert_eq!(reading.band.map(Band::name), Some("20m"));
        assert_eq!(session.reading(), Some(reading));
    }

    /// Arriving on a band is what fires its commands, and staying on it is
    /// not: a poll every second must not resend them every second.
    #[test]
    fn a_band_sends_its_commands_on_arrival_and_not_again() {
        let fake = FakeRig::spawn(&[
            NO_VFO,
            "Freq: 7178000\nRPRT 0\n",
            DONE,
            "Freq: 7040000\nRPRT 0\n",
            "Freq: 14230000\nRPRT 0\n",
            DONE,
        ]);
        let mut script = Script::default();
        script.set_band(
            Band::from_name("40m").unwrap(),
            vec![Command::new(["\\set_ant", "1", "0"]).unwrap()],
        );
        script.set_band(
            Band::from_name("20m").unwrap(),
            vec![Command::new(["\\set_ant", "2", "0"]).unwrap()],
        );
        let mut session = Session::open(&fake.address, TEST_TIMEOUT, script).unwrap();

        session.poll().unwrap();
        session.poll().unwrap();
        session.poll().unwrap();

        assert_eq!(
            fake.received(),
            [
                "+\\chk_vfo",
                "+\\get_freq",
                "+\\set_ant 1 0",
                "+\\get_freq",
                "+\\get_freq",
                "+\\set_ant 2 0",
            ]
        );
    }

    /// A rig tuned off the bands has no commands to send, and coming back on
    /// to the band it left is an arrival again.
    #[test]
    fn leaving_the_bands_and_returning_counts_as_arriving_again() {
        let fake = FakeRig::spawn(&[
            NO_VFO,
            "Freq: 7178000\nRPRT 0\n",
            DONE,
            "Freq: 6000000\nRPRT 0\n",
            "Freq: 7178000\nRPRT 0\n",
            DONE,
        ]);
        let mut script = Script::default();
        script.set_band(
            Band::from_name("40m").unwrap(),
            vec![Command::new(["\\set_ant", "1", "0"]).unwrap()],
        );
        let mut session = Session::open(&fake.address, TEST_TIMEOUT, script).unwrap();

        session.poll().unwrap();
        let away = session.poll().unwrap();
        session.poll().unwrap();

        assert_eq!(away.band, None);
        assert_eq!(
            fake.received()
                .iter()
                .filter(|line| line.starts_with("+\\set_ant"))
                .count(),
            2
        );
    }
}
