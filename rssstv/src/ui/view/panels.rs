//! The sections of the side panel, and the controls each one holds.

use super::*;

pub(super) fn side_panel(ui: &mut Ui, app: &mut App) {
    // The sections below can outgrow the panel's height on a small window or
    // a large font scale; scroll rather than silently clipping the bottom.
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        tab_selector(ui, app);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(TAB_CONTROLS_HEIGHT);
            tab_controls(ui, app);
        });
        ui.add_space(16.0);
        let radio_title = app.i18n.text("section-radio");
        section(ui, &radio_title, |ui| radio_panel(ui, app));
        ui.add_space(16.0);
        let qso_title = app.i18n.text("section-qso");
        section(ui, &qso_title, |ui| qso_panel(ui, app));
    });
}

/// The connection, where the rig is, and the two ways the interface moves it.
///
/// Always present, because this is where rig control is switched on: a station
/// that has not connected still needs somewhere to say so, and a panel that
/// came and went with the connection would be somewhere it could not.
pub(super) fn radio_panel(ui: &mut Ui, app: &mut App) {
    connection_row(ui, app);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    tuning_row(ui, app);
}

/// The switch, and what the connection has to say for itself.
pub(super) fn connection_row(ui: &mut Ui, app: &mut App) {
    let failed = app.rig_snapshot.state == RigState::Failed;
    let label = app.i18n.text(if app.rig.enabled {
        "action-rig-disconnect"
    } else {
        "action-rig-connect"
    });
    let retry = app.i18n.text("action-rig-retry");
    let mut toggled = false;
    let mut retried = false;
    ui.horizontal(|ui| {
        let height = ui.spacing().interact_size.y;
        let gap = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - gap) / 2.0;
        let switch = egui::Button::new(RichText::new(label).size(SMALL)).selected(app.rig.enabled);
        toggled = ui.add_sized([width, height], switch).clicked();
        let again = egui::Button::new(RichText::new(retry).size(SMALL));
        retried = ui
            .add_enabled_ui(failed, |ui| ui.add_sized([width, height], again))
            .inner
            .clicked();
    });
    let state = app.i18n.text(app.rig_snapshot.state.label_key());
    ui.label(RichText::new(state).size(LABEL).weak());
    if toggled {
        app.set_rig_enabled(!app.rig.enabled);
    }
    if retried {
        app.retry_rig();
    }
}

pub(super) fn tuning_row(ui: &mut Ui, app: &mut App) {
    let tunable = app.can_tune();
    let selected = app
        .rig_snapshot
        .reading
        .as_ref()
        .and_then(|reading| reading.band.clone());
    let unknown = app.i18n.text("radio-band-unknown");
    let mode = app
        .rig_snapshot
        .reading
        .as_ref()
        .map(|reading| reading.mode.clone())
        .unwrap_or_else(|| app.i18n.text("rig-mode-unknown"));
    let readout = frequency_readout(app);
    let (down, up) = (app.stepped_frequency(-1), app.stepped_frequency(1));
    let mut chosen = None;
    let mut stepped = 0;
    ui.add_enabled_ui(tunable, |ui| {
        ComboBox::from_id_salt("band")
            .selected_text(RichText::new(selected.clone().unwrap_or(unknown)).size(SMALL))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for band in app.bands.bands() {
                    let picked = selected.as_deref() == Some(band.name.as_str());
                    if ui.selectable_label(picked, &band.name).clicked() {
                        chosen = Some(band.name.clone());
                    }
                }
            });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let line_height =
                ui.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(SMALL)));
            let height = (line_height * 2.0 + ui.spacing().item_spacing.y).ceil();
            let step = ui.spacing().interact_size.y * 1.6;
            let gaps = ui.spacing().item_spacing.x * 2.0;
            let width = (ui.available_width() - gaps - step * 2.0).max(0.0);
            // Disabled at the band edges rather than clamped: a button that
            // moves nothing is better than one that says it moved something.
            let back = egui::Button::new("−").min_size([step, height].into());
            if ui.add_enabled(down.is_some(), back).clicked() {
                stepped = -1;
            }
            ui.allocate_ui_with_layout(
                egui::vec2(width, height),
                Layout::top_down(Align::Center),
                |ui| {
                    ui.label(RichText::new(mode).size(SMALL));
                    ui.label(RichText::new(readout).size(SMALL));
                },
            );
            let forward = egui::Button::new("+").min_size([step, height].into());
            if ui.add_enabled(up.is_some(), forward).clicked() {
                stepped = 1;
            }
        });
    });
    if let Some(name) = chosen {
        app.change_band(&name);
    }
    if stepped != 0 {
        app.step_frequency(stepped);
    }
}

