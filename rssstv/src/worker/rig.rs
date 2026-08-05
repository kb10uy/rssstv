use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub mod script;

use rssstv_rig::DEFAULT_TIMEOUT;

use crate::{
    storage::{
        bands::BandPlan,
        config::{PortSettings, RigSettings},
    },
    worker::rig::script::{Entry, ScriptError, ScriptHost},
};

/// What the rig was found to be tuned to.
///
/// The band is a name rather than a definition, because this crosses to the
/// interface and to a composition: what either does with it is print it or
/// compare it, and the settings behind it belong to the script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reading {
    pub frequency_hz: u64,
    pub mode: String,
    /// The band it falls in, named as the plan names it.
    pub band: Option<String>,
}

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
    /// The transports are being opened; nothing can be asked of them yet.
    Connecting,
    /// Connected, with the rig not keyed.
    Receiving,
    /// The script's transmit function ran and the lead-in has passed.
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
    SetFrequency(u64),
    ChangeBand(String),
}

/// The thread that owns the transports and the operator's script.
///
/// The interface never waits on it. Keying a rig means a socket round trip and
/// then a lead-in the rig needs to switch, and neither belongs in a frame.
pub struct RigWorker {
    snapshot: Arc<Mutex<RigSnapshot>>,
    /// Held as an option so that dropping the worker can close the channel,
    /// which is what tells the thread to call `close` and stop.
    requests: Option<Sender<Request>>,
    thread: Option<JoinHandle<()>>,
}

