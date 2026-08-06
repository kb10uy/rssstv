//! Checks of the interface state that sits behind the widgets.

use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use rssstv_sstv::image::Rgb8;
use rstest::rstest;

use super::*;
use crate::{
    test_util::TempDir,
    worker::receive::{Frame, HistoryCandidate, RxSnapshot},
};
use rssstv_rig::RigError;

/// Builds an interface over real directories but no host audio.
fn disconnected(paths: AppPaths, settings: &Settings) -> App {
    let config = Config::load(paths.config_file());
    let mut app = App::from_parts(
        AudioState::disconnected(),
        paths,
        config,
        settings,
        Box::new(platform::QuietPlatform),
    );
    app.saved = app.settings();
    app
}

fn library(root: &TempDir) -> AppPaths {
    let paths = AppPaths::from_roots(
        root.path().join("config"),
        root.path().join("data"),
        root.path().join("pictures"),
        root.path().join("state"),
    );
    paths.initialize().unwrap();
    fs::write(paths.templates_dir().join("alpha.kdl"), "").unwrap();
    fs::write(paths.templates_dir().join("beta.kdl"), "").unwrap();
    image::RgbImage::new(7, 5)
        .save(paths.stocks_dir().join("first.png"))
        .unwrap();
    image::RgbImage::new(7, 5)
        .save(paths.stocks_dir().join("second.png"))
        .unwrap();
    paths
}

#[test]
fn only_supported_modes_are_selectable() {
    let app = App::headless();
    assert!(!app.rx_modes.is_empty());
    assert!(!app.tx_modes.is_empty());
    assert!(
        app.rx_modes
            .iter()
            .all(|mode| mode.spec().decode_support() == Support::Supported)
    );
    assert!(
        app.tx_modes
            .iter()
            .all(|mode| mode.spec().encode_support() == Support::Supported)
    );
}

fn decoding(rows: usize, total: usize) -> RxSnapshot {
    RxSnapshot {
        progress: RxProgress::Decoding { rows, total },
        display_fraction: RxProgress::Decoding { rows, total }.fraction(),
        ..RxSnapshot::default()
    }
}

fn identified(calls: &[&str]) -> RxSnapshot {
    RxSnapshot {
        callsigns: calls.iter().map(|call| (*call).to_owned()).collect(),
        ..RxSnapshot::default()
    }
}

fn numbered(numbers: &[&str]) -> RxSnapshot {
    RxSnapshot {
        numbers: numbers.iter().map(|number| (*number).to_owned()).collect(),
        ..RxSnapshot::default()
    }
}

#[rstest]
#[case(true, 1)]
#[case(false, 0)]
fn automatic_history_follows_its_setting(#[case] enabled: bool, #[case] expected_files: usize) {
    let root = TempDir::new();
    let paths = library(&root);
    let received = paths.received_dir().to_owned();
    let settings = Settings {
        auto_history: enabled,
        ..Settings::default()
    };
    let mut app = disconnected(paths, &settings);
    app.audio.set_snapshot(RxSnapshot {
        history: Some(HistoryCandidate {
            mode: Mode::Robot36,
            frame: Frame {
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 255],
            },
            received_at: "2026-08-04T12:34:56+09:00".to_owned(),
            fsk_ids: vec!["JA1ABC".to_owned()],
        }),
        ..RxSnapshot::default()
    });

    app.poll_workers();
    app.wait_for_history_writers();

    assert_eq!(fs::read_dir(received).unwrap().count(), expected_files);
}

/// The received image belongs to the template, not to the received folder,
/// so an operator who keeps nothing on disk still transmits over what was
/// just received.
#[rstest]
#[case(true)]
#[case(false)]
fn a_kept_reception_becomes_the_received_image(#[case] saving: bool) {
    let root = TempDir::new();
    let settings = Settings {
        auto_history: saving,
        ..Settings::default()
    };
    let mut app = disconnected(library(&root), &settings);
    app.audio.set_snapshot(RxSnapshot {
        history: Some(HistoryCandidate {
            mode: Mode::Robot36,
            frame: Frame {
                width: 2,
                height: 1,
                rgba: vec![10, 20, 30, 255, 40, 50, 60, 255],
            },
            received_at: "2026-08-04T12:34:56+09:00".to_owned(),
            fsk_ids: Vec::new(),
        }),
        ..RxSnapshot::default()
    });

    app.poll_workers();

    assert_eq!(app.composition.received_image.size().width(), 2);
    assert_eq!(app.composition.received_image.size().height(), 1);
    assert_eq!(
        app.composition.received_image.pixels().first(),
        Some(&Rgb8::new(10, 20, 30))
    );
}

/// A reception the worker never offered leaves the layer showing what it
/// showed before, so a lost signal does not blank a prepared transmission.
#[test]
fn a_reception_without_a_candidate_leaves_the_received_image_alone() {
    let mut app = App::headless();

    app.audio.set_snapshot(decoding(10, 100));
    app.poll_workers();

    assert_eq!(
        *app.composition.received_image,
        test_pattern_image(app.rx_mode)
    );
}

