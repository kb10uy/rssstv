use egui::{Align, Color32, ComboBox, Id, Layout, Panel, ProgressBar, RichText, Ui};
use egui_extras::{Column, TableBuilder};

use rssstv_audio::FaultKind;
use rssstv_template::valid_variable_name;

use crate::{
    app::{App, Dsp, Entry, Tab},
    error::AppError,
    i18n::{arg, number},
    storage::paths::Folder,
    ui::{canvas, colors, menu},
    worker::{
        receive::RxProgress,
        rig::RigState,
        transmit::{TxGain, TxPhase, TxProgress},
    },
};

const SIDE_PANEL_WIDTH: f32 = 224.0;
const TAB_CONTROLS_HEIGHT: f32 = 160.0;
/// Default height of the library row; the operator can drag it.
const LIBRARY_HEIGHT: f32 = 182.0;
/// Narrowest a library list is laid out at before the panel scrolls.
const LIST_WIDTH: f32 = 182.0;
const FIELD_LABEL_WIDTH: f32 = 72.0;

const SMALL: f32 = 12.0;
const LABEL: f32 = 11.0;

/// Draws the whole interface, returning the menu action the operator chose.
pub fn view(ui: &mut Ui, app: &mut App, model: &[menu::Menu]) -> Option<menu::Action> {
    // Labels are inert throughout. Several of them sit inside rows that sense
    // the click themselves, and a selectable label takes the text cursor and
    // swallows the press; none of this text is worth dragging a selection
    // across either. Set here rather than at startup so the interface behaves
    // the same under test.
    ui.style_mut().interaction.selectable_labels = false;

    let mut action = None;
    if menu::is_in_window() {
        Panel::top(Id::new("menu-bar")).show(ui, |ui| {
            action = menu::bar(ui, model);
        });
    }
    Panel::bottom(Id::new("status-bar")).show(ui, |ui| status_bar(ui, app));
    // The side panel is claimed before the library so it runs the full height
    // above the status bar: its sections are read while working in either half
    // of the window, and the library only needs the width the lists are laid
    // out in.
    Panel::right(Id::new("side-panel"))
        .resizable(true)
        .default_size(SIDE_PANEL_WIDTH)
        .size_range(182.0..=560.0)
        .show(ui, |ui| side_panel(ui, app));
    Panel::bottom(Id::new("library"))
        .resizable(true)
        .default_size(LIBRARY_HEIGHT)
        .size_range(112.0..=640.0)
        .show(ui, |ui| library(ui, app));
    egui::CentralPanel::default().show(ui, |ui| main_pane(ui, app));
    station_dialog(ui, app);
    custom_variable_dialog(ui, app);
    device_fault_modal(ui, app);
    action
}

/// Edits what the station says about itself.
///
/// Kept out of the QSO panel and behind the Settings menu because none of it
/// belongs to the contact being worked: it is set once for the operator and
/// then left alone, while the panel beside the image is for the station on the
/// air right now.
fn station_dialog(ui: &mut Ui, app: &mut App) {
    if !app.station_open {
        return;
    }

    let title = app.i18n.text("station-title");
    let note = app.i18n.text("station-callsign-required");
    let close = app.i18n.text("station-close");
    let labels = ["station-callsign", "station-qth", "station-grid"].map(|key| app.i18n.text(key));

    let mut finished = false;
    let mut done = false;
    let response = egui::Modal::new(Id::new("station")).show(ui.ctx(), |ui| {
        ui.set_max_width(360.0);
        ui.heading(title);
        ui.add_space(8.0);
        let width = ui.available_width() - FIELD_LABEL_WIDTH - ui.spacing().item_spacing.x;
        finished = station_field(ui, &labels[0], &mut app.station_callsign, width);
        finished |= station_field(ui, &labels[1], &mut app.station_qth, width);
        finished |= station_field(ui, &labels[2], &mut app.station_grid, width);
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(LABEL).weak());
        ui.add_space(16.0);
        done = ui.button(close).clicked();
    });

    // Taken up once the field is left rather than on every keystroke: half a
    // callsign is not one, and uppercasing the text under the cursor while it
    // is still being typed fights the operator. Closing counts as leaving,
    // for a dialog dismissed without moving focus first.
    let closing = done || response.should_close();
    if finished || closing {
        // Normalizing composes again on its own, and does it with the
        // uppercased callsign rather than with what was typed.
        app.normalize_station_callsign();
    }
    if closing {
        app.station_open = false;
    }
}

