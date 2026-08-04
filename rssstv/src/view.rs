use egui::{Align, Color32, ComboBox, Id, Layout, Panel, ProgressBar, RichText, Ui};
use egui_extras::{Column, TableBuilder};

use rssstv_audio::FaultKind;

use crate::{
    app::{App, Dsp, Entry, Tab},
    canvas,
    i18n::{number, text as arg},
    menu,
    receive::Progress,
    transmit::{TxPhase, TxProgress},
};

const SIDE_PANEL_WIDTH: f32 = 320.0;
/// Default height of the library row; the operator can drag it.
const LIBRARY_HEIGHT: f32 = 260.0;
const LIST_WIDTH: f32 = 260.0;
const FIELD_LABEL_WIDTH: f32 = 64.0;

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
    Panel::top(Id::new("toolbar")).show(ui, |ui| toolbar(ui, app));
    Panel::bottom(Id::new("status-bar")).show(ui, |ui| status_bar(ui, app));
    Panel::bottom(Id::new("library"))
        .resizable(true)
        .default_size(LIBRARY_HEIGHT)
        .size_range(160.0..=640.0)
        .show(ui, |ui| library(ui, app));
    Panel::right(Id::new("side-panel"))
        .resizable(true)
        .default_size(SIDE_PANEL_WIDTH)
        .size_range(260.0..=560.0)
        .show(ui, |ui| side_panel(ui, app));
    egui::CentralPanel::default().show(ui, |ui| main_pane(ui, app));
    device_fault_modal(ui, app);
    action
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

fn toolbar(ui: &mut Ui, app: &mut App) {
    ui.horizontal(|ui| {
        for tab in Tab::ALL {
            let label = app.i18n.text(tab.label_key());
            ui.selectable_value(&mut app.tab, tab, label);
        }
    });
}

fn main_pane(ui: &mut Ui, app: &mut App) {
    let badge = badge(app);
    let geometry = geometry_label(app);
    let fraction = app.decoded_fraction();

    let available = ui.available_height();
    ui.allocate_ui(egui::vec2(ui.available_width(), available - 32.0), |ui| {
        let area = canvas::image_view(ui, app.active_raster_mut(), fraction);
        ui.painter().text(
            area.min + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            badge,
            egui::FontId::proportional(LABEL),
            Color32::from_rgb(200, 200, 210),
        );
    });
    action_bar(ui, app, &geometry);
}

