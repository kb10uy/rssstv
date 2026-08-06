use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::Duration,
};

use crate::error::RigError;

/// Where `rigctld` listens unless it was told otherwise.
pub const DEFAULT_ADDRESS: &str = "127.0.0.1:4532";

/// How long a command may take before the connection is called broken.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1_500);

/// How many lines one answer may run to before it is called unreadable.
///
/// `\dump_state` is the longest thing `rigctld` says and stays well inside
/// this, so a count past it means the stream is no longer framed the way the
/// protocol says and reading on would never end.
const MAXIMUM_RESPONSE_LINES: usize = 512;

/// How long one line of an answer may run, for the same reason.
///
/// The read timeout only catches a peer that goes quiet; a peer that streams
/// bytes without ever framing a line would otherwise grow the buffer for as
/// long as it keeps talking.
const MAXIMUM_RESPONSE_LINE_BYTES: u64 = 4_096;

/// What `rigctld` said in answer to one command.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Response {
    lines: Vec<String>,
}

impl Response {
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Reads the first number in the answer, however it was labelled.
    ///
    /// The label a Hamlib backend prints is not worth depending on, and every
    /// answer this crate reads for itself holds exactly one number, so the
    /// first one found is the one that was asked for.
    fn number(&self) -> Option<u64> {
        self.lines.iter().find_map(|line| {
            line.rsplit(':')
                .next()
                .and_then(|value| value.trim().parse().ok())
        })
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.lines.iter().find_map(|line| {
            let (label, value) = line.split_once(':')?;
            label
                .trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim())
                .filter(|value| !value.is_empty())
        })
    }
}

/// A live connection to `rigctld`.
///
/// Every command is sent in the protocol's extended form, which answers with a
/// terminating `RPRT` line whatever the command was. Without it there is no
/// way to know how many lines an answer runs to, and the operator's own
/// commands are exactly the ones this crate cannot know the shape of.
#[derive(Debug)]
pub struct Rigctld {
    address: String,
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    /// Set when `rigctld` was started with `--vfo` and wants every command
    /// addressed to a VFO.
    requires_vfo: bool,
}