/// Edits the variables the operator invented for their own templates.
///
/// Everything else a template can read is something the application already
/// knows; these are the ones only the operator does, so this is the one place
/// where both the name and the value are typed.
fn custom_variable_dialog(ui: &mut Ui, app: &mut App) {
    if !app.custom_open {
        return;
    }

    let title = app.i18n.text("custom-title");
    let note = app.i18n.text("custom-note");
    let invalid = app.i18n.text("custom-invalid");
    let add = app.i18n.text("custom-add");
    let close = app.i18n.text("station-close");

    let mut changed = false;
    let mut removed = None;
    let mut done = false;
    let response = egui::Modal::new(Id::new("custom-variables")).show(ui.ctx(), |ui| {
        ui.set_max_width(420.0);
        ui.heading(title);
        ui.add_space(8.0);
        let remove_width = ui.spacing().interact_size.y;
        let gaps = ui.spacing().item_spacing.x * 2.0;
        let name_width = (ui.available_width() - remove_width - gaps) * 0.4;
        let value_width = ui.available_width() - remove_width - gaps - name_width;
        for (index, (name, value)) in app.custom_draft.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let usable = valid_variable_name(name);
                let field = egui::TextEdit::singleline(name)
                    .desired_width(name_width)
                    .text_color_opt((!usable).then_some(colors::INVALID));
                let mut response = ui.add(field);
                if !usable {
                    response = response.on_hover_text(invalid.clone());
                }
                // A name is taken up once the field is left rather than on
                // every keystroke: half a name is a different variable, and
                // composing against each one in turn is work for nothing.
                changed |= response.lost_focus();
                changed |= ui
                    .add(egui::TextEdit::singleline(value).desired_width(value_width))
                    .changed();
                if ui.button("×").clicked() {
                    removed = Some(index);
                }
            });
        }
        ui.add_space(4.0);
        if ui.button(add).clicked() {
            app.add_custom_variable();
        }
        ui.add_space(4.0);
        ui.label(RichText::new(note).size(LABEL).weak());
        ui.add_space(16.0);
        done = ui.button(close).clicked();
    });

    if let Some(index) = removed {
        app.custom_draft.remove(index);
        changed = true;
    }
    let closing = done || response.should_close();
    if changed || closing {
        app.commit_custom_variables();
    }
    if closing {
        app.custom_open = false;
    }
}

/// One labelled field of the station dialog.
///
/// Returns whether the operator finished with it, which is losing focus to
/// another field or to the button, or pressing Enter in it.
fn station_field(ui: &mut Ui, label: &str, text: &mut String, width: f32) -> bool {
    ui.horizontal(|ui| {
        field_label(ui, label);
        ui.add(egui::TextEdit::singleline(text).desired_width(width))
            .lost_focus()
    })
    .inner
}

