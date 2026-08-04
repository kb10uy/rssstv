use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub use rssstv_rig::Reading;
use rssstv_rig::{DEFAULT_TIMEOUT, Event, RigError, Session};

use crate::storage::config::RigSettings;

/// How long the worker waits on a request when there is no polling to do.
///
/// A request wakes the wait the moment it is sent, so this only decides how
/// long an idle worker sleeps between doing nothing.
const IDLE_GAP: Duration = Duration::from_secs(60);

/// What rig control is doing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RigState {
    /// The operator has not asked for rig control.
    #[default]
    Disconnected,
    /// The connection is being made; nothing can be asked of it yet.
    Connecting,
    /// Connected, with the rig not keyed.
    Receiving,
    /// The transmit commands have run and the lead-in has passed.
    Transmitting,
    /// The connection is gone and the worker has stopped.
    Failed,
}

impl RigState {
    /// Whether a transmission may be handed to the rig.
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Receiving | Self::Transmitting)
    }

    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Disconnected => "rig-state-disconnected",
            Self::Connecting => "rig-state-connecting",
            Self::Receiving => "rig-state-receiving",
            Self::Transmitting => "rig-state-transmitting",
            Self::Failed => "rig-state-failed",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RigSnapshot {
    pub state: RigState,
    /// What the rig was last found to be tuned to.
    pub reading: Option<Reading>,
    /// The last failure, cleared when a transmission asks to be keyed.
    pub error: Option<String>,
}

/// What the interface asks the rig for.
///
/// Only the two sides of a transmission: everything else the rig is told is
/// the operator's own script, and the moments it hangs off are these.
enum Request {
    Transmit,
    Receive,
}

/// The thread that owns the connection to `rigctld`.
///
/// The interface never waits on it. Keying a rig means a socket round trip and
/// then a lead-in the rig needs to switch, and neither belongs in a frame.
pub struct RigWorker {
    snapshot: Arc<Mutex<RigSnapshot>>,
    /// Held as an option so that dropping the worker can close the channel,
    /// which is what tells the thread to run the close script and stop.
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl RigWorker {
    pub fn spawn(settings: &RigSettings) -> Self {
        let snapshot = Arc::new(Mutex::new(RigSnapshot {
            state: RigState::Connecting,
            ..RigSnapshot::default()
        }));
        let (requests, incoming) = mpsc::channel();
        let worker_snapshot = Arc::clone(&snapshot);
        let plan = Plan::new(settings);
        let thread = thread::spawn(move || rig_loop(plan, &incoming, &worker_snapshot));
        Self {
            snapshot,
            requests: Some(requests),
            thread: Some(thread),
        }
    }

    pub fn latest(&self) -> RigSnapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| RigSnapshot {
                state: RigState::Failed,
                error: Some("rig control state is unavailable".to_owned()),
                reading: None,
            })
    }

    /// Asks for the rig to be keyed.
    ///
    /// The last failure is cleared here rather than on the worker's thread, so
    /// that a transmission asking to be keyed cannot read the error left by
    /// the one before it in the moment before the request is picked up.
    pub fn transmit(&self) {
        update(&self.snapshot, |state| state.error = None);
        self.request(Request::Transmit);
    }

    /// Asks for the rig to be unkeyed.
    pub fn receive(&self) {
        self.request(Request::Receive);
    }

    fn request(&self, request: Request) {
        if let Some(requests) = self.requests.as_ref() {
            // A worker that has already stopped is one whose failure is
            // already in the snapshot, so there is nothing here to report.
            let _ = requests.send(request);
        }
    }
}

