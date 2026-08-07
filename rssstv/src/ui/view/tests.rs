//! Checks that the interface offers what it should, driven through egui's own
//! test harness so what is asserted is what a widget actually reports.

use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rstest::rstest;

use super::*;
use crate::{
    app::{App, FIRST_QSO_NUMBER},
    i18n::Locale,
};
use rssstv_rig::RigError;

use super::panels::decibels;

/// Runs the interface for a few frames and returns the harness.
///
/// egui reports duplicate widget ids and bad layouts by panicking at run
/// time rather than at compile time, so the whole view is exercised here:
/// nothing else in the suite would notice.
fn render(app: &mut App) -> Harness<'_> {
    let mut harness = Harness::new_ui(|ui| {
        let model = menu::model(app);
        view(ui, app, &model, menu::is_in_window());
    });
    harness.run();
    harness
}

#[rstest]
#[case(Tab::Receive)]
#[case(Tab::Transmit)]
fn every_tab_renders(#[case] tab: Tab) {
    let mut app = App::headless();
    app.tab = tab;
    let harness = render(&mut app);
    harness.get_by_label(&app_label(tab));
}

fn app_label(tab: Tab) -> String {
    crate::i18n::I18n::new(Locale::default()).text(tab.label_key())
}

#[rstest]
#[case(Locale::En)]
#[case(Locale::Ja)]
fn every_locale_renders(#[case] locale: Locale) {
    let mut app = App::headless();
    app.select_locale(locale);
    let harness = render(&mut app);
    let receive = crate::i18n::I18n::new(locale).text("tab-receive");
    harness.get_by_label(&receive);
}

/// Every row of the variable dialog carries the same three widgets, which
/// is exactly the shape egui panics over if they are not given ids of
/// their own.
#[test]
fn the_variable_dialog_renders_a_row_for_every_name() {
    let mut app = App::headless();
    app.custom_variables = std::collections::BTreeMap::from([
        ("club".to_owned(), "JARL".to_owned()),
        ("rig".to_owned(), "FT-991A".to_owned()),
    ]);
    app.open_custom_variables();
    assert_eq!(app.variables_draft.len(), 2);
    let title = app.i18n.text("custom-title");

    let harness = render(&mut app);

    harness.get_by_label(&title);
}

/// The panel keeps the same controls in every connection state so its
/// contents do not move while a connection is established or lost.
#[test]
fn the_radio_panel_is_where_the_connection_is_worked() {
    let mut app = App::headless();
    let title = app.i18n.text("section-radio");
    let connect = app.i18n.text("action-rig-connect");
    let disconnect = app.i18n.text("action-rig-disconnect");
    let retry = app.i18n.text("action-rig-retry");
    let unknown = app.i18n.text("rig-frequency-unknown");
    let mode = "LSB";
    let readout = app
        .i18n
        .text_with("radio-frequency", &[("frequency", arg("7.100"))]);

    {
        let harness = render(&mut app);
        harness.get_by_label(&title);
        harness.get_by_label(&connect);
        harness.get_by_label(&unknown);
        harness.get_by_label(&retry);
    }

    app.rig.enabled = true;
    app.rig_snapshot.state = RigState::Receiving;
    app.rig_snapshot.reading = Some(crate::worker::rig::Reading {
        frequency_hz: 7_100_000,
        mode: "LSB".to_owned(),
        band: Some("40m".to_owned()),
    });
    {
        let harness = render(&mut app);
        harness.get_by_label(&disconnect);
        let mode_rect = harness.get_by_label(mode).rect();
        let frequency_rect = harness.get_by_label(&readout).rect();
        let step_rect = harness.get_by_label("−").rect();
        assert_eq!(
            step_rect.height(),
            frequency_rect.bottom() - mode_rect.top()
        );
        harness.get_by_label(&retry);
    }

    // Reconnecting is offered exactly when there is a failure to recover
    // from.
    app.rig_snapshot.state = RigState::Failed;
    let failure = AppError::Rig(RigError::Connect {
        address: "127.0.0.1:4532".to_owned(),
        detail: "connection refused".to_owned(),
    });
    let reported = failure.to_string();
    app.rig_snapshot.error = Some(failure);
    let harness = render(&mut app);
    harness.get_by_label(&retry);
    harness.get_by_label(&reported);
}