/// Reports a device that stopped, and offers to open it again.
///
/// Shown as a modal because losing the device stops reception outright: the
/// interface behind it is describing a session that is no longer running, and
/// the operator has to act before any of it means anything again.
fn device_fault_modal(ui: &mut Ui, app: &mut App) {
    let Some(fault) = app.device_fault.clone() else {
        return;
    };

    let reason = match fault.kind {
        FaultKind::Disconnected => app.i18n.text_with(
            "device-lost-disconnected",
            &[("device", arg(&fault.device))],
        ),
        FaultKind::Invalidated => app
            .i18n
            .text_with("device-lost-invalidated", &[("device", arg(&fault.device))]),
        FaultKind::Backend => app.i18n.text_with(
            "device-lost-backend",
            &[
                ("device", arg(&fault.device)),
                ("detail", arg(&fault.detail)),
            ],
        ),
    };

    let mut retry = false;
    let mut dismiss = false;
    let response = egui::Modal::new(Id::new("device-fault")).show(ui.ctx(), |ui| {
        ui.set_max_width(420.0);
        ui.heading(app.i18n.text("device-lost-title"));
        ui.add_space(8.0);
        ui.label(reason);
        ui.add_space(8.0);
        ui.label(app.i18n.text("device-lost-reception-stopped"));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            retry = ui
                .button(app.i18n.text("device-lost-retry"))
                .on_hover_text(fault.detail.clone())
                .clicked();
            dismiss = ui.button(app.i18n.text("device-lost-dismiss")).clicked();
        });
    });

    // A click outside the modal is the same acknowledgement as the button:
    // the report has been read, and the interface should not trap the
    // operator in it.
    if retry {
        app.retry_device();
    } else if dismiss || response.should_close() {
        app.dismiss_device_fault();
    }
}

/// Chooses which half of the application the panel below is showing.
///
/// It sits on top of that box in place of a title, because the tab it selects
/// is what the box would have been titled after.
fn tab_selector(ui: &mut Ui, app: &mut App) {
    ui.horizontal(|ui| {
        let gaps = ui.spacing().item_spacing.x * (Tab::ALL.len() - 1) as f32;
        let width = (ui.available_width() - gaps) / Tab::ALL.len() as f32;
        let height = ui.spacing().interact_size.y;
        for tab in Tab::ALL {
            let label = RichText::new(app.i18n.text(tab.label_key())).size(SMALL);
            let entry = egui::Button::selectable(app.tab == tab, label);
            if ui.add_sized([width, height], entry).clicked() {
                app.tab = tab;
            }
        }
    });
}

fn main_pane(ui: &mut Ui, app: &mut App) {
    let state = state(app);
    let geometry = geometry_label(app);
    let fraction = app.decoded_fraction();

    // The bar is claimed first so it takes the height its text needs and the
    // canvas fills exactly what is left. Reserving a fixed height for it
    // instead left whatever it did not use as a gap under the image.
    Panel::bottom(Id::new("action-bar")).show(ui, |ui| action_bar(ui, &geometry, &state));
    canvas::image_view(ui, app.active_raster_mut(), fraction);
}

/// What the tab in front is doing, for the line under the image.
///
/// It names no mode, because the line it goes on already does: this is the
/// state alone.
fn state(app: &App) -> String {
    match app.tab {
        Tab::Transmit => match app.tx_snapshot.phase {
            TxPhase::Priming => app.i18n.text("state-transmit-preparing"),
            TxPhase::Producing | TxPhase::Draining => match app.tx_progress() {
                TxProgress::Scanning { rows, total } => app.i18n.text_with(
                    "state-transmitting",
                    &[
                        ("row", number(rows as u32)),
                        ("total", number(total as u32)),
                    ],
                ),
                TxProgress::Identifying => app.i18n.text("state-transmit-identifying"),
                _ => app.i18n.text("state-transmit-leader"),
            },
            TxPhase::Complete => app.i18n.text("state-transmit-complete"),
            _ if app.can_transmit() => app.i18n.text("state-transmit-ready"),
            _ => app.i18n.text("state-transmit-not-ready"),
        },
        Tab::Receive => {
            let progress = app.audio.snapshot().progress;
            // A stopped reception leaves a partial image on the canvas, so it
            // has to read differently from having nothing at all.
            if progress == RxProgress::Stopped {
                return app.i18n.text("state-stopped");
            }
            if !progress.is_active() && progress != RxProgress::Complete {
                return app.i18n.text("state-waiting");
            }
            if progress == RxProgress::Complete {
                app.i18n.text("state-complete")
            } else {
                let percent = (progress.fraction() * 100.0).round();
                app.i18n
                    .text_with("state-receiving", &[("percent", number(percent))])
            }
        }
    }
}