impl Rigctld {
    /// Connects to `address` and settles how commands have to be addressed.
    pub fn connect(address: &str, timeout: Duration) -> Result<Self, RigError> {
        let target = address
            .to_socket_addrs()
            .map_err(|_| RigError::Address(address.to_owned()))?
            .next()
            .ok_or_else(|| RigError::Address(address.to_owned()))?;
        let stream =
            TcpStream::connect_timeout(&target, timeout).map_err(|error| RigError::Connect {
                address: address.to_owned(),
                detail: error.to_string(),
            })?;
        let connect = |error: std::io::Error| RigError::Connect {
            address: address.to_owned(),
            detail: error.to_string(),
        };
        stream.set_read_timeout(Some(timeout)).map_err(connect)?;
        stream.set_write_timeout(Some(timeout)).map_err(connect)?;
        // Keying is the latency that matters: a command held back waiting for
        // more to coalesce with would delay the carrier behind it.
        stream.set_nodelay(true).map_err(connect)?;
        let writer = stream.try_clone().map_err(connect)?;
        let mut rig = Self {
            address: address.to_owned(),
            reader: BufReader::new(stream),
            writer,
            requires_vfo: false,
        };
        rig.requires_vfo = rig.asks_for_a_vfo();
        Ok(rig)
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub const fn requires_vfo(&self) -> bool {
        self.requires_vfo
    }

    /// Runs one command written by the operator's script, as written.
    ///
    /// Nothing is added to it, including the VFO argument: which commands take
    /// one is a property of the command, and a script written against a
    /// particular `rigctld` knows how it was started better than a guess here
    /// would.
    ///
    /// One call is one command. A line holding two is something only the far
    /// end could recognize, but a line break here would frame two outright, so
    /// that much is refused.
    pub fn run(&mut self, command: &str) -> Result<Response, RigError> {
        let line = command.trim();
        if line.is_empty() || line.contains(['\r', '\n']) {
            return Err(RigError::NotOneLine);
        }
        self.send(line)
    }

    /// Asks what the rig is tuned to.
    pub fn frequency_hz(&mut self) -> Result<u64, RigError> {
        let response = self.query("\\get_freq")?;
        response.number().ok_or(RigError::Unreadable {
            command: "\\get_freq".to_owned(),
        })
    }

    /// Asks which mode the rig is using.
    pub fn mode(&mut self) -> Result<String, RigError> {
        let response = self.query("\\get_mode")?;
        response
            .text("mode")
            .map(str::to_owned)
            .ok_or(RigError::Unreadable {
                command: "\\get_mode".to_owned(),
            })
    }

    /// Sends one of this crate's own commands, addressed as `rigctld` wants.
    fn query(&mut self, name: &str) -> Result<Response, RigError> {
        if self.requires_vfo {
            self.send(&format!("{name} currVFO"))
        } else {
            self.send(name)
        }
    }

    /// Asks whether `rigctld` was started expecting a VFO on every command.
    ///
    /// A failure here is not one: an installation too old to know the question
    /// is also one that does not want the argument, so the answer is no.
    fn asks_for_a_vfo(&mut self) -> bool {
        self.send("\\chk_vfo")
            .ok()
            .and_then(|response| response.number())
            .is_some_and(|answer| answer == 1)
    }

    fn send(&mut self, line: &str) -> Result<Response, RigError> {
        writeln!(self.writer, "+{line}")
            .and_then(|()| self.writer.flush())
            .map_err(|error| RigError::Transport(error.to_string()))?;
        let mut lines = Vec::new();
        loop {
            let mut raw = String::new();
            let read = (&mut self.reader)
                .take(MAXIMUM_RESPONSE_LINE_BYTES)
                .read_line(&mut raw)
                .map_err(|error| RigError::Transport(error.to_string()))?;
            if read == 0 {
                return Err(RigError::Closed);
            }
            if read as u64 == MAXIMUM_RESPONSE_LINE_BYTES && !raw.ends_with('\n') {
                return Err(RigError::Unreadable {
                    command: line.to_owned(),
                });
            }
            let trimmed = raw.trim_end_matches(['\r', '\n']);
            if let Some(status) = trimmed.strip_prefix("RPRT ") {
                let code: i32 = status.trim().parse().map_err(|_| RigError::Unreadable {
                    command: line.to_owned(),
                })?;
                if code != 0 {
                    return Err(RigError::Refused {
                        command: line.to_owned(),
                        code,
                    });
                }
                return Ok(Response { lines });
            }
            if lines.len() >= MAXIMUM_RESPONSE_LINES {
                return Err(RigError::Unreadable {
                    command: line.to_owned(),
                });
            }
            lines.push(trimmed.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::BufWriter,
        net::TcpListener,
        sync::{Arc, Mutex},
        thread::{self, JoinHandle},
    };

    use rstest::rstest;

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    /// A stand-in for `rigctld` that answers from a fixed list.
    ///
    /// Answers are handed out in the order they were given, so a test says
    /// what the far end replies without depending on Hamlib being installed.
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
                let mut writer = BufWriter::new(stream.try_clone().unwrap());
                let reader = BufReader::new(stream);
                for (request, answer) in reader.lines().map_while(Result::ok).zip(answers) {
                    recorder.lock().unwrap().push(request);
                    if writer.write_all(answer.as_bytes()).is_err() || writer.flush().is_err() {
                        return;
                    }
                }
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

    #[test]
    fn a_frequency_is_read_from_a_labelled_answer() {
        let fake = FakeRig::spawn(&[
            "chk_vfo:\nChkVFO: 0\nRPRT 0\n",
            "get_freq:\nFreq: 14230000\nRPRT 0\n",
        ]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert_eq!(rig.frequency_hz(), Ok(14_230_000));
        assert!(!rig.requires_vfo());
        assert_eq!(fake.received(), ["+\\chk_vfo", "+\\get_freq"]);
    }

    #[test]
    fn a_mode_is_read_from_a_labelled_answer() {
        let fake = FakeRig::spawn(&[
            "chk_vfo:\nChkVFO: 1\nRPRT 0\n",
            "get_mode: currVFO\nMode: USB\nPassband: 2400\nRPRT 0\n",
        ]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert_eq!(rig.mode(), Ok("USB".to_owned()));
        assert_eq!(fake.received(), ["+\\chk_vfo", "+\\get_mode currVFO"]);
    }

    /// A `rigctld` started with `--vfo` refuses every command that does not
    /// name one, which is a failure the operator would have no way to read.
    #[test]
    fn a_vfo_answer_puts_a_vfo_on_this_crate_s_own_commands() {
        let fake = FakeRig::spawn(&[
            "chk_vfo:\nChkVFO: 1\nRPRT 0\n",
            "get_freq:\nFreq: 7178000\nRPRT 0\n",
        ]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert!(rig.requires_vfo());
        assert_eq!(rig.frequency_hz(), Ok(7_178_000));
        assert_eq!(fake.received(), ["+\\chk_vfo", "+\\get_freq currVFO"]);
    }

    /// A script's commands go out as written: this crate does not know which
    /// of them take a VFO, and guessing would break the ones that do not.
    #[test]
    fn a_script_command_is_sent_untouched() {
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 1\nRPRT 0\n", "set_level:\nRPRT 0\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert!(rig.run("L MONITOR_GAIN 0.15").is_ok());
        assert_eq!(fake.received()[1], "+L MONITOR_GAIN 0.15");
    }

    /// One call is one command. A line break would frame two, and the answer
    /// to the second would be left in the stream for the next command to read.
    #[rstest]
    #[case::two_lines("T 1\nT 0")]
    #[case::a_trailing_line_break_and_another_command("T 1\r\nT 0")]
    #[case::nothing("")]
    #[case::only_spacing("   ")]
    fn a_command_that_is_not_one_line_is_refused(#[case] written: &str) {
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 0\nRPRT 0\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert_eq!(rig.run(written), Err(RigError::NotOneLine));
        // Nothing reached the far end, so the stream is still in step.
        assert_eq!(fake.received(), ["+\\chk_vfo"]);
    }

    /// A peer that streams bytes without ever framing a line has left the
    /// protocol, and the answer is refused at a fixed size rather than being
    /// buffered for as long as the peer keeps talking.
    #[test]
    fn an_unframed_answer_is_refused_at_a_fixed_size() {
        let unframed = "x".repeat(MAXIMUM_RESPONSE_LINE_BYTES as usize * 2);
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 0\nRPRT 0\n", &unframed]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert_eq!(
            rig.run("f"),
            Err(RigError::Unreadable {
                command: "f".to_owned(),
            })
        );
    }

    /// Spacing a command out to read well in a script is not a reason to
    /// refuse it, and the far end never sees the difference.
    #[test]
    fn surrounding_spacing_is_trimmed_rather_than_refused() {
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 0\nRPRT 0\n", "RPRT 0\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert!(rig.run("  T 1  ").is_ok());
        assert_eq!(fake.received()[1], "+T 1");
    }

    #[test]
    fn a_refused_command_reports_the_status_hamlib_gave() {
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 0\nRPRT 0\n", "set_ptt:\nRPRT -11\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        let refused = rig.run("T 1").unwrap_err();

        assert_eq!(
            refused,
            RigError::Refused {
                command: "T 1".to_owned(),
                code: -11,
            }
        );
        // The command was refused, not the connection, so the next one may go.
        assert!(!refused.is_fatal());
    }

    #[test]
    fn a_hangup_is_reported_rather_than_waited_on() {
        let fake = FakeRig::spawn(&["chk_vfo:\nChkVFO: 0\nRPRT 0\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert_eq!(rig.frequency_hz(), Err(RigError::Closed));
    }

    /// An installation too old to answer the question is one that does not
    /// want the argument either, so it has to connect rather than fail.
    #[test]
    fn a_refused_vfo_question_settles_as_no_vfo() {
        let fake = FakeRig::spawn(&["RPRT -11\n", "get_freq:\nFreq: 50110000\nRPRT 0\n"]);
        let mut rig = Rigctld::connect(&fake.address, TEST_TIMEOUT).unwrap();

        assert!(!rig.requires_vfo());
        assert_eq!(rig.frequency_hz(), Ok(50_110_000));
    }

    #[test]
    fn nothing_listening_is_reported_against_the_address() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);

        let error = Rigctld::connect(&address, Duration::from_millis(200)).unwrap_err();

        assert!(matches!(error, RigError::Connect { .. }), "{error:?}");
    }

    #[test]
    fn an_unresolvable_address_is_reported_before_any_socket_is_opened() {
        let error = Rigctld::connect("not a host", TEST_TIMEOUT).unwrap_err();
        assert_eq!(error, RigError::Address("not a host".to_owned()));
    }
}