#[test]
fn a_decoded_identifier_fills_the_qso_contact_field() {
    let mut app = App::headless();

    app.audio.set_snapshot(identified(&["JA1ABC"]));
    app.poll_workers();

    assert_eq!(app.qso.call, "JA1ABC");
}

/// The worker republishes every identifier it has decoded, so the same
/// list observed again must not undo an edit made in the meantime.
#[test]
fn an_unchanged_identifier_list_leaves_the_contact_field_alone() {
    let mut app = App::headless();

    app.audio.set_snapshot(identified(&["JA1ABC"]));
    app.poll_workers();
    app.qso.call = "JA1XYZ".to_owned();
    app.poll_workers();

    assert_eq!(app.qso.call, "JA1XYZ");
}

/// The identifier names whoever is on the air now, so a new arrival takes
/// the field even when the operator had put something else there.
#[test]
fn a_newly_decoded_identifier_replaces_the_contact_field() {
    let mut app = App::headless();

    app.audio.set_snapshot(identified(&["JA1ABC"]));
    app.poll_workers();
    app.qso.call = "TYPED".to_owned();
    app.audio.set_snapshot(identified(&["JA1ABC", "JH1XYZ"]));
    app.poll_workers();

    assert_eq!(app.qso.call, "JH1XYZ");
}

/// Reopening a device restarts the worker with an empty list; the next
/// identifier it decodes is a new arrival even though the count went down.
#[test]
fn an_identifier_after_a_restart_is_adopted_again() {
    let mut app = App::headless();

    app.audio.set_snapshot(identified(&["JA1ABC", "JH1XYZ"]));
    app.poll_workers();
    app.audio.set_snapshot(RxSnapshot::default());
    app.poll_workers();
    app.audio.set_snapshot(identified(&["JA1ABC"]));
    app.poll_workers();

    assert_eq!(app.qso.call, "JA1ABC");
}

/// A sync-interval match can only narrow a mode down, never confirm it the
/// way a header does, so how far it may reach follows the operator's own
/// choice: it may pick any mode while automatic detection is on, and
/// otherwise only confirm the mode already selected.
#[test]
fn sync_start_scope_follows_automatic_mode_detection() {
    let mut app = App::headless();
    app.auto_mode = true;
    app.poll_workers();
    assert_eq!(app.audio.sync_start(), SyncStart::Any);

    app.auto_mode = false;
    app.select_rx_mode(Mode::Scottie1);
    app.poll_workers();
    assert_eq!(app.audio.sync_start(), SyncStart::Only(Mode::Scottie1));

    app.select_rx_mode(Mode::Pd120);
    app.poll_workers();
    assert_eq!(app.audio.sync_start(), SyncStart::Only(Mode::Pd120));
}

/// The FSKID alphabet includes the space, and an identifier that is only
/// spaces names nobody.
#[test]
fn a_blank_identifier_does_not_reach_the_contact_field() {
    let mut app = App::headless();
    app.qso.call = "JA1ABC".to_owned();

    app.audio.set_snapshot(identified(&["   "]));
    app.poll_workers();

    assert_eq!(app.qso.call, "JA1ABC");
}

/// The switch in the menu is the whole of the decision, so a worker has to
/// come and go with it rather than needing anything else to be asked.
#[test]
fn a_rig_worker_exists_exactly_while_rig_control_is_switched_on() {
    let mut app = App::headless();
    assert!(app.rig_worker.is_none());

    app.rig.enabled = true;
    app.poll_rig();
    assert!(app.rig_worker.is_some());

    app.rig.enabled = false;
    app.poll_rig();
    assert!(app.rig_worker.is_none());
    assert_eq!(app.rig_snapshot.state, RigState::Disconnected);
}

/// The lead-in is time anything sent into the rig is lost, so a keyed
/// transmission has to wait out the rig before any audio leaves.
#[rstest]
#[case::nothing_was_keyed(false, RigState::Receiving, true)]
#[case::still_switching_over(true, RigState::Receiving, false)]
#[case::keyed_and_settled(true, RigState::Transmitting, true)]
fn audio_waits_until_the_rig_says_it_is_transmitting(
    #[case] keyed: bool,
    #[case] state: RigState,
    #[case] expected: bool,
) {
    let mut app = App::headless();
    app.rig_keyed = keyed;
    app.rig_snapshot.state = state;

    assert_eq!(app.rig_ready_to_send(), expected);
}

/// Transmitting into a rig that was asked to key and did not is a
/// transmission nobody hears, so it is refused rather than sent.
#[rstest]
#[case::not_connected_yet(true, RigState::Connecting, true)]
#[case::failed(true, RigState::Failed, true)]
#[case::ready(true, RigState::Receiving, false)]
#[case::keyed(true, RigState::Transmitting, false)]
// Nothing was asked of the rig, so nothing it says can stop a transmission.
#[case::switched_off(false, RigState::Failed, false)]
fn rig_control_that_is_not_ready_stops_a_transmission(
    #[case] enabled: bool,
    #[case] state: RigState,
    #[case] blocked: bool,
) {
    let mut app = App::headless();
    app.rig.enabled = enabled;
    app.rig_snapshot.state = state;

    assert_eq!(app.rig_problem().is_some(), blocked);
}