fn geometry_label(app: &App) -> String {
    let size = match app.tab {
        Tab::Transmit => app.tx_raster.size(),
        Tab::Receive => app.rx_raster.size(),
    };
    app.i18n.text_with(
        "geometry",
        &[
            ("mode", arg(app.active_mode().spec().name())),
            ("width", number(size.width() as u32)),
            ("height", number(size.height() as u32)),
        ],
    )
}

/// The line under the image: what is being shown, and what is happening to it.
///
/// The state used to be painted over the picture itself, which put text on top
/// of the one thing on the tab worth looking at.
fn action_bar(ui: &mut Ui, geometry: &str, state: &str) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(state).size(SMALL));
            ui.label(RichText::new(geometry).size(SMALL).weak());
        });
    });
}

/// Starts or stops a transmission.
///
/// It sits where the receive tab keeps its DSP toggles, so the control that
/// acts on the picture is in the panel beside it rather than under it.
fn transmit_button(ui: &mut Ui, app: &mut App, height: f32) {
    let active = app.tx_snapshot.phase.is_active();
    let label = app.i18n.text(if active {
        "action-stop-transmit"
    } else {
        "action-transmit"
    });
    let size = egui::vec2(ui.available_width(), height);
    // Stopping stays available for as long as something is being sent.
    // Starting does not: with anything missing the button is disabled and says
    // what, rather than taking the press and reporting the same thing as an
    // error afterwards.
    let problem = (!active).then(|| app.transmit_problem()).flatten();
    let button = egui::Button::new(RichText::new(label).size(SMALL)).fill(colors::TX_BUTTON);
    let mut response = ui
        .add_enabled_ui(problem.is_none(), |ui| ui.add_sized(size, button))
        .inner;
    if let Some(problem) = problem {
        response = response.on_disabled_hover_text(problem);
    }
    if response.clicked() {
        if active {
            app.stop_transmit();
        } else {
            app.start_transmit();
        }
    }
}

fn side_panel(ui: &mut Ui, app: &mut App) {
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
fn radio_panel(ui: &mut Ui, app: &mut App) {
    connection_row(ui, app);
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    tuning_row(ui, app);
}

/// The switch, and what the connection has to say for itself.
fn connection_row(ui: &mut Ui, app: &mut App) {
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

fn tuning_row(ui: &mut Ui, app: &mut App) {
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

fn frequency_readout(app: &App) -> String {
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
/// The two tabs share a shape: a level across the top, the mode below it, and
/// the controls that act on the signal at the bottom. What each one means
/// differs, so the level is an input meter while receiving and the output level
/// while transmitting, and the DSP toggles give way to the transmit trigger.
fn tab_controls(ui: &mut Ui, app: &mut App) {
    ui.set_width(ui.available_width());
    heading(
        ui,
        &app.i18n.text(match app.tab {
            Tab::Receive => "section-rx-level",
            Tab::Transmit => "section-tx-level",
        }),
    );
    match app.tab {
        Tab::Receive => rx_level(ui, app),
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
        Tab::Transmit => transmit_button(ui, app, signal_controls_height),
    }
}

fn heading(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).size(LABEL).weak());
}

fn section(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    heading(ui, title);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        contents(ui);
    });
}

fn rx_level(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let color = if snapshot.progress.is_active() {
        colors::RX_LEVEL
    } else {
        Color32::WHITE
    };
    ui.add(ProgressBar::new(snapshot.level).fill(color));
}