fn badge(app: &App) -> String {
    let name = app.active_mode().spec().name();
    let mode = arg(name);
    match app.tab {
        Tab::Transmit => match app.tx_snapshot.phase {
            TxPhase::Priming => app
                .i18n
                .text_with("badge-transmit-preparing", &[("mode", mode)]),
            TxPhase::Producing | TxPhase::Draining => match app.tx_progress() {
                TxProgress::Scanning { rows, total } => app.i18n.text_with(
                    "badge-transmitting",
                    &[
                        ("mode", mode),
                        ("row", number(rows as u32)),
                        ("total", number(total as u32)),
                    ],
                ),
                TxProgress::Identifying => app
                    .i18n
                    .text_with("badge-transmit-identifying", &[("mode", mode)]),
                _ => app
                    .i18n
                    .text_with("badge-transmit-leader", &[("mode", mode)]),
            },
            TxPhase::Complete => app
                .i18n
                .text_with("badge-transmit-complete", &[("mode", mode)]),
            _ if app.can_transmit() => app
                .i18n
                .text_with("badge-transmit-ready", &[("mode", mode)]),
            _ => app
                .i18n
                .text_with("badge-transmit-not-ready", &[("mode", mode)]),
        },
        Tab::Receive => {
            let progress = app.audio.snapshot().progress;
            // A stopped reception leaves a partial image on the canvas, so it
            // has to read differently from having nothing at all.
            if progress == Progress::Stopped {
                return app.i18n.text_with("badge-stopped", &[("mode", mode)]);
            }
            if !progress.is_active() && progress != Progress::Complete {
                return app.i18n.text("badge-waiting");
            }
            if progress == Progress::Complete {
                app.i18n.text_with("badge-complete", &[("mode", mode)])
            } else {
                let percent = (progress.fraction() * 100.0).round();
                app.i18n.text_with(
                    "badge-receiving",
                    &[("mode", mode), ("percent", number(percent))],
                )
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

fn action_bar(ui: &mut Ui, app: &mut App, geometry: &str) {
    ui.horizontal(|ui| {
        match app.tab {
            Tab::Receive => {
                pending(ui, app, "action-lock");
                pending(ui, app, "action-resync");
                let label = app.i18n.text("action-auto-history");
                ui.checkbox(&mut app.auto_history, label);
                ui.label(app.i18n.text("history-format"));
                ComboBox::from_id_salt("receive-history-format")
                    .selected_text(app.i18n.text(app.history_format.label_key()))
                    .show_ui(ui, |ui| {
                        for format in crate::history::HistoryFormat::ALL {
                            ui.selectable_value(
                                &mut app.history_format,
                                format,
                                app.i18n.text(format.label_key()),
                            );
                        }
                    });
            }
            Tab::Transmit => {
                let active = app.tx_snapshot.phase.is_active();
                let label = app.i18n.text(if active {
                    "action-stop-transmit"
                } else {
                    "action-transmit"
                });
                let mut response = ui.add(
                    egui::Button::new(RichText::new(label).size(SMALL))
                        .fill(Color32::from_rgb(140, 40, 40)),
                );
                if !active && let Some(problem) = app.transmit_problem() {
                    response = response.on_hover_text(problem);
                }
                if response.clicked() {
                    if active {
                        app.stop_transmit();
                    } else {
                        app.start_transmit();
                    }
                }
                pending(ui, app, "action-tone");
                pending(ui, app, "action-cw");
                pending(ui, app, "action-fskid");
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            pending(ui, app, "action-zoom");
            match app.tab {
                Tab::Receive => pending(ui, app, "action-save"),
                Tab::Transmit => pending(ui, app, "action-paste"),
            }
            ui.label(RichText::new(geometry).size(SMALL));
        });
    });
}

/// A control whose behavior arrives with a later feature.
fn pending(ui: &mut Ui, app: &App, key: &str) {
    ui.add_enabled(
        false,
        egui::Button::new(RichText::new(app.i18n.text(key)).size(SMALL)),
    );
}

fn side_panel(ui: &mut Ui, app: &mut App) {
    // The sections below can outgrow the panel's height on a small window or
    // a large font scale; scroll rather than silently clipping the bottom.
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(4.0);
        let status_title = app.i18n.text("section-rx-status");
        section(ui, &status_title, |ui| receive_controls(ui, app));
        ui.add_space(16.0);
        let qso_title = app.i18n.text("section-qso");
        section(ui, &qso_title, |ui| qso_panel(ui, app));
    });
}

fn receive_controls(ui: &mut Ui, app: &mut App) {
    ui.set_width(ui.available_width());
    rx_status(ui, app);
    ui.add_space(12.0);
    heading(ui, &app.i18n.text("section-mode"));
    mode_panel(ui, app);
    ui.add_space(12.0);
    heading(ui, &app.i18n.text("section-dsp"));
    dsp_panel(ui, app);
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

fn rx_status(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let color = if snapshot.progress.is_active() {
        Color32::from_rgb(80, 200, 120)
    } else {
        Color32::WHITE
    };
    ui.add(ProgressBar::new(snapshot.level).fill(color));
}

fn mode_panel(ui: &mut Ui, app: &mut App) {
    if app.tab != Tab::Transmit {
        let label = app.i18n.text("label-auto-vis");
        ui.checkbox(&mut app.auto_mode, RichText::new(label).size(SMALL));
    }
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
        let width = (ui.available_width() - gaps) / Dsp::ALL.len() as f32;
        for dsp in Dsp::ALL {
            let label = RichText::new(app.i18n.text(dsp.label_key())).size(SMALL);
            let button = egui::Button::new(label)
                .min_size(egui::vec2(width, 0.0))
                .selected(app.dsp.get(dsp));
            if ui.add(button).clicked() {
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
    let station_label = app.i18n.text("qso-station-call");
    let report_label = app.i18n.text("qso-rsv-nr");

    ui.horizontal(|ui| {
        field_label(ui, &station_label);
        let edit = egui::TextEdit::singleline(&mut app.station_callsign).desired_width(fields);
        if ui.add(edit).changed() {
            app.normalize_station_callsign();
        }
    });
    ui.horizontal(|ui| {
        field_label(ui, &call_label);
        let edit = egui::TextEdit::singleline(&mut app.qso.call).desired_width(fields);
        if ui.add(edit).changed() {
            app.normalize_call();
            app.qso_changed();
        }
    });
    ui.horizontal(|ui| {
        // RSV and the serial number are one report, so they share a label and
        // sit next to each other rather than being introduced separately.
        field_label(ui, &report_label);
        let field_width = (fields - gap) / 2.0;
        let rsv = ui.add(egui::TextEdit::singleline(&mut app.qso.rsv).desired_width(field_width));
        let number =
            ui.add(egui::TextEdit::singleline(&mut app.qso.number).desired_width(field_width));
        if rsv.changed() || number.changed() {
            app.qso_changed();
        }
    });
    ui.horizontal(|ui| {
        pending(ui, app, "qso-record");
        if ui
            .button(RichText::new(app.i18n.text("qso-clear")).size(SMALL))
            .clicked()
        {
            app.clear_qso();
        }
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

fn library(ui: &mut Ui, app: &mut App) {
    ui.add_space(2.0);
    ui.horizontal_top(|ui| {
        let height = ui.available_height();

        let labels = ListLabels::new(app, "section-templates");
        let previous_template = app.template;
        match entry_list(ui, &labels, height, &app.templates, &mut app.template) {
            Some(ListAction::Reveal) => app.reveal_templates(),
            Some(ListAction::Refresh) => app.refresh_templates(),
            None => {}
        }
        if app.template != previous_template {
            app.preview_changed();
        }

        let labels = ListLabels::new(app, "section-stocks");
        let previous_stock = app.stock;
        match entry_list(ui, &labels, height, &app.stocks, &mut app.stock) {
            Some(ListAction::Reveal) => app.reveal_stocks(),
            Some(ListAction::Refresh) => app.refresh_stocks(),
            None => {}
        }
        if app.stock != previous_stock {
            app.preview_changed();
        }

        composite(ui, app);
    });
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
    height: f32,
    entries: &[Entry],
    selected: &mut Option<usize>,
) -> Option<ListAction> {
    let mut action = None;
    // The enclosing layout runs left to right, so the header and the list are
    // wrapped in a vertical Ui; without it they end up side by side.
    ui.allocate_ui(egui::vec2(LIST_WIDTH, height), |ui| {
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

fn composite(ui: &mut Ui, app: &mut App) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            heading(ui, &app.i18n.text("section-composite"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = app.i18n.text("action-set-transmit");
                if ui
                    .add_enabled(
                        app.can_set_for_transmit(),
                        egui::Button::new(RichText::new(label).size(SMALL)),
                    )
                    .clicked()
                {
                    app.set_for_transmit();
                }
                pending(ui, app, "action-edit");
            });
        });
        canvas::image_view(ui, &mut app.composite_raster, 1.0);
    });
}

fn status_bar(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let status = if app.tx_snapshot.phase.is_active() {
        match app.tx_progress() {
            TxProgress::Scanning { rows, total } => app.i18n.text_with(
                "status-transmitting",
                &[
                    ("row", number(rows as u32)),
                    ("total", number(total as u32)),
                ],
            ),
            TxProgress::Identifying => app.i18n.text("status-transmit-identifying"),
            _ => app.i18n.text("status-transmit-leader"),
        }
    } else if snapshot.progress.is_active() {
        let percent = (snapshot.progress.fraction() * 100.0).round();
        app.i18n
            .text_with("status-receiving", &[("percent", number(percent))])
    } else {
        app.i18n.text("status-idle")
    };
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

    ui.horizontal(|ui| {
        ui.label(RichText::new(status).size(LABEL));
        ui.label(RichText::new(audio).size(LABEL));
        ui.label(RichText::new(output).size(LABEL));
        if !snapshot.callsigns.is_empty() {
            ui.label(RichText::new(snapshot.callsigns.join(" ")).size(LABEL));
        }
        if snapshot.dropped_samples > 0 {
            let dropped = app.i18n.text_with(
                "status-dropped",
                &[("samples", number(snapshot.dropped_samples as u32))],
            );
            ui.label(RichText::new(dropped).size(LABEL));
        }
        for error in [
            app.audio.error.as_deref(),
            snapshot.error.as_deref(),
            app.tx_error.as_deref(),
            app.library_error.as_deref(),
            app.config_error(),
        ]
        .into_iter()
        .flatten()
        {
            ui.label(
                RichText::new(error)
                    .size(LABEL)
                    .color(Color32::from_rgb(220, 120, 120)),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::{Harness, kittest::Queryable as _};
    use rstest::rstest;

    use super::*;
    use crate::{app::App, i18n::Locale};

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

    #[test]
    fn switching_tabs_does_not_collide_widget_ids() {
        let mut app = App::headless();
        for tab in Tab::ALL {
            app.tab = tab;
            render(&mut app);
        }
    }

    #[test]
    fn tx_button_reports_why_transmission_cannot_start() {
        let mut app = App::headless();
        app.tab = Tab::Transmit;
        {
            let mut harness = Harness::new_ui(|ui| {
                let model = menu::model(&app);
                view(ui, &mut app, &model);
            });
            harness.run();
            harness.get_by_label("TX").click();
            harness.run();
        }
        assert_eq!(
            app.tx_error.as_deref(),
            Some(app.i18n.text("error-no-transmit-frame").as_str())
        );
    }
}