impl Drop for RigWorker {
    fn drop(&mut self) {
        self.requests = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl core::fmt::Debug for RigWorker {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RigWorker")
            .field("snapshot", &self.latest())
            .finish_non_exhaustive()
    }
}

/// What the worker was started with, owned so the thread can keep it.
struct Plan {
    address: String,
    /// How long between frequency reads, or nothing to never read it.
    poll: Option<Duration>,
    lead_in: Duration,
    tail: Duration,
    script: rssstv_rig::Script,
}

impl Plan {
    fn new(settings: &RigSettings) -> Self {
        Self {
            address: settings.address.clone(),
            poll: (settings.poll_seconds > 0.0)
                .then(|| Duration::from_secs_f32(settings.poll_seconds)),
            lead_in: Duration::from_secs_f32(settings.lead_in_seconds),
            tail: Duration::from_secs_f32(settings.tail_seconds),
            script: settings.script.clone(),
        }
    }
}

fn rig_loop(plan: Plan, requests: &Receiver<Request>, snapshot: &Arc<Mutex<RigSnapshot>>) {
    let mut session = match Session::open(&plan.address, DEFAULT_TIMEOUT, plan.script) {
        Ok(session) => session,
        Err(error) => {
            update(snapshot, |state| {
                state.state = RigState::Failed;
                state.error = Some(error.to_string());
            });
            return;
        }
    };
    update(snapshot, |state| state.state = RigState::Receiving);
    let mut keyed = false;
    loop {
        // Polling stops while the rig is keyed. Reading the frequency back
        // mid-transmission says nothing the operator cannot see, and it puts
        // CAT traffic on the wire during the one part of a session that has
        // to be left alone.
        let gap = match plan.poll {
            Some(poll) if !keyed => poll,
            _ => IDLE_GAP,
        };
        let outcome = match requests.recv_timeout(gap) {
            Ok(Request::Transmit) => {
                let outcome = session.fire(Event::Transmit);
                if outcome.is_ok() {
                    // The rig switches over on its own time, and anything sent
                    // inside that is lost. Waiting here rather than in the
                    // interface keeps the sleep off the frame.
                    thread::sleep(plan.lead_in);
                    keyed = true;
                    update(snapshot, |state| state.state = RigState::Transmitting);
                }
                outcome
            }
            Ok(Request::Receive) => {
                thread::sleep(plan.tail);
                keyed = false;
                update(snapshot, |state| state.state = RigState::Receiving);
                session.fire(Event::Receive)
            }
            Err(RecvTimeoutError::Timeout) if !keyed && plan.poll.is_some() => session
                .poll()
                .map(|reading| update(snapshot, |state| state.reading = Some(reading))),
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Err(error) = outcome {
            let stopping = !is_recoverable(&error);
            update(snapshot, |state| {
                state.error = Some(error.to_string());
                if stopping {
                    state.state = RigState::Failed;
                }
            });
            if stopping {
                return;
            }
        }
    }
    // Reported but not acted on: the session is being given up either way, and
    // a close script that failed leaves nothing behind to recover.
    if let Err(error) = session.close() {
        update(snapshot, |state| state.error = Some(error.to_string()));
    }
    update(snapshot, |state| state.state = RigState::Disconnected);
}

/// Whether the connection is still worth holding on to after `error`.
///
/// A command the rig refused is about that command; the operator can fix it
/// and try again over the same connection. A transport that failed is not
/// something the next command would survive either.
fn is_recoverable(error: &RigError) -> bool {
    matches!(
        error,
        RigError::Refused { .. } | RigError::Unreadable { .. } | RigError::EmptyCommand
    )
}

fn update(snapshot: &Mutex<RigSnapshot>, update: impl FnOnce(&mut RigSnapshot)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, BufWriter, Write},
        net::TcpListener,
        time::Instant,
    };

    use rssstv_rig::{Band, Command, Script};

    use super::*;

    /// A stand-in for `rigctld` that answers every command with success.
    ///
    /// Frequency reads are answered with a fixed frequency, so a test can
    /// watch what the worker asks for without Hamlib being installed.
    struct FakeRig {
        address: String,
        received: Arc<Mutex<Vec<String>>>,
    }