/// The transmit level, set by dragging the bar the receive meter fills.
///
/// Drawn as that meter rather than as a slider so the panel keeps one shape
/// across the tabs, and colored while transmitting for the same reason the
/// receive meter is: the bar says whether the radio is doing anything.
fn tx_level(ui: &mut Ui, app: &mut App) {
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
fn decibels(travel: f32) -> String {
    let amplitude = TxGain::amplitude(travel);
    if amplitude <= 0.0 {
        return "-\u{221e}".to_owned();
    }
    format!("{:.1}", 20.0 * amplitude.log10())
}

fn mode_panel(ui: &mut Ui, app: &mut App) {
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

fn dsp_panel(ui: &mut Ui, app: &mut App) {
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

fn qso_panel(ui: &mut Ui, app: &mut App) {
    // Text fields default to a width far wider than this panel, so every one
    // of them is sized from the space actually available.
    let gap = ui.spacing().item_spacing.x;
    let full = ui.available_width();
    let fields = full - FIELD_LABEL_WIDTH - gap;
    let call_label = app.i18n.text("qso-call");
    let received_label = app.i18n.text("qso-rsv-received");
    let sent_label = app.i18n.text("qso-rsv-nr");

    ui.horizontal(|ui| {
        field_label(ui, &call_label);
        let edit = egui::TextEdit::singleline(&mut app.qso.call).desired_width(fields);
        if ui.add(edit).changed() {
            app.normalize_call();
            app.qso_changed();
        }
    });
    // The report the other station gave is one field: the number in it arrives
    // over the air as one thing, and it is read rather than composed.
    ui.horizontal(|ui| {
        field_label(ui, &received_label);
        let edit = egui::TextEdit::singleline(&mut app.qso.rsv_received).desired_width(fields);
        if ui.add(edit).changed() {
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
        let mut changed = ui
            .add(egui::TextEdit::singleline(&mut app.qso.rsv).desired_width(field_width))
            .changed();
        changed |= ui
            .add_enabled_ui(contest, |ui| {
                ui.add(egui::TextEdit::singleline(&mut app.qso.number).desired_width(field_width))
                    .changed()
            })
            .inner;
        if changed {
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
}

/// A field label, aligned with the middle of the field beside it.
///
/// Sizing it to the row height rather than to nothing keeps it from settling
/// against the bottom of the row.
fn field_label(ui: &mut Ui, label: &str) {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(FIELD_LABEL_WIDTH, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(FIELD_LABEL_WIDTH, height));
            ui.label(RichText::new(label).size(SMALL));
        },
    );
}

/// The template and stock lists, which together decide the transmit image.
///
/// A transmission is sending the image these lists produced, so they are
/// disabled while one is running rather than letting the operator choose
/// something that could not take effect until it ends.
fn library(ui: &mut Ui, app: &mut App) {
    ui.add_space(2.0);
    let transmitting = app.tx_snapshot.phase.is_active();
    ui.add_enabled_ui(!transmitting, |ui| {
        ui.horizontal_top(|ui| {
            let height = ui.available_height();
            // The two lists divide the panel between them, so widening the
            // window widens the file names rather than the empty space beside
            // them.
            let size = egui::vec2(list_width(ui), height);

            let labels = ListLabels::new(app, "section-templates");
            let previous_template = app.template;
            match entry_list(ui, &labels, size, &app.templates, &mut app.template) {
                Some(ListAction::Reveal) => app.reveal(Folder::Templates),
                Some(ListAction::Refresh) => app.refresh_templates(),
                None => {}
            }
            if app.template != previous_template {
                app.composition_changed();
            }

            let labels = ListLabels::new(app, "section-stocks");
            let previous_stock = app.stock;
            match entry_list(ui, &labels, size, &app.stocks, &mut app.stock) {
                Some(ListAction::Reveal) => app.reveal(Folder::Stocks),
                Some(ListAction::Refresh) => app.refresh_stocks(),
                None => {}
            }
            if app.stock != previous_stock {
                app.composition_changed();
            }
        });
    });
}

/// Half of the panel, or the minimum width a list is readable at.
fn list_width(ui: &Ui) -> f32 {
    let available = ui.available_width() - ui.spacing().item_spacing.x;
    (available / 2.0).max(LIST_WIDTH)
}

enum ListAction {
    Reveal,
    Refresh,
}

/// The strings one library list needs, resolved before the list borrows state.
struct ListLabels {
    title: String,
    empty: String,
    reveal: String,
    refresh: String,
}

impl ListLabels {
    fn new(app: &App, title_key: &str) -> Self {
        Self {
            title: app.i18n.text(title_key),
            empty: app.i18n.text("library-empty"),
            reveal: app.i18n.text("action-open-folder"),
            refresh: app.i18n.text("action-refresh"),
        }
    }
}

fn entry_list(
    ui: &mut Ui,
    labels: &ListLabels,
    size: egui::Vec2,
    entries: &[Entry],
    selected: &mut Option<usize>,
) -> Option<ListAction> {
    let mut action = None;
    // The enclosing layout runs left to right, so the header and the list are
    // wrapped in a vertical Ui; without it they end up side by side.
    ui.allocate_ui(size, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                heading(ui, &labels.title);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("↻").on_hover_text(&labels.refresh).clicked() {
                        action = Some(ListAction::Refresh);
                    }
                    if ui.button("📂").on_hover_text(&labels.reveal).clicked() {
                        action = Some(ListAction::Reveal);
                    }
                });
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                if entries.is_empty() {
                    ui.label(RichText::new(&labels.empty).size(LABEL).weak());
                    return;
                }
                entry_table(ui, labels, entries, selected);
            });
        });
    });
    action
}