/// What the rig said is more use than the state it ended up in, so the
/// failure is what the operator is shown when there is one.
#[test]
fn a_rig_failure_is_reported_in_the_rig_s_own_words() {
    let mut app = App::headless();
    app.rig.enabled = true;
    app.rig_snapshot.state = RigState::Failed;
    app.rig_snapshot.error = Some(AppError::Rig(RigError::Connect {
        address: "127.0.0.1:4532".to_owned(),
        detail: "connection refused".to_owned(),
    }));

    let problem = app.rig_problem().unwrap();

    assert!(problem.contains("connection refused"), "{problem}");
}

fn tuned_to(app: &mut App, frequency_hz: u64) {
    app.rig.enabled = true;
    app.rig_snapshot.state = RigState::Receiving;
    app.rig_snapshot.reading = Some(Reading {
        frequency_hz,
        mode: "USB".to_owned(),
        band: app
            .bands
            .for_frequency(frequency_hz)
            .map(|band| band.name.clone()),
    });
}

/// Stepping out of the band is not tuning, so the edge is where it stops
/// rather than somewhere the operator may not be licensed to be.
#[rstest]
#[case::down(7_100_000, -1, Some(7_099_000))]
#[case::up(7_100_000, 1, Some(7_101_000))]
#[case::several(7_100_000, 5, Some(7_105_000))]
#[case::at_the_bottom_edge(7_000_000, -1, None)]
#[case::at_the_top_edge(7_300_000, 1, None)]
// Between the bands there is no plan entry, and so no step to take.
#[case::off_band(6_000_000, 1, None)]
fn stepping_stays_inside_the_band(
    #[case] frequency_hz: u64,
    #[case] steps: i64,
    #[case] expected: Option<u64>,
) {
    let mut app = App::headless();
    tuned_to(&mut app, frequency_hz);

    assert_eq!(app.stepped_frequency(steps), expected);
}

#[test]
fn a_band_with_no_step_has_nothing_to_move_by() {
    let mut app = App::headless();
    app.bands = Arc::new(
        BandPlan::parse("[[bands]]\nname = \"40m\"\nlow = 7000000\nhigh = 7300000\n").unwrap(),
    );
    tuned_to(&mut app, 7_100_000);

    assert_eq!(app.stepped_frequency(1), None);
}

/// Moving a rig that is on the air moves the transmission with it.
#[rstest]
#[case(RigState::Receiving, true)]
#[case(RigState::Transmitting, false)]
#[case(RigState::Connecting, false)]
#[case(RigState::Failed, false)]
fn a_keyed_rig_is_not_tunable_from_the_interface(#[case] state: RigState, #[case] expected: bool) {
    let mut app = App::headless();
    app.rig_snapshot.state = state;

    assert_eq!(app.can_tune(), expected);
}

/// A platform that records what the interface asked it for.
#[derive(Clone, Default)]
struct RecordingPlatform(Rc<RefCell<Vec<Activity>>>);

impl Platform for RecordingPlatform {
    fn set_activity(&mut self, activity: Activity) {
        self.0.borrow_mut().push(activity);
    }