    impl FakeRig {
        fn spawn(frequency_hz: u64) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            let received = Arc::new(Mutex::new(Vec::new()));
            let recorder = Arc::clone(&received);
            thread::spawn(move || {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut writer = BufWriter::new(stream.try_clone().unwrap());
                let reader = BufReader::new(stream);
                for request in reader.lines().map_while(Result::ok) {
                    let answer = if request.contains("get_freq") {
                        format!("get_freq:\nFreq: {frequency_hz}\nRPRT 0\n")
                    } else if request.contains("chk_vfo") {
                        "chk_vfo:\nChkVFO: 0\nRPRT 0\n".to_owned()
                    } else {
                        "RPRT 0\n".to_owned()
                    };
                    recorder.lock().unwrap().push(request);
                    if writer.write_all(answer.as_bytes()).is_err() || writer.flush().is_err() {
                        return;
                    }
                }
            });
            Self { address, received }
        }

        fn received(&self) -> Vec<String> {
            self.received.lock().unwrap().clone()
        }
    }

    fn settings(address: &str) -> RigSettings {
        RigSettings {
            enabled: true,
            address: address.to_owned(),
            poll_seconds: 0.0,
            lead_in_seconds: 0.0,
            tail_seconds: 0.0,
            script: Script::default(),
        }
    }

    /// Waits for `predicate` to hold of the snapshot, or gives up.
    fn settle(worker: &RigWorker, predicate: impl Fn(&RigSnapshot) -> bool) -> RigSnapshot {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = worker.latest();
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "rig control settled at {snapshot:?}"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn keying_runs_the_transmit_script_and_then_the_receive_script() {
        let fake = FakeRig::spawn(14_230_000);
        let worker = RigWorker::spawn(&settings(&fake.address));
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        worker.receive();
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);
        drop(worker);

        assert_eq!(fake.received(), ["+\\chk_vfo", "+T 1", "+T 0"]);
    }

    /// The lead-in is what a rig needs to switch over, so nothing may call the
    /// rig ready to transmit until it has passed.
    #[test]
    fn the_lead_in_is_waited_out_before_the_rig_reports_it_is_transmitting() {
        let fake = FakeRig::spawn(14_230_000);
        let worker = RigWorker::spawn(&RigSettings {
            lead_in_seconds: 0.3,
            ..settings(&fake.address)
        });
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        let asked = Instant::now();
        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);

        assert!(
            asked.elapsed() >= Duration::from_millis(300),
            "{:?}",
            asked.elapsed()
        );
    }

    #[test]
    fn polling_reports_what_the_rig_is_tuned_to() {
        let fake = FakeRig::spawn(7_178_000);
        let worker = RigWorker::spawn(&RigSettings {
            poll_seconds: 0.02,
            ..settings(&fake.address)
        });

        let snapshot = settle(&worker, |snapshot| snapshot.reading.is_some());

        let reading = snapshot.reading.unwrap();
        assert_eq!(reading.frequency_hz, 7_178_000);
        assert_eq!(reading.band.map(Band::name), Some("40m"));
    }

    /// A keyed rig has to be left alone: a poll landing mid-transmission is
    /// CAT traffic during the one part of a session that cannot take it.
    #[test]
    fn a_keyed_rig_is_not_polled() {
        let fake = FakeRig::spawn(7_178_000);
        let worker = RigWorker::spawn(&RigSettings {
            poll_seconds: 0.02,
            ..settings(&fake.address)
        });
        settle(&worker, |snapshot| snapshot.reading.is_some());

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        let keyed_at = fake.received().len();
        thread::sleep(Duration::from_millis(200));

        assert_eq!(fake.received().len(), keyed_at);
    }

    #[test]
    fn a_rig_that_is_not_listening_is_reported_rather_than_waited_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        drop(listener);

        let worker = RigWorker::spawn(&settings(&address));

        let snapshot = settle(&worker, |snapshot| snapshot.state == RigState::Failed);
        assert!(snapshot.error.is_some());
    }

    /// The close script is the operator's, and giving up the connection is
    /// what it hangs off.
    #[test]
    fn dropping_the_worker_runs_the_close_script() {
        let fake = FakeRig::spawn(14_230_000);
        let mut script = Script::default();
        script.set(Event::Close, vec![Command::new(["T", "0"]).unwrap()]);
        let worker = RigWorker::spawn(&RigSettings {
            script,
            ..settings(&fake.address)
        });
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        drop(worker);

        assert_eq!(fake.received(), ["+\\chk_vfo", "+T 0"]);
    }
}