impl RigWorker {
    pub fn spawn(settings: &RigSettings, config_dir: &Path, bands: Arc<BandPlan>) -> Self {
        let snapshot = Arc::new(Mutex::new(RigSnapshot {
            state: RigState::Connecting,
            ..RigSnapshot::default()
        }));
        let (requests, incoming) = mpsc::channel();
        let worker_snapshot = Arc::clone(&snapshot);
        let plan = Plan::new(settings, config_dir, bands);
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

    /// Asks for the rig to be tuned.
    pub fn set_frequency(&self, target_hz: u64) {
        self.request(Request::SetFrequency(target_hz));
    }

    /// Asks for the rig to be moved to a band, named as the plan names it.
    pub fn change_band(&self, name: &str) {
        self.request(Request::ChangeBand(name.to_owned()));
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
    config_dir: PathBuf,
    ports: BTreeMap<String, PortSettings>,
    bands: Arc<BandPlan>,
    /// How long between frequency reads, or nothing to never read it.
    poll: Option<Duration>,
    lead_in: Duration,
    tail: Duration,
}

impl Plan {
    fn new(settings: &RigSettings, config_dir: &Path, bands: Arc<BandPlan>) -> Self {
        Self {
            config_dir: config_dir.to_path_buf(),
            ports: settings.ports.clone(),
            bands,
            poll: (settings.poll_seconds > 0.0)
                .then(|| Duration::from_secs_f32(settings.poll_seconds)),
            lead_in: Duration::from_secs_f32(settings.lead_in_seconds),
            tail: Duration::from_secs_f32(settings.tail_seconds),
        }
    }
}

fn rig_loop(plan: Plan, requests: &Receiver<Request>, snapshot: &Arc<Mutex<RigSnapshot>>) {
    let host = match ScriptHost::source(&plan.config_dir).and_then(|source| {
        ScriptHost::load(
            &source,
            &plan.ports,
            Arc::clone(&plan.bands),
            DEFAULT_TIMEOUT,
        )
    }) {
        Ok(host) => host,
        Err(error) => {
            update(snapshot, |state| {
                state.state = RigState::Failed;
                state.error = Some(error.to_string());
            });
            return;
        }
    };
    if let Err(error) = host.call(Entry::Open, None) {
        update(snapshot, |state| {
            state.state = RigState::Failed;
            state.error = Some(error.to_string());
        });
        return;
    }
    update(snapshot, |state| state.state = RigState::Receiving);
    let mut keyed = false;
    let mut frequency: Option<u64> = None;
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
                let outcome = host.call(Entry::Transmit, frequency);
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
                host.call(Entry::Receive, frequency)
            }
            // Tuning a keyed rig would move a transmission that is on the air.
            // The interface disables the controls while transmitting, so a
            // request arriving anyway is one to drop rather than report.
            Ok(Request::SetFrequency(_) | Request::ChangeBand(_)) if keyed => Ok(()),
            Ok(Request::SetFrequency(target_hz)) => host
                .set_frequency(frequency, target_hz)
                .and_then(|()| read_tuning(&host, &plan, snapshot, &mut frequency)),
            Ok(Request::ChangeBand(name)) => match plan.bands.by_name(&name) {
                Some(band) => host
                    .change_band(frequency, band)
                    .and_then(|()| read_tuning(&host, &plan, snapshot, &mut frequency)),
                // The plan was reloaded out from under the interface, which is
                // not a failure: the band it offered is simply gone.
                None => Ok(()),
            },
            Err(RecvTimeoutError::Timeout) if !keyed && plan.poll.is_some() => {
                read_tuning(&host, &plan, snapshot, &mut frequency)
            }
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Err(error) = outcome {
            // What decides whether to go on is the transport rather than the
            // error: a script may have caught the failure with `pcall` and
            // raised something of its own, but a socket that closed is closed.
            let stopping = host.transport_failed();
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
    // a close that failed leaves nothing behind to recover.
    if let Err(error) = host.call(Entry::Close, frequency) {
        update(snapshot, |state| state.error = Some(error.to_string()));
    }
    update(snapshot, |state| state.state = RigState::Disconnected);
}

/// Reads the frequency and mode, then publishes the band the frequency names.
///
/// Called after tuning as well as on the poll, so the interface shows where
/// the rig went rather than where it was until the next poll comes round.
fn read_tuning(
    host: &ScriptHost,
    plan: &Plan,
    snapshot: &Mutex<RigSnapshot>,
    frequency: &mut Option<u64>,
) -> Result<(), ScriptError> {
    let read = host.poll_frequency(*frequency)?;
    *frequency = read.as_ref().map(|(frequency_hz, _)| *frequency_hz);
    let reading = read.map(|(frequency_hz, mode)| Reading {
        frequency_hz,
        mode,
        band: plan
            .bands
            .for_frequency(frequency_hz)
            .map(|band| band.name.clone()),
    });
    update(snapshot, |state| state.reading = reading);
    Ok(())
}

fn update(snapshot: &Mutex<RigSnapshot>, update: impl FnOnce(&mut RigSnapshot)) {
    if let Ok(mut state) = snapshot.lock() {
        update(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{BufRead, BufReader, BufWriter, Write},
        net::TcpListener,
        time::Instant,
    };

    use super::*;
    use crate::{storage::bands::BandPlan, test_util::TempDir};

    fn with_script(source: &str) -> TempDir {
        let directory = TempDir::new();
        fs::write(directory.path().join(script::SCRIPT_FILE), source).unwrap();
        directory
    }

    /// A stand-in for `rigctld` that answers every command with success.
    ///
    /// Frequency reads are answered with a fixed frequency, so a test can
    /// watch what the script asks for without Hamlib being installed.
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
                    } else if request.contains("get_mode") {
                        "get_mode:\nMode: USB\nPassband: 2400\nRPRT 0\n".to_owned()
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

    /// The band plan the application ships, which is what a station that has
    /// written no bands.toml of its own runs on.
    fn plan() -> Arc<BandPlan> {
        Arc::new(BandPlan::built_in())
    }

    fn settings(address: &str) -> RigSettings {
        RigSettings {
            enabled: true,
            ports: BTreeMap::from([(
                "rig".to_owned(),
                PortSettings {
                    address: address.to_owned(),
                },
            )]),
            poll_seconds: 0.0,
            lead_in_seconds: 0.0,
            tail_seconds: 0.0,
        }
    }

    /// Waits for `predicate` to hold of the snapshot, or gives up.
    fn settle(worker: &RigWorker, predicate: impl Fn(&RigSnapshot) -> bool) -> RigSnapshot {
        let deadline = Instant::now() + Duration::from_secs(20);
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

    /// The script the application ships has to key a rig on its own, because
    /// it is what runs for every operator who has not written one.
    #[test]
    fn the_default_script_keys_and_unkeys() {
        let fake = FakeRig::spawn(14_230_000);
        let config = TempDir::new();
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        worker.receive();
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);
        drop(worker);

        assert_eq!(fake.received(), ["+\\chk_vfo", "+T 1", "+T 0"]);
    }

    /// A module that omits a function is not in error: a station keyed by VOX
    /// exports no `transmit`, and leaving it out is the whole of saying so.
    #[test]
    fn a_script_that_exports_nothing_keys_nothing_and_still_connects() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script("return {}");
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        drop(worker);

        assert_eq!(fake.received(), ["+\\chk_vfo"]);
    }

    /// The lead-in is what a rig needs to switch over, so nothing may call the
    /// rig ready to transmit until it has passed.
    #[test]
    fn the_lead_in_is_waited_out_before_the_rig_reports_it_is_transmitting() {
        let fake = FakeRig::spawn(14_230_000);
        let config = TempDir::new();
        let worker = RigWorker::spawn(
            &RigSettings {
                lead_in_seconds: 0.3,
                ..settings(&fake.address)
            },
            config.path(),
            plan(),
        );
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
        let config = TempDir::new();
        let worker = RigWorker::spawn(
            &RigSettings {
                poll_seconds: 0.02,
                ..settings(&fake.address)
            },
            config.path(),
            plan(),
        );

        let snapshot = settle(&worker, |snapshot| snapshot.reading.is_some());

        let reading = snapshot.reading.unwrap();
        assert_eq!(reading.frequency_hz, 7_178_000);
        assert_eq!(reading.mode, "USB");
        assert_eq!(reading.band.as_deref(), Some("40m"));
    }

    #[test]
    fn a_frequency_without_a_mode_is_not_a_reading() {
        let fake = FakeRig::spawn(7_178_000);
        let config = with_script(
            "return { poll_frequency = function(ctx) return ctx.ports.rig:frequency() end }",
        );
        let worker = RigWorker::spawn(
            &RigSettings {
                poll_seconds: 0.02,
                ..settings(&fake.address)
            },
            config.path(),
            plan(),
        );

        let snapshot = settle(&worker, |snapshot| snapshot.error.is_some());

        assert!(snapshot.reading.is_none());
        assert!(
            snapshot
                .error
                .as_deref()
                .is_some_and(|error| error.contains("both frequency and mode"))
        );
    }

    /// A keyed rig has to be left alone: a poll landing mid-transmission is
    /// CAT traffic during the one part of a session that cannot take it.
    #[test]
    fn a_keyed_rig_is_not_polled() {
        let fake = FakeRig::spawn(7_178_000);
        let config = TempDir::new();
        let worker = RigWorker::spawn(
            &RigSettings {
                poll_seconds: 0.02,
                ..settings(&fake.address)
            },
            config.path(),
            plan(),
        );
        settle(&worker, |snapshot| snapshot.reading.is_some());

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        let keyed_at = fake.received().len();
        thread::sleep(Duration::from_millis(200));

        assert_eq!(fake.received().len(), keyed_at);
    }

    /// The band the rig is on reaches the script, which is what lets one
    /// script cover a station's bands rather than one per band.
    #[test]
    fn the_script_is_told_which_band_the_rig_is_on() {
        let fake = FakeRig::spawn(7_178_000);
        let config = with_script(
            r#"
            return {
              poll_frequency = function(ctx)
                return ctx.ports.rig:frequency(), ctx.ports.rig:mode()
              end,
              transmit = function(ctx)
                ctx.ports.rig:send("BAND " .. (ctx.band and ctx.band.name or "none"))
              end,
            }
            "#,
        );
        let worker = RigWorker::spawn(
            &RigSettings {
                poll_seconds: 0.02,
                ..settings(&fake.address)
            },
            config.path(),
            plan(),
        );
        settle(&worker, |snapshot| snapshot.reading.is_some());

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);

        assert!(
            fake.received().contains(&"+BAND 40m".to_owned()),
            "{:?}",
            fake.received()
        );
    }

    /// A script that raises leaves the rig unkeyed, and the interface has to
    /// be told so that the transmission it was keyed for does not go out.
    #[test]
    fn a_script_that_raises_reports_the_failure_and_stays_connected() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script("return { transmit = function() error('no antenna') end }");
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        let snapshot = settle(&worker, |snapshot| snapshot.error.is_some());

        assert!(
            snapshot.error.as_deref().unwrap().contains("no antenna"),
            "{snapshot:?}"
        );
        // The script failed, not the connection, so rig control is still up.
        assert_eq!(snapshot.state, RigState::Receiving);
    }