    fn open_path(&mut self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

/// Sleep has to be held off for the whole of a reception and released
/// again when it ends, or a long picture is cut short by an idle timer.
#[test]
fn a_reception_asks_the_platform_to_stay_awake_until_it_ends() {
    let recorder = RecordingPlatform::default();
    let mut app = App::headless_on(Box::new(recorder.clone()));

    app.audio.set_snapshot(decoding(40, 100));
    app.poll_workers();
    assert_eq!(app.activity(), Activity::Receiving);

    app.audio.set_snapshot(RxSnapshot::default());
    app.poll_workers();
    assert_eq!(app.activity(), Activity::Idle);
    assert_eq!(
        recorder.0.take(),
        vec![Activity::Receiving, Activity::Idle],
        "each transition should be reported once"
    );
}

/// An unchanged activity must not be restated, or the platform is asked
/// the same thing on every frame.
#[test]
fn an_unchanged_activity_is_not_reported_again() {
    let recorder = RecordingPlatform::default();
    let mut app = App::headless_on(Box::new(recorder.clone()));

    app.audio.set_snapshot(decoding(10, 100));
    app.poll_workers();
    app.audio.set_snapshot(decoding(20, 100));
    app.poll_workers();

    assert_eq!(recorder.0.take(), vec![Activity::Receiving]);
}

/// A transmission outranks a reception: both cannot hold the device, and
/// the transmission is the stronger claim on the machine.
#[rstest]
#[case(TxPhase::Priming, Activity::Transmitting)]
#[case(TxPhase::Producing, Activity::Transmitting)]
#[case(TxPhase::Draining, Activity::Transmitting)]
#[case(TxPhase::Complete, Activity::Receiving)]
#[case(TxPhase::Cancelled, Activity::Receiving)]
fn a_transmission_outranks_a_reception(#[case] phase: TxPhase, #[case] expected: Activity) {
    let mut app = App::headless();
    app.audio.set_snapshot(decoding(40, 100));
    app.tx_snapshot = TxSnapshot {
        phase,
        ..TxSnapshot::default()
    };

    app.poll_workers();

    assert_eq!(app.activity(), expected);
}

/// The station's own signal comes back off the antenna, so nothing is
/// listened for while anything is going out. Every ending releases it,
/// including one the operator did not ask for.
#[rstest]
#[case(TxPhase::Priming, true)]
#[case(TxPhase::Producing, true)]
#[case(TxPhase::Draining, true)]
#[case(TxPhase::Complete, false)]
#[case(TxPhase::Cancelled, false)]
#[case(TxPhase::Failed, false)]
fn reception_is_muted_for_as_long_as_a_transmission_runs(
    #[case] phase: TxPhase,
    #[case] expected: bool,
) {
    let mut app = App::headless();
    app.tx_snapshot = TxSnapshot {
        phase,
        ..TxSnapshot::default()
    };

    app.poll_workers();

    assert_eq!(app.audio.is_muted_for_transmit(), expected);
}

/// A tone keys the rig just as a picture does, and the operator stopping it
/// is what gives reception back.
#[test]
fn a_tune_tone_mutes_reception_until_it_is_stopped() {
    let mut app = App::headless();
    app.poll_workers();
    assert!(!app.audio.is_muted_for_transmit());

    app.tune_for_test();
    app.poll_workers();
    assert!(app.audio.is_muted_for_transmit());

    app.stop_tune();
    app.poll_workers();

    assert!(!app.audio.is_muted_for_transmit());
}

#[test]
fn switching_tabs_preserves_receive_progress() {
    let mut app = App::headless();
    app.audio.set_snapshot(decoding(40, 100));
    app.tab = Tab::Transmit;
    app.tab = Tab::Receive;
    assert_eq!(app.decoded_fraction(), 0.4);
}

/// A tone sends no picture, so the one on the tab is not drawn out under it.
#[test]
fn a_tune_tone_leaves_the_transmit_raster_whole() {
    let mut app = App::headless();
    app.tab = Tab::Transmit;
    app.tune_for_test();

    assert_eq!(app.decoded_fraction(), 1.0);
}

/// The tone asks for less than a picture does: it carries no image and names
/// no station, so only the output device and the rig stand in its way.
#[test]
fn a_tune_tone_needs_only_an_output_device() {
    let mut app = App::headless();
    app.station.callsign = String::new();
    assert!(app.composition.frame.is_none());

    assert_eq!(
        app.tune_problem(),
        Some(app.i18n.text("error-no-output-device"))
    );

    app.start_tune();

    assert!(!app.is_tuning());
    assert_eq!(app.tx_error, Some(app.i18n.text("error-no-output-device")));
}

/// One stream and one rig: whichever of the two is running refuses the other.
#[test]
fn a_tone_and_a_picture_refuse_each_other() {
    let mut app = App::headless();
    app.tx_snapshot.phase = TxPhase::Producing;

    assert_eq!(
        app.tune_problem(),
        Some(app.i18n.text("error-transmit-active"))
    );

    app.tune_for_test();

    assert_eq!(
        app.transmit_problem(),
        Some(app.i18n.text("error-tone-active"))
    );
    assert!(!app.can_transmit());
}

/// A keyed carrier is the one thing here that goes out with nobody watching
/// it, so it gives the rig back on its own.
#[test]
fn a_tune_tone_stops_itself_once_its_time_is_up() {
    let mut app = App::headless();
    app.tune_for_test();
    app.tune_until = Some(Instant::now() - Duration::from_millis(1));

    app.poll_transmit();

    assert!(!app.is_tuning());
    assert_eq!(app.tx_snapshot.phase, TxPhase::Complete);
}

#[test]
fn stopping_a_tune_tone_ends_the_transmission() {
    let mut app = App::headless();
    app.tune_for_test();

    app.stop_tune();

    assert!(!app.is_tuning());
    assert_eq!(app.tx_snapshot.phase, TxPhase::Cancelled);
}

#[test]
fn an_idle_transmit_tab_draws_a_full_raster() {
    let mut app = App::headless();
    app.audio.set_snapshot(decoding(40, 100));
    app.tab = Tab::Transmit;
    assert_eq!(app.decoded_fraction(), 1.0);
}

#[test]
fn an_idle_receiver_draws_nothing_as_decoded() {
    assert_eq!(App::headless().decoded_fraction(), 0.0);
}

#[test]
fn waiting_receiver_keeps_the_retained_frame_fraction() {
    let mut app = App::headless();
    app.audio.set_snapshot(RxSnapshot {
        display_fraction: 0.65,
        ..RxSnapshot::default()
    });
    assert_eq!(app.decoded_fraction(), 0.65);
}

#[test]
fn selecting_a_mode_replaces_the_raster() {
    let mut app = App::headless();
    app.select_rx_mode(Mode::Robot36);
    assert_eq!(app.rx_mode, Mode::Robot36);
    assert_eq!(
        app.rx_raster.size().width(),
        Mode::Robot36.spec().width() as usize
    );
    assert_eq!(
        app.rx_raster.size().height(),
        Mode::Robot36.spec().height() as usize
    );
}

#[rstest]
#[case(Dsp::Afc, false)]
#[case(Dsp::Lms, true)]
#[case(Dsp::Slant, false)]
fn dsp_toggles_flip_one_flag(#[case] dsp: Dsp, #[case] expected: bool) {
    let mut app = App::headless();
    app.toggle_dsp(dsp);
    assert_eq!(app.dsp.get(dsp), expected);
    if dsp == Dsp::Slant {
        assert_eq!(app.audio.live_slant(), expected);
    }
}

/// A frame that prints the clock is composed again as the minute turns,
/// and one that does not is left alone however long it sits there.
#[test]
fn only_a_composition_that_shows_the_time_is_made_again() {
    let mut app = App::headless();
    let stale = current_minute() - 1;

    app.composition.shows_clock = false;
    app.composition.minute = stale;
    app.refresh_timed_composition();
    assert_eq!(app.composition.minute, stale);

    app.composition.shows_clock = true;
    app.refresh_timed_composition();
    assert_eq!(app.composition.minute, current_minute());
}

/// A half-typed name is still in the dialog to be finished, but it is not
/// offered to a template, which could only fail to read it.
#[test]
fn an_unusable_custom_variable_name_is_kept_out_of_the_composition() {
    let mut app = App::headless();
    app.variables_draft = vec![
        ("club".to_owned(), "JARL".to_owned()),
        ("2m rig".to_owned(), "FT-991A".to_owned()),
    ];

    app.commit_custom_variables();

    assert_eq!(
        app.custom_variables,
        BTreeMap::from([("club".to_owned(), "JARL".to_owned())])
    );
    assert_eq!(app.variables_draft.len(), 2);
}

#[test]
fn slant_is_enabled_by_default_in_the_ui_and_worker_settings() {
    let app = App::headless();
    assert!(app.dsp.live_slant);
    assert!(app.audio.live_slant());
}

/// The setting reaches the receive worker rather than only the menu mark,
/// or turning it off would leave the reception restarting anyway.
#[test]
fn the_vis_restart_setting_reaches_the_receive_worker() {
    let mut app = App::headless();
    assert!(app.vis_restart);
    assert!(app.audio.vis_restart());

    app.set_vis_restart(false);

    assert!(!app.audio.vis_restart());
}

/// The setting decides whether the identifier is sent, not whether the
/// station needs a callsign: a transmission is made by a station either
/// way, and nothing else names it.
#[test]
fn the_identifier_setting_decides_only_whether_the_callsign_is_sent() {
    let mut app = App::headless();
    app.station.callsign = "!".to_owned();
    assert!(app.send_fskid);
    assert!(matches!(app.station_id(), Some(Err(_))));

    app.send_fskid = false;

    // Nothing is sent now, but the station still has to be named: the
    // callsign is reported before anything else a transmission needs.
    assert!(app.station_id().is_none());
    let problem = app.transmit_problem().expect("an unusable callsign");
    assert_ne!(problem, app.i18n.text("error-no-transmit-frame"));

    app.station.callsign = "JA1ABC".to_owned();

    assert_eq!(
        app.transmit_problem(),
        Some(app.i18n.text("error-no-transmit-frame"))
    );
}

/// The station dialog writes the same fields the composition reads, and a
/// callsign typed into it reaches the transmit check uppercased.
#[test]
fn station_details_are_kept_and_normalized() {
    let mut app = App::headless();

    app.station.callsign = " ja1abc ".to_owned();
    app.station.qth = "  Chiyoda, Tokyo ".to_owned();
    app.station.grid = " PM95uq".to_owned();
    app.normalize_station();

    assert_eq!(app.station.callsign, "JA1ABC");
    let settings = app.settings();
    assert_eq!(settings.station_callsign, "JA1ABC");
    assert_eq!(settings.station_qth, "Chiyoda, Tokyo");
    assert_eq!(settings.station_grid, "PM95uq");
}

/// Every field but the operator's own template variables is a value rather
/// than text, so what is taken up when one is left is the trimmed field.
#[test]
fn leaving_a_qso_field_trims_it() {
    let mut app = App::headless();
    app.qso.call = " JA1ABC ".to_owned();
    app.qso.rsv = " 595 ".to_owned();
    app.qso.rsv_received = "595  ".to_owned();
    app.qso.number = " 007".to_owned();

    app.finish_qso_edit();

    assert_eq!(app.qso.call, "JA1ABC");
    assert_eq!(app.qso.rsv, "595");
    assert_eq!(app.qso.rsv_received, "595");
    assert_eq!(app.qso.number, "007");
}

/// What the operator wrote is what a template reads, spaces and all: these
/// are the values only they know the shape of.
#[test]
fn custom_variables_are_kept_exactly_as_they_were_entered() {
    let mut app = App::headless();
    app.variables_draft = vec![("club".to_owned(), "  JARL  ".to_owned())];

    app.commit_custom_variables();

    assert_eq!(
        app.custom_variables.get("club").map(String::as_str),
        Some("  JARL  ")
    );
}

#[test]
fn callsign_input_is_normalized() {
    let mut app = App::headless();
    app.qso.call = "ja1xyz".to_owned();
    app.normalize_call();
    assert_eq!(app.qso.call, "JA1XYZ");
}

/// The serial keeps its three digits as it counts, and is allowed to grow
/// out of them rather than starting over.
#[rstest]
#[case("001", "002")]
#[case("009", "010")]
#[case("099", "100")]
#[case("999", "1000")]
fn the_serial_number_counts_on(#[case] before: &str, #[case] after: &str) {
    let mut app = App::headless();
    app.qso.number = before.to_owned();

    app.increment_number();

    assert_eq!(app.qso.number, after);
}

/// An exchange that is not a serial has nothing to count, so the button
/// leaves it as it was typed.
#[test]
fn a_serial_that_is_not_a_number_is_left_alone() {
    let mut app = App::headless();
    app.qso.number = "13H".to_owned();

    app.increment_number();

    assert_eq!(app.qso.number, "13H");
}

#[test]
fn the_serial_number_is_reset_and_kept() {
    let mut app = App::headless();
    app.qso.number = "042".to_owned();
    assert_eq!(app.settings().qso_number, "042");

    app.reset_number();
    assert_eq!(app.qso.number, FIRST_QSO_NUMBER);
}

/// The number is filtered the way MMSSTV filters it, and one the record
/// cannot hold is left off rather than stopping the transmission.
#[rstest]
#[case("001", Some("001"))]
#[case("13h", Some("13H"))]
#[case(" 42 ", Some("42"))]
#[case("100-2", Some("1002"))]
#[case("", None)]
#[case("123456789", None)]
fn the_contest_number_is_sent_as_the_record_can_hold_it(
    #[case] typed: &str,
    #[case] expected: Option<&str>,
) {
    let mut app = App::headless();
    app.contest_mode = true;
    app.qso.number = typed.to_owned();

    let number = app.contest_number();

    assert_eq!(
        number.map(|number| number.as_str().to_owned()).as_deref(),
        expected
    );
}

/// A station that is not in a contest gives out no number, whatever is left
/// in the field from the last one.
#[test]
fn no_contest_number_is_sent_outside_contest_mode() {
    let mut app = App::headless();
    app.qso.number = "001".to_owned();

    assert!(!app.contest_mode);
    assert_eq!(app.contest_number(), None);
}

/// A contest number arrives as digits alone, and is read as the report
/// MMSSTV reads it as.
#[test]
fn a_decoded_contest_number_fills_the_received_report() {
    let mut app = App::headless();

    app.audio.set_snapshot(numbered(&["001"]));
    app.poll_workers();

    assert_eq!(app.qso.rsv_received, "595001");
}

/// The worker republishes every number it has decoded, so the same list
/// observed again must not undo an edit made in the meantime.
#[test]
fn an_unchanged_number_list_leaves_the_received_report_alone() {
    let mut app = App::headless();

    app.audio.set_snapshot(numbered(&["001"]));
    app.poll_workers();
    app.qso.rsv_received = "599".to_owned();
    app.poll_workers();

    assert_eq!(app.qso.rsv_received, "599");
}

/// A restarted worker publishes an empty list, and the next number it
/// decodes is a new arrival even though the count went down.
#[test]
fn a_number_after_a_restart_is_adopted_again() {
    let mut app = App::headless();

    app.audio.set_snapshot(numbered(&["001", "002"]));
    app.poll_workers();
    assert_eq!(app.qso.rsv_received, "595002");
    app.audio.set_snapshot(RxSnapshot::default());
    app.poll_workers();
    app.audio.set_snapshot(numbered(&["001"]));
    app.poll_workers();

    assert_eq!(app.qso.rsv_received, "595001");
}

#[test]
fn locale_switching_replaces_the_bundle() {
    let mut app = App::headless();
    let english = app.i18n.text("tab-receive");
    app.select_locale(Locale::Ja);
    assert_eq!(app.i18n.locale(), Locale::Ja);
    assert_ne!(app.i18n.text("tab-receive"), english);
}

#[test]
fn changed_settings_are_written_and_restored_on_the_next_start() {
    let root = TempDir::new();
    let paths = library(&root);

    let mut app = disconnected(paths.clone(), &Settings::default());
    app.refresh_library();
    app.select_locale(Locale::Ja);
    app.select_rx_mode(Mode::Robot36);
    app.toggle_dsp(Dsp::Lms);
    app.auto_mode = false;
    app.auto_history = false;
    app.history_format = crate::storage::history::HistoryFormat::Jpeg;
    app.select_tx_mode(Mode::Martin1);
    app.station.callsign = "JA1ABC".to_owned();
    app.library.template = Some(1);
    app.library.stock = Some(1);
    app.persist();
    assert!(app.config_error().is_none());

    let restored = Config::load(paths.config_file()).settings();
    let mut next = disconnected(paths.clone(), &restored);
    next.refresh_library();
    next.restore_selection(&restored);

    assert_eq!(next.i18n.locale(), Locale::Ja);
    assert_eq!(next.rx_mode, Mode::Robot36);
    assert_eq!(next.tx_mode, Mode::Martin1);
    assert!(next.dsp.lms_filter);
    assert!(!next.auto_mode);
    assert!(!next.auto_history);
    assert_eq!(
        next.history_format,
        crate::storage::history::HistoryFormat::Jpeg
    );
    assert_eq!(next.station.callsign, "JA1ABC");
    assert_eq!(
        next.library.templates[next.library.template.unwrap()].name,
        "beta.kdl"
    );
    assert_eq!(
        next.library.stocks[next.library.stock.unwrap()].name,
        "second.png"
    );
}

#[test]
fn a_stored_selection_that_disappeared_falls_back_to_the_first_entry() {
    let root = TempDir::new();
    let paths = library(&root);
    let settings = Settings {
        template: Some("gone.kdl".to_owned()),
        ..Settings::default()
    };

    let mut app = disconnected(paths, &settings);
    app.refresh_library();
    app.restore_selection(&settings);

    assert_eq!(
        app.library.templates[app.library.template.unwrap()].name,
        "alpha.kdl"
    );
}

#[test]
fn transient_state_is_not_written_to_the_configuration_file() {
    let root = TempDir::new();
    let paths = library(&root);

    let mut app = disconnected(paths.clone(), &Settings::default());
    app.refresh_library();
    app.saved = app.settings();
    app.tab = Tab::Transmit;
    app.qso.call = "JA1XYZ".to_owned();
    app.poll_workers();
    app.persist();

    assert_eq!(fs::read_to_string(paths.config_file()).unwrap(), "");
}

#[test]
fn an_unchanged_frame_does_not_rewrite_the_configuration_file() {
    let root = TempDir::new();
    let paths = library(&root);

    let mut app = disconnected(paths.clone(), &Settings::default());
    app.refresh_library();
    app.saved = app.settings();
    app.select_locale(Locale::Ja);
    app.persist();
    let written = fs::metadata(paths.config_file())
        .unwrap()
        .modified()
        .unwrap();

    app.persist();
    app.persist();

    assert_eq!(
        fs::metadata(paths.config_file())
            .unwrap()
            .modified()
            .unwrap(),
        written
    );
}

#[test]
fn library_refresh_lists_matching_files_and_preserves_selection() {
    let root = TempDir::new();
    let paths = AppPaths::from_roots(
        root.path().join("config"),
        root.path().join("data"),
        root.path().join("pictures"),
        root.path().join("state"),
    );
    paths.initialize().unwrap();
    fs::write(paths.templates_dir().join("beta.kdl"), "").unwrap();
    fs::write(paths.templates_dir().join("alpha.KDL"), "").unwrap();
    fs::write(paths.templates_dir().join("ignored.txt"), "").unwrap();
    image::RgbImage::new(7, 5)
        .save(paths.stocks_dir().join("valid.png"))
        .unwrap();
    fs::write(paths.stocks_dir().join("broken.png"), "not an image").unwrap();

    let mut app = disconnected(paths.clone(), &Settings::default());
    app.refresh_library();

    assert_eq!(
        app.library
            .templates
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha.KDL", "beta.kdl"]
    );
    assert_eq!(app.library.template, Some(0));
    assert_eq!(app.library.stocks.len(), 1);
    assert_eq!(app.library.stocks[0].name, "valid.png");
    assert_eq!(app.library.stocks[0].geometry, "7×5");

    app.library.template = Some(1);
    fs::write(paths.templates_dir().join("aardvark.kdl"), "").unwrap();
    app.refresh_templates();
    app.wait_for_library_scan();

    assert_eq!(
        app.library
            .template
            .map(|index| app.library.templates[index].name.as_str()),
        Some("beta.kdl")
    );
}

#[test]
fn qso_changes_do_not_invalidate_template_or_stock_files() {
    let mut app = App::headless();
    let template_generation = app.composition.template_generation;
    let stock_generation = app.composition.stock_generation;

    app.qso_changed();

    assert_eq!(app.composition.template_generation, template_generation);
    assert_eq!(app.composition.stock_generation, stock_generation);

    app.template_changed();
    assert_eq!(
        app.composition.template_generation,
        template_generation.wrapping_add(1)
    );
    assert_eq!(app.composition.stock_generation, stock_generation);

    app.stock_changed();
    assert_eq!(
        app.composition.stock_generation,
        stock_generation.wrapping_add(1)
    );
}

/// Polls until the composition worker has delivered the transmit image.
fn composed(app: &mut App) -> Arc<RgbImage> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_workers();
        assert!(app.tx_error.is_none(), "{:?}", app.tx_error);
        assert!(Instant::now() < deadline, "no composition arrived");
        if let Some(frame) = app.composition.frame.clone() {
            return frame;
        }
        std::thread::yield_now();
    }
}