pub(super) fn frequency_readout(app: &App) -> String {
    let Some(reading) = app.rig_snapshot.reading.as_ref() else {
        return app.i18n.text("rig-frequency-unknown");
    };
    // Formatted here rather than by the message, because the message would
    // group the digits by locale and a frequency is read as one number.
    let megahertz = format!("{:.3}", reading.frequency_hz as f64 / 1_000_000.0);
    app.i18n
        .text_with("radio-frequency", &[("frequency", arg(&megahertz))])
}

/// The controls for whichever half of the application is in front.
///
/// The two tabs share a shape: a bar across the top, the mode below it, and
/// the controls that act on the signal at the bottom. What each one means
/// differs, so the bar says whether a picture is arriving while receiving and
/// sets the output level while transmitting, and the DSP toggles give way to
/// the transmit trigger.
pub(super) fn tab_controls(ui: &mut Ui, app: &mut App) {
    ui.set_width(ui.available_width());
    heading(
        ui,
        &app.i18n.text(match app.tab {
            Tab::Receive => "section-rx-state",
            Tab::Transmit => "section-tx-level",
        }),
    );
    match app.tab {
        Tab::Receive => rx_state(ui, app),
        Tab::Transmit => tx_level(ui, app),
    }
    ui.add_space(12.0);
    heading(ui, &app.i18n.text("section-mode"));
    mode_panel(ui, app);
    ui.add_space(12.0);
    let spacing = ui.spacing().item_spacing.y + ui.spacing().interact_size.y;
    let signal_controls_height =
        (ui.fonts_mut(|fonts| fonts.row_height(&egui::FontId::proportional(LABEL))) + spacing)
            .ceil();
    match app.tab {
        Tab::Receive => {
            heading(ui, &app.i18n.text("section-dsp"));
            dsp_panel(ui, app);
        }
        Tab::Transmit => transmit_controls(ui, app, signal_controls_height),
    }
}

pub(super) fn section(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    heading(ui, title);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        contents(ui);
    });
}

/// Whether a picture is arriving, as the bar the transmit level is set on.
///
/// It used to show the input amplitude, which is not what the operator is
/// looking for here and could only be redrawn as often as a block of audio
/// arrives, so it stepped rather than moved. What the bar was read for is
/// whether the signal is being decoded at all, and that is all it says now.
pub(super) fn rx_state(ui: &mut Ui, app: &App) {
    let active = app.audio.snapshot().progress.is_active();
    let filled = if active { 1.0 } else { 0.0 };
    ui.add(ProgressBar::new(filled).fill(colors::RX_ACTIVE));
}

/// The transmit level, set by dragging the bar the receive indicator fills.
///
/// Drawn as that bar rather than as a slider so the panel keeps one shape
/// across the tabs, and colored while transmitting for the same reason the
/// receive indicator is: the bar says whether the radio is doing anything.
pub(super) fn tx_level(ui: &mut Ui, app: &mut App) {
    let transmitting = app.tx_snapshot.phase.is_active();
    let color = if transmitting {
        colors::TX_LEVEL
    } else {
        Color32::WHITE
    };
    let bar = ui.add(ProgressBar::new(app.tx_volume).fill(color));
    let dragged = bar.interact(egui::Sense::click_and_drag());
    let rect = dragged.rect;
    if let Some(pointer) = dragged.interact_pointer_pos() {
        app.tx_volume = ((pointer.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
    }
    dragged.on_hover_text(app.i18n.text_with(
        "tx-volume",
        &[
            ("percent", number((app.tx_volume * 100.0).round())),
            ("decibels", arg(&decibels(app.tx_volume))),
        ],
    ));
}

/// The level as an amplitude ratio in decibels, which is the unit the setting
/// is actually perceived in.
pub(super) fn decibels(travel: f32) -> String {
    let amplitude = TxGain::amplitude(travel);
    if amplitude <= 0.0 {
        return "-\u{221e}".to_owned();
    }
    format!("{:.1}", 20.0 * amplitude.log10())
}

pub(super) fn mode_panel(ui: &mut Ui, app: &mut App) {
    let label = app.i18n.text("label-auto-vis");
    ui.add_enabled_ui(app.tab == Tab::Receive, |ui| {
        ui.checkbox(&mut app.auto_mode, RichText::new(label).size(SMALL));
    });
    let (selected, options) = match app.tab {
        Tab::Transmit => (app.tx_mode, app.tx_modes.clone()),
        Tab::Receive => (app.rx_mode, app.rx_modes.clone()),
    };
    let mut chosen = selected;
    ComboBox::from_id_salt("mode")
        .selected_text(selected.spec().name())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for mode in options {
                ui.selectable_value(&mut chosen, mode, mode.spec().name());
            }
        });
    if chosen != selected {
        match app.tab {
            Tab::Transmit => app.select_tx_mode(chosen),
            Tab::Receive => app.select_rx_mode(chosen),
        }
    }
}