/// The files in one library list.
///
/// A table rather than a column of buttons: the name and the geometry are
/// real columns, so the geometry lines up down the list instead of merely
/// sitting at the end of each row, and only the visible rows are built.
fn entry_table(ui: &mut Ui, labels: &ListLabels, entries: &[Entry], selected: &mut Option<usize>) {
    let row_height = ui.spacing().interact_size.y;
    TableBuilder::new(ui)
        .id_salt(&labels.title)
        // A table insists on 200 points of scrolling area by default, which
        // would hold the whole library panel open at that height however far
        // down its divider is dragged.
        .min_scrolled_height(0.0)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .auto_shrink([false, false])
        .column(Column::remainder().clip(true))
        .column(Column::auto())
        .body(|body| {
            body.rows(row_height, entries.len(), |mut row| {
                let index = row.index();
                let entry = &entries[index];
                row.set_selected(*selected == Some(index));
                row.col(|ui| {
                    ui.label(RichText::new(&entry.name).size(SMALL));
                });
                row.col(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&entry.geometry).size(LABEL).weak());
                    });
                });
                if row.response().clicked() {
                    *selected = Some(index);
                }
            });
        });
}

/// What is true of the session rather than of the picture.
///
/// What the tab is doing reads on the state line under the image, so it is not
/// repeated here: faults run from the left while audio facts run from the right.
fn status_bar(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let audio = match app.audio.sample_rate_hz() {
        Some(rate) => app
            .i18n
            .text_with("status-audio", &[("rate", number(rate))]),
        None => app.i18n.text("status-no-audio"),
    };
    let output = match app.output_sample_rate_hz() {
        Some(rate) => app
            .i18n
            .text_with("status-output-audio", &[("rate", number(rate))]),
        None if app.audio.output_device.is_some() => app.i18n.text("status-output-ready"),
        None => app.i18n.text("status-no-output"),
    };
    let error_color = colors::ERROR;

    ui.horizontal(|ui| {
        if snapshot.dropped_samples > 0 {
            let dropped = app.i18n.text_with(
                "status-dropped",
                &[("samples", number(snapshot.dropped_samples as u32))],
            );
            ui.label(RichText::new(dropped).size(LABEL).color(error_color));
        }
        let reported = [
            app.audio.error.as_ref(),
            snapshot.error.as_ref(),
            app.rig_snapshot.error.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(AppError::to_string);
        for error in reported.chain(
            [
                app.tx_error.as_deref(),
                app.library_error.as_deref(),
                app.config_error(),
            ]
            .into_iter()
            .flatten()
            .map(str::to_owned),
        ) {
            ui.label(RichText::new(error).size(LABEL).color(error_color));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if !snapshot.callsigns.is_empty() {
                ui.label(RichText::new(snapshot.callsigns.join(" ")).size(LABEL));
            }
            ui.label(RichText::new(output).size(LABEL));
            ui.label(RichText::new(audio).size(LABEL));
        });
    });
}