/// A pixel from the middle of the frame, away from any resampled edge.
fn center(frame: &RgbImage) -> Rgb8 {
    let size = frame.size();
    frame.row(size.height() / 2).expect("a middle row")[size.width() / 2]
}

#[test]
fn a_composed_image_becomes_the_transmit_image() {
    let root = TempDir::new();
    let paths = library(&root);
    fs::write(
        paths.templates_dir().join("alpha.kdl"),
        r##"rximage {
position x=(fw)0 y=(fh)0
size width=(fw)100 height=(fh)100 fit="stretch"
}
"##,
    )
    .unwrap();
    let settings = Settings {
        station_callsign: "JA1ABC".to_owned(),
        ..Settings::default()
    };
    let mut app = disconnected(paths, &settings);
    app.refresh_library();
    app.request_composition();

    composed(&mut app);

    assert_eq!(
        app.tx_raster.size().width(),
        app.tx_mode.spec().width() as usize
    );
    assert_eq!(
        app.tx_raster.size().height(),
        app.tx_mode.spec().height() as usize
    );
    assert!(app.transmit_problem().is_none() || app.audio.output_device.is_none());
}

/// A kept reception reaches the transmit image on its own, so the operator
/// does not have to touch the library to send what was just received.
#[test]
fn a_kept_reception_reaches_the_transmit_image() {
    let root = TempDir::new();
    let paths = library(&root);
    fs::write(
        paths.templates_dir().join("alpha.kdl"),
        r##"rximage {
position x=(fw)0 y=(fh)0
size width=(fw)100 height=(fh)100 fit="stretch"
}
"##,
    )
    .unwrap();
    let settings = Settings {
        auto_history: false,
        ..Settings::default()
    };
    let mut app = disconnected(paths, &settings);
    app.refresh_library();
    app.audio.set_snapshot(RxSnapshot {
        history: Some(HistoryCandidate {
            mode: Mode::Robot36,
            frame: Frame {
                width: 1,
                height: 1,
                rgba: vec![200, 10, 20, 255],
            },
            received_at: "2026-08-04T12:34:56+09:00".to_owned(),
            fsk_ids: Vec::new(),
        }),
        ..RxSnapshot::default()
    });

    app.poll_workers();

    // The stock images the library holds are black, so a composition made
    // of the received color could not have come from the background.
    let frame = composed(&mut app);
    assert_eq!(frame.pixels().first(), Some(&Rgb8::new(200, 10, 20)));
}