pub(super) fn dsp_panel(ui: &mut Ui, app: &mut App) {
    let mut toggled = None;
    ui.horizontal(|ui| {
        let gaps = ui.spacing().item_spacing.x * (Dsp::ALL.len() - 1) as f32;
        // Divided rather than given a minimum: three buttons that each insist
        // on a readable width overflow the panel once it is dragged narrow.
        let width = (ui.available_width() - gaps) / Dsp::ALL.len() as f32;
        let height = ui.spacing().interact_size.y;
        for dsp in Dsp::ALL {
            let label = RichText::new(app.i18n.text(dsp.label_key())).size(SMALL);
            let button = egui::Button::new(label).selected(app.dsp.get(dsp));
            // Sized rather than given a minimum, because a button wider than
            // its label leaves the text against its left edge otherwise.
            if ui.add_sized([width, height], button).clicked() {
                toggled = Some(dsp);
            }
        }
    });
    if let Some(dsp) = toggled {
        app.toggle_dsp(dsp);
    }
}

pub(super) fn qso_panel(ui: &mut Ui, app: &mut App) {
    // Text fields default to a width far wider than this panel, so every one
    // of them is sized from the space actually available.
    let gap = ui.spacing().item_spacing.x;
    let full = ui.available_width();
    let fields = full - FIELD_LABEL_WIDTH - gap;
    let call_label = app.i18n.text("qso-call");
    let call_hint = app.i18n.text("hint-callsign");
    let received_label = app.i18n.text("qso-rsv-received");
    let sent_label = app.i18n.text("qso-rsv-nr");

    // A field is taken up as typed while it is being typed, and trimmed once
    // the operator has left it.
    let mut finished = false;
    ui.horizontal(|ui| {
        field_label(ui, &call_label);
        let edit = egui::TextEdit::singleline(&mut app.qso.call)
            .desired_width(fields)
            .hint_text(call_hint.as_str());
        let response = ui.add(edit);
        finished |= response.lost_focus();
        if response.changed() {
            app.normalize_call();
            app.qso_changed();
        }
    });
    // The report the other station gave is one field: the number in it arrives
    // over the air as one thing, and it is read rather than composed.
    ui.horizontal(|ui| {
        field_label(ui, &received_label);
        let edit = egui::TextEdit::singleline(&mut app.qso.rsv_received).desired_width(fields);
        let response = ui.add(edit);
        finished |= response.lost_focus();
        if response.changed() {
            app.qso_changed();
        }
    });
    // The serial number belongs to a contest, so it is worked only while the
    // operator has said they are in one. The report beside it is not: every
    // contact gets one.
    let contest = app.contest_mode;
    ui.horizontal(|ui| {
        // The report being sent is two fields rather than one: the report
        // itself is set once and then left alone, while the serial number
        // moves with every contact.
        field_label(ui, &sent_label);
        let field_width = (fields - gap) / 2.0;
        let report =
            ui.add(egui::TextEdit::singleline(&mut app.qso.rsv).desired_width(field_width));
        let serial = ui
            .add_enabled_ui(contest, |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.qso.number).desired_width(field_width))
            })
            .inner;
        finished |= report.lost_focus() || serial.lost_focus();
        if report.changed() || serial.changed() {
            app.qso_changed();
        }
    });
    ui.horizontal(|ui| {
        let width = (full - gap) / 2.0;
        let height = ui.spacing().interact_size.y;
        let increment = RichText::new(app.i18n.text("qso-nr-increment")).size(SMALL);
        let reset = RichText::new(app.i18n.text("qso-nr-reset")).size(SMALL);
        ui.add_enabled_ui(contest, |ui| {
            if ui
                .add_sized([width, height], egui::Button::new(increment))
                .clicked()
            {
                app.increment_number();
            }
            if ui
                .add_sized([width, height], egui::Button::new(reset))
                .clicked()
            {
                app.reset_number();
            }
        });
    });
    if finished {
        app.finish_qso_edit();
    }
}