    /// A script that does not finish would hold the worker, and with it every
    /// transmission after the one it was called for.
    #[test]
    fn a_script_that_never_finishes_is_abandoned() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script("return { transmit = function() while true do end end }");
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        let snapshot = settle(&worker, |snapshot| snapshot.error.is_some());

        assert!(
            snapshot.error.as_deref().unwrap().contains("too long"),
            "{snapshot:?}"
        );
        assert_ne!(snapshot.state, RigState::Transmitting);
    }

    #[test]
    fn a_script_that_does_not_return_a_module_is_reported() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script("return 3");
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());

        let snapshot = settle(&worker, |snapshot| snapshot.state == RigState::Failed);

        assert!(
            snapshot.error.as_deref().unwrap().contains("table"),
            "{snapshot:?}"
        );
    }

    /// Port 1 rather than one this test freed a moment ago: the tests here run
    /// alongside each other, and a port just released is one the next stand-in
    /// to ask for an ephemeral port can be handed, which would have this test
    /// connect to somebody else's rig and pass for the wrong reason.
    const NOTHING_LISTENING: &str = "127.0.0.1:1";

    #[test]
    fn a_rig_that_is_not_listening_is_reported_rather_than_waited_on() {
        let config = TempDir::new();

        let worker = RigWorker::spawn(&settings(NOTHING_LISTENING), config.path(), plan());

        let snapshot = settle(&worker, |snapshot| snapshot.state == RigState::Failed);
        assert!(snapshot.error.is_some());
    }

    /// A band is handed over whole, so a setting the operator invented reaches
    /// the script beside the ones the plan ships with.
    #[test]
    fn changing_band_hands_the_script_the_whole_band() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script(
            r#"
            return {
              change_band = function(ctx, band)
                ctx.ports.rig:send(("BAND %s %d %s %s"):format(
                  band.name, band.target, band.receive_mode, tostring(band.amplifier)))
              end,
            }
            "#,
        );
        let plan = BandPlan::parse(concat!(
            "[[bands]]\n",
            "name = \"40m\"\n",
            "low = 7000000\n",
            "high = 7300000\n",
            "target = 7171000\n",
            "receive-mode = \"LSB\"\n",
            "amplifier = true\n",
        ))
        .unwrap();
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), Arc::new(plan));
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.change_band("40m");
        settle(&worker, |_| {
            fake.received().iter().any(|line| line.starts_with("+BAND"))
        });

        assert!(
            fake.received()
                .contains(&"+BAND 40m 7171000 LSB true".to_owned()),
            "{:?}",
            fake.received()
        );
    }

    /// Tuning a keyed rig would move a transmission that is on the air.
    #[test]
    fn a_keyed_rig_is_not_tuned() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script(
            "return { set_frequency = function(ctx, hz) ctx.ports.rig:send('F ' .. hz) end }",
        );
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        worker.transmit();
        settle(&worker, |snapshot| snapshot.state == RigState::Transmitting);
        worker.set_frequency(14_240_000);
        thread::sleep(Duration::from_millis(200));

        assert!(
            !fake.received().iter().any(|line| line.starts_with("+F ")),
            "{:?}",
            fake.received()
        );
    }

    /// Closing is the operator's, and giving up the connection is what it
    /// hangs off.
    #[test]
    fn dropping_the_worker_calls_close() {
        let fake = FakeRig::spawn(14_230_000);
        let config = with_script("return { close = function(ctx) ctx.ports.rig:send('T 0') end }");
        let worker = RigWorker::spawn(&settings(&fake.address), config.path(), plan());
        settle(&worker, |snapshot| snapshot.state == RigState::Receiving);

        drop(worker);

        assert_eq!(fake.received(), ["+\\chk_vfo", "+T 0"]);
    }
}