#[test]
fn the_transmit_tab_keeps_mode_detection_visible_but_inactive() {
    let mut app = App::headless();
    app.tab = Tab::Transmit;
    let automatic = app.i18n.text("label-auto-vis");
    let before = app.auto_mode;

    {
        let mut harness = render(&mut app);
        harness.get_by_label(&automatic).click();
        harness.run();
    }

    assert_eq!(app.auto_mode, before);
}

/// The QSO panel reads downwards in the order a contact is worked: who is
/// being called, what they gave, and what is being given back.
#[test]
fn the_qso_panel_reads_from_the_call_down_to_the_report_being_sent() {
    let mut app = App::headless();
    let call = app.i18n.text("qso-call");
    let received = app.i18n.text("qso-rsv-received");
    let sent = app.i18n.text("qso-rsv-nr");

    let harness = render(&mut app);
    let call_top = harness.get_by_label(&call).rect().top();
    let received_top = harness.get_by_label(&received).rect().top();
    let sent_top = harness.get_by_label(&sent).rect().top();

    assert!(call_top < received_top);
    assert!(received_top < sent_top);
}

#[test]
fn the_serial_number_buttons_work_it() {
    let mut app = App::headless();
    app.contest_mode = true;
    app.qso.number = "007".to_owned();
    let increment = app.i18n.text("qso-nr-increment");
    let reset = app.i18n.text("qso-nr-reset");

    {
        let mut harness = render(&mut app);
        harness.get_by_label(&increment).click();
        harness.run();
    }
    assert_eq!(app.qso.number, "008");

    {
        let mut harness = render(&mut app);
        harness.get_by_label(&reset).click();
        harness.run();
    }
    assert_eq!(app.qso.number, FIRST_QSO_NUMBER);
}

/// The serial belongs to a contest, so nothing about it can be worked
/// until the operator has said they are in one.
#[test]
fn the_serial_number_is_inert_outside_contest_mode() {
    let mut app = App::headless();
    assert!(!app.contest_mode);
    let increment = app.i18n.text("qso-nr-increment");
    let reset = app.i18n.text("qso-nr-reset");

    let harness = render(&mut app);

    for label in [&increment, &reset] {
        assert!(
            harness.get_by_label(label).accesskit_node().is_disabled(),
            "{label} is still clickable"
        );
    }
}

/// The indicator keeps the whole row to itself, because the row is one
/// widget tall on both tabs and nothing may be added beside it that would
/// make it taller.
#[test]
fn the_receive_indicator_spans_its_row() {
    let mut app = App::headless();
    app.tab = Tab::Receive;
    // The DSP row spans the frame's full content width; nothing else in the
    // tab reports that width.
    let dsp_labels = Dsp::ALL.map(|dsp| app.i18n.text(dsp.label_key()));

    let harness = render(&mut app);
    let bar = harness
        .get_by_role(egui::accesskit::Role::ProgressIndicator)
        .rect();
    let left = harness.get_by_label(&dsp_labels[0]).rect().left();
    let right = harness
        .get_by_label(dsp_labels.last().unwrap())
        .rect()
        .right();

    assert!(
        (bar.left() - left).abs() < 1.0 && (bar.right() - right).abs() < 1.0,
        "the indicator spans {}..{} rather than the content's {left}..{right}",
        bar.left(),
        bar.right()
    );
}

/// A bar carries no sign that it can be clicked, so the hover text is the
/// whole of the invitation and has to be there to be read.
#[test]
fn hovering_the_receive_indicator_offers_the_reset() {
    let mut app = App::headless();
    app.tab = Tab::Receive;
    let hint = app.i18n.text("rx-reset-hint");

    let mut harness = render(&mut app);
    assert!(harness.query_by_label(&hint).is_none());
    harness
        .get_by_role(egui::accesskit::Role::ProgressIndicator)
        .hover();
    harness.run();

    harness.get_by_label(&hint);
}

#[test]
fn clicking_the_receive_indicator_asks_for_a_reset() {
    let mut app = App::headless();
    app.tab = Tab::Receive;
    assert_eq!(app.audio.resets(), 0);

    {
        let mut harness = render(&mut app);
        harness
            .get_by_role(egui::accesskit::Role::ProgressIndicator)
            .click();
        harness.run();
    }

    assert_eq!(app.audio.resets(), 1);
}

#[test]
fn the_tab_control_panel_keeps_its_height_between_tabs() {
    let mut tops = Vec::new();
    for tab in Tab::ALL {
        let mut app = App::headless();
        app.tab = tab;
        let radio = app.i18n.text("section-radio");
        let harness = render(&mut app);
        tops.push(harness.get_by_label(&radio).rect().top());
    }

    assert!((tops[0] - tops[1]).abs() < f32::EPSILON);
}