#[cfg(test)]
mod tests {
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

    /// Runs the interface for a few frames and returns the harness.
    ///
    /// egui reports duplicate widget ids and bad layouts by panicking at run
    /// time rather than at compile time, so the whole view is exercised here:
    /// nothing else in the suite would notice.
    fn render(app: &mut App) -> Harness<'_> {
        let mut harness = Harness::new_ui(|ui| {
            let model = menu::model(app);
            view(ui, app, &model);
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
        assert_eq!(app.custom_draft.len(), 2);
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
        app.library_error = Some("library unavailable".to_owned());
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

    /// A populated library exercises the entry rows, which an interface built
    /// over empty directories never reaches.
    #[test]
    fn a_populated_library_renders() {
        let mut app = App::headless();
        app.templates = vec![crate::app::Entry::sample("field-day.kdl", "")];
        app.template = Some(0);
        app.stocks = vec![
            crate::app::Entry::sample("antenna.png", "640×496"),
            crate::app::Entry::sample("shack.png", "320×256"),
        ];
        app.stock = Some(1);

        // Rows carry the geometry alongside the name, so the accessible label
        // is the two of them together rather than the file name alone.
        let harness = render(&mut app);
        harness.get_by_label_contains("field-day.kdl");
        harness.get_by_label_contains("antenna.png");
    }

    /// The row is what senses the click, so its text must not. A selectable
    /// label takes the text cursor and swallows the press, which leaves the
    /// row unclickable exactly where it has text on it.
    #[test]
    fn clicking_a_row_label_selects_that_row() {
        let mut app = App::headless();
        app.stocks = vec![
            crate::app::Entry::sample("antenna.png", "640x496"),
            crate::app::Entry::sample("shack.png", "320x256"),
        ];
        app.stock = Some(0);

        {
            let mut harness = Harness::new_ui(|ui| {
                let model = menu::model(&app);
                view(ui, &mut app, &model);
            });
            harness.run();
            harness.get_by_label("shack.png").click();
            harness.run();
        }

        assert_eq!(app.stock, Some(1));
    }

    /// The lists decide the image a transmission is sending, so clicking one
    /// while it runs must not change the selection under it.
    #[test]
    fn a_transmission_locks_the_library_lists() {
        let mut app = App::headless();
        app.tx_snapshot.phase = TxPhase::Producing;
        app.stocks = vec![
            crate::app::Entry::sample("antenna.png", "640x496"),
            crate::app::Entry::sample("shack.png", "320x256"),
        ];
        app.stock = Some(0);

        {
            let mut harness = Harness::new_ui(|ui| {
                let model = menu::model(&app);
                view(ui, &mut app, &model);
            });
            harness.run();
            harness.get_by_label("shack.png").click();
            harness.run();
        }

        assert_eq!(app.stock, Some(0));
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
        app.templates = vec![crate::app::Entry::sample("field-day.kdl", "")];
        app.stocks = vec![crate::app::Entry::sample("antenna.png", "320x256")];
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
        app.station_open = true;
        let title = app.i18n.text("station-title");
        let close = app.i18n.text("station-close");

        {
            let mut harness = Harness::new_ui(|ui| {
                let model = menu::model(&app);
                view(ui, &mut app, &model);
            });
            harness.run();
            harness.get_by_label(&title);
            harness.get_by_label(&close).click();
            harness.run();
        }

        assert!(!app.station_open);
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
                view(ui, &mut app, &model);
            });
            harness.run();
            harness.get_by_label("TX").click();
            harness.run();
        }

        assert_eq!(app.tx_error, None);
        assert_eq!(app.tx_snapshot.phase, TxPhase::Idle);
    }
}