/// The transmit image is what a transmission is sending, so a stock chosen
/// while one runs must not replace it until the transmission is over.
#[test]
fn a_selection_during_a_transmission_takes_effect_when_it_ends() {
    let root = TempDir::new();
    let paths = library(&root);
    image::RgbImage::from_pixel(8, 6, image::Rgb([200, 30, 30]))
        .save(paths.stocks_dir().join("first.png"))
        .unwrap();
    image::RgbImage::from_pixel(8, 6, image::Rgb([30, 200, 30]))
        .save(paths.stocks_dir().join("second.png"))
        .unwrap();
    let mut app = disconnected(paths, &Settings::default());
    app.refresh_library();
    app.request_composition();
    let first = composed(&mut app);
    assert!(center(&first).r > center(&first).g);

    app.tx_snapshot.phase = TxPhase::Producing;
    app.library.stock = Some(1);
    app.stock_changed();

    app.poll_workers();
    assert_eq!(app.composition.frame.as_deref(), Some(first.as_ref()));

    app.stop_transmit();

    let second = composed(&mut app);
    assert!(center(&second).g > center(&second).r);
}

/// An interface with nothing running asks for no frames at all.
///
/// This is the whole point of the workers asking for their own: a station
/// sitting idle has nothing to draw, and drawing it anyway is what kept the
/// machine busy.
#[test]
fn an_idle_interface_schedules_no_frames() {
    let app = App::headless();
    assert_eq!(app.repaint_after(), None);
}

/// A transmission is read by polling its worker and the playback queue, so it
/// has to be looked at rather than waited on.
#[test]
fn a_running_tone_schedules_frames() {
    let mut app = App::headless();
    app.tune_for_test();
    assert_eq!(app.repaint_after(), Some(LIVE_INTERVAL));
}

/// The soonest of the reasons to draw is the one that decides.
#[test]
fn the_shortest_interval_wins() {
    let mut app = App::headless();
    app.rig.enabled = true;
    assert_eq!(app.repaint_after(), Some(WATCH_INTERVAL));

    app.tune_for_test();
    assert_eq!(app.repaint_after(), Some(LIVE_INTERVAL));
}

/// Writing a file out is a success, and the status line must not dress it in
/// the error color a failure gets.
#[test]
fn a_written_file_reports_as_a_notice_rather_than_a_failure() {
    let mut app = App::headless();
    app.report_written(Ok(std::path::PathBuf::from("rig.lua")));
    assert!(app.notice.is_some());
    assert!(app.library.error.is_none());

    app.report_written(Err(std::io::Error::other("disk full")));
    assert!(app.notice.is_none());
    assert_eq!(app.library.error.as_deref(), Some("disk full"));
}