#[test]
fn the_transmit_button_fills_the_receive_dsp_region() {
    let receive_height = {
        let mut app = App::headless();
        app.tab = Tab::Receive;
        let dsp_heading = app.i18n.text("section-dsp");
        let dsp_labels = Dsp::ALL.map(|dsp| app.i18n.text(dsp.label_key()));
        let harness = render(&mut app);
        let top = harness.get_by_label(&dsp_heading).rect().top();
        let bottom = dsp_labels
            .iter()
            .map(|label| harness.get_by_label(label).rect().bottom())
            .fold(f32::NEG_INFINITY, f32::max);
        bottom - top
    };
    let transmit_height = {
        let mut app = App::headless();
        app.tab = Tab::Transmit;
        let transmit = app.i18n.text("action-transmit");
        let harness = render(&mut app);
        harness.get_by_label(&transmit).rect().height()
    };

    assert!(
        (receive_height - transmit_height).abs() < f32::EPSILON,
        "DSP region is {receive_height}, TX button is {transmit_height}"
    );
}

#[test]
fn status_errors_are_left_and_audio_facts_are_right() {
    let mut app = App::headless();
    app.library.error = Some("library unavailable".to_owned());
    let no_audio = app.i18n.text("status-no-audio");
    let no_output = app.i18n.text("status-no-output");
    let mut harness = Harness::builder()
        .with_size(egui::vec2(600.0, 40.0))
        .build_ui(|ui| status_bar(ui, &app));
    harness.run();

    let error = harness.get_by_label("library unavailable").rect();
    let audio = harness.get_by_label(&no_audio).rect();
    let output = harness.get_by_label(&no_output).rect();
    assert!(error.left() < audio.left());
    assert!(audio.left() > 300.0);
    assert!(output.right() > 580.0);
}

#[test]
fn the_in_window_menu_bar_renders_on_every_platform() {
    // muda is not compiled on Linux, so this renderer is the only menu
    // there. Exercising it here keeps it working on a machine that never
    // takes that path at run time.
    let model = menu::model(&App::headless());
    let labels: Vec<String> = model.iter().map(|menu| menu.label.clone()).collect();
    let mut harness = Harness::new_ui(|ui| {
        menu::bar(ui, &model);
    });
    harness.run();
    for label in &labels {
        harness.get_by_label(label);
    }
}

fn lost_device() -> rssstv_audio::StreamFault {
    rssstv_audio::StreamFault {
        device: "USB Audio CODEC".to_owned(),
        kind: FaultKind::Disconnected,
        detail: "the device is not available".to_owned(),
    }
}

/// A lost device has to name itself: "an audio device stopped" leaves the
/// operator guessing which of several is unplugged.
#[test]
fn a_lost_device_is_named_in_the_report() {
    let mut app = App::headless();
    app.device_fault = Some(lost_device());

    let harness = render(&mut app);

    harness.get_by_label_contains("USB Audio CODEC");
    harness.get_by_label("Retry");
    harness.get_by_label("Close");
}

/// Closing the report has to clear it, or it returns on the next frame and
/// the operator cannot reach the interface behind it.
#[test]
fn closing_the_report_clears_it() {
    let mut app = App::headless();
    app.device_fault = Some(lost_device());

    {
        let mut harness = render(&mut app);
        harness.get_by_label("Close").click();
        harness.run();
    }

    assert!(app.device_fault.is_none());
}

/// Nothing of the report may be drawn while no device has been lost.
#[test]
fn no_report_is_drawn_without_a_fault() {
    let mut app = App::headless();

    let harness = render(&mut app);

    assert!(harness.query_by_label("Retry").is_none());
}

/// The receive tab has to say why it has gone quiet while the station
/// transmits: it is not waiting for a signal, it has stopped listening.
#[test]
fn the_receive_tab_reports_that_reception_is_muted() {
    let mut app = App::headless();
    app.tab = Tab::Receive;
    let waiting = app.i18n.text("state-waiting");
    let muted = app.i18n.text("state-rx-muted");
    assert_eq!(state(&app), waiting);

    app.tx_snapshot.phase = TxPhase::Producing;
    app.poll_workers();

    assert_eq!(state(&app), muted);
    let harness = render(&mut app);
    harness.get_by_label(&muted);
}

/// A populated library exercises the entry rows, which an interface built
/// over empty directories never reaches.
#[test]
fn a_populated_library_renders() {
    let mut app = App::headless();
    app.library.templates = vec![crate::app::Entry::sample("field-day.kdl", "")];
    app.library.template = Some(0);
    app.library.stocks = vec![
        crate::app::Entry::sample("antenna.png", "640×496"),
        crate::app::Entry::sample("shack.png", "320×256"),
    ];
    app.library.stock = Some(1);

    // Rows carry the geometry alongside the name, so the accessible label
    // is the two of them together rather than the file name alone.
    let harness = render(&mut app);
    harness.get_by_label_contains("field-day.kdl");
    harness.get_by_label_contains("antenna.png");
}

#[test]
fn template_and_stock_tables_give_the_file_name_most_of_the_width() {
    let mut app = App::headless();
    let template = "a-very-long-template-file-name-that-needs-the-complete-column-width.kdl";
    let stock = "a-very-long-stock-file-name-that-needs-a-wide-column.png";
    app.library.templates = vec![crate::app::Entry::sample(template, "")];
    app.library.stocks = vec![crate::app::Entry::sample(stock, "320×256")];
    let mut harness = Harness::new_ui(|ui| {
        ui.allocate_ui(egui::vec2(700.0, 180.0), |ui| library(ui, &mut app));
    });
    harness.run();

    let template_width = harness.get_by_label(template).rect().width();
    let stock_width = harness.get_by_label(stock).rect().width();
    assert!(template_width > stock_width);
    assert!(
        stock_width > 240.0,
        "stock name column was {stock_width} points"
    );
}

/// The row is what senses the click, so its text must not. A selectable
/// label takes the text cursor and swallows the press, which leaves the
/// row unclickable exactly where it has text on it.
#[test]
fn clicking_a_row_label_selects_that_row() {
    let mut app = App::headless();
    app.library.stocks = vec![
        crate::app::Entry::sample("antenna.png", "640x496"),
        crate::app::Entry::sample("shack.png", "320x256"),
    ];
    app.library.stock = Some(0);

    {
        let mut harness = Harness::new_ui(|ui| {
            let model = menu::model(&app);
            view(ui, &mut app, &model, menu::is_in_window());
        });
        harness.run();
        harness.get_by_label("shack.png").click();
        harness.run();
    }

    assert_eq!(app.library.stock, Some(1));
}

/// The lists decide the image a transmission is sending, so clicking one
/// while it runs must not change the selection under it.
#[test]
fn a_transmission_locks_the_library_lists() {
    let mut app = App::headless();
    app.tx_snapshot.phase = TxPhase::Producing;
    app.library.stocks = vec![
        crate::app::Entry::sample("antenna.png", "640x496"),
        crate::app::Entry::sample("shack.png", "320x256"),
    ];
    app.library.stock = Some(0);

    {
        let mut harness = Harness::new_ui(|ui| {
            let model = menu::model(&app);
            view(ui, &mut app, &model, menu::is_in_window());
        });
        harness.run();
        harness.get_by_label("shack.png").click();
        harness.run();
    }

    assert_eq!(app.library.stock, Some(0));
}

/// Decibels are the unit the level is heard in, so the readout is what
/// makes the squared travel legible rather than arbitrary.
#[test]
fn the_transmit_level_reads_out_in_decibels() {
    assert_eq!(decibels(1.0), "0.0");
    assert_eq!(decibels(0.5), "-12.0");
    assert_eq!(decibels(0.0), "-\u{221e}");
}

/// A table asks for 200 points of scrolling area unless it is told
/// otherwise, which held the library panel open at that height however far
/// down its divider was dragged.
#[test]
fn the_library_fits_the_height_it_is_given() {
    let mut app = App::headless();
    app.library.templates = vec![crate::app::Entry::sample("field-day.kdl", "")];
    app.library.stocks = vec![crate::app::Entry::sample("antenna.png", "320x256")];
    let mut used = 0.0;

    {
        let mut harness = Harness::new_ui(|ui| {
            used = ui
                .allocate_ui(egui::vec2(560.0, 80.0), |ui| library(ui, &mut app))
                .response
                .rect
                .height();
        });
        harness.run();
    }

    assert!(used <= 80.0, "the library insisted on {used} points");
}

/// The station details are edited in front of the interface and dismissed
/// from the dialog itself, so opening it must not be a state the operator
/// is left in.
#[test]
fn the_station_dialog_closes_from_its_own_button() {
    let mut app = App::headless();
    app.station.open = true;
    let title = app.i18n.text("station-title");
    let close = app.i18n.text("station-close");

    {
        let mut harness = Harness::new_ui(|ui| {
            let model = menu::model(&app);
            view(ui, &mut app, &model, menu::is_in_window());
        });
        harness.run();
        harness.get_by_label(&title);
        harness.get_by_label(&close).click();
        harness.run();
    }

    assert!(!app.station.open);
}

#[test]
fn switching_tabs_does_not_collide_widget_ids() {
    let mut app = App::headless();
    for tab in Tab::ALL {
        app.tab = tab;
        render(&mut app);
    }
}

/// Pressing TX with nothing to send used to report the reason as an error.
/// The button now refuses the press instead, so the reason is read from the
/// state line and the button's own hover text.
#[test]
fn the_tx_button_refuses_a_transmission_it_cannot_start() {
    let mut app = App::headless();
    app.tab = Tab::Transmit;
    assert!(!app.can_transmit());

    {
        let mut harness = Harness::new_ui(|ui| {
            let model = menu::model(&app);
            view(ui, &mut app, &model, menu::is_in_window());
        });
        harness.run();
        harness.get_by_label("TX").click();
        harness.run();
    }

    assert_eq!(app.tx_error, None);
    assert_eq!(app.tx_snapshot.phase, TxPhase::Idle);
}

/// The tone is captioned with its frequency and takes a quarter of the row,
/// so the trigger beside it stays the button the operator aims for.
#[test]
fn the_tune_tone_takes_a_quarter_of_the_transmit_row() {
    let mut app = App::headless();
    app.tab = Tab::Transmit;
    let transmit = app.i18n.text("action-transmit");
    let caption = TUNE_FREQUENCY_HZ.to_string();

    let harness = render(&mut app);
    let trigger = harness.get_by_label(&transmit).rect();
    let tone = harness.get_by_label(&caption).rect();

    assert!(tone.left() >= trigger.right());
    assert!((tone.top() - trigger.top()).abs() < f32::EPSILON);
    assert!((tone.height() - trigger.height()).abs() < f32::EPSILON);
    assert!(
        (trigger.width() - tone.width() * 3.0).abs() < 0.5,
        "TX is {}, tone is {}",
        trigger.width(),
        tone.width()
    );
}

/// The two ways of keying the rig cannot both be taken up, so whichever one
/// is running refuses the other rather than cutting it off.
#[test]
fn the_transmit_trigger_is_refused_while_a_tone_is_being_sent() {
    let mut app = App::headless();
    app.tab = Tab::Transmit;
    app.tune_for_test();
    let caption = TUNE_FREQUENCY_HZ.to_string();

    {
        let mut harness = render(&mut app);
        harness.get_by_label("TX").click();
        harness.run();
    }

    assert!(app.is_tuning());
    assert_eq!(app.tx_error, None);
    assert_eq!(
        app.transmit_problem(),
        Some(app.i18n.text("error-tone-active"))
    );

    // The tone's own button is what gives the rig back.
    {
        let mut harness = render(&mut app);
        harness.get_by_label(&caption).click();
        harness.run();
    }

    assert!(!app.is_tuning());
}

/// A panel keeps the width it was last laid out at, so a window dragged
/// narrow enough to squeeze this one used to leave it squeezed once the
/// window was given its size back.
#[test]
fn the_side_panel_returns_to_its_width_after_a_narrow_window() {
    let mut app = App::headless();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 700.0))
        .build_ui(|ui| {
            let model = menu::model(&app);
            view(ui, &mut app, &model, menu::is_in_window());
        });

    harness.run();
    assert_eq!(side_panel_width(&harness), Some(SIDE_PANEL_WIDTH));

    // Narrower than the panel asks for, which is the one case it gives way in:
    // it cannot be wider than the window it is in.
    harness.set_size(egui::vec2(200.0, 700.0));
    harness.run();
    assert!(
        side_panel_width(&harness).is_some_and(|width| width < SIDE_PANEL_WIDTH),
        "the panel should give way to a window narrower than it is"
    );

    harness.set_size(egui::vec2(900.0, 700.0));
    harness.run();
    assert_eq!(side_panel_width(&harness), Some(SIDE_PANEL_WIDTH));
}

fn side_panel_width(harness: &Harness<'_>) -> Option<f32> {
    egui::containers::panel::PanelState::load(&harness.ctx, Id::new("side-panel"))
        .map(|state| state.outer_rect.width())
}
