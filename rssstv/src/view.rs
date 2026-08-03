use egui::{Align, Color32, ComboBox, Id, Layout, Panel, ProgressBar, RichText, ScrollArea, Ui};

use crate::app::{App, Dsp, Entry, Tab};
use crate::canvas;
use crate::i18n::{number, text as arg};
use crate::menu;
use crate::receive::Progress;

const SIDE_PANEL_WIDTH: f32 = 320.0;
const LIBRARY_HEIGHT: f32 = 246.0;
const LIST_WIDTH: f32 = 236.0;
const SMALL: f32 = 12.0;
const LABEL: f32 = 11.0;

/// Draws the whole interface, returning the menu action the operator chose.
pub fn view(ui: &mut Ui, app: &mut App, model: &[menu::Menu]) -> Option<menu::Action> {
    let mut action = None;
    if menu::is_in_window() {
        Panel::top(Id::new("menu-bar")).show(ui, |ui| {
            action = menu::bar(ui, model);
        });
    }
    Panel::top(Id::new("toolbar")).show(ui, |ui| toolbar(ui, app));
    Panel::bottom(Id::new("status-bar")).show(ui, |ui| status_bar(ui, app));
    Panel::bottom(Id::new("library"))
        .exact_size(LIBRARY_HEIGHT)
        .show(ui, |ui| library(ui, app));
    Panel::right(Id::new("side-panel"))
        .exact_size(SIDE_PANEL_WIDTH)
        .show(ui, |ui| side_panel(ui, app));
    egui::CentralPanel::default().show(ui, |ui| main_pane(ui, app));
    action
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
        Tab::Transmit => app
            .i18n
            .text_with("badge-transmit-ready", &[("mode", mode)]),
        Tab::History => app.i18n.text_with("badge-history", &[("mode", mode)]),
        Tab::Receive => {
            let progress = app.audio.snapshot().progress;
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
        Tab::Receive | Tab::History => app.rx_raster.size(),
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
            }
            Tab::Transmit => {
                let label = app.i18n.text("action-transmit");
                ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new(label).size(SMALL))
                        .fill(Color32::from_rgb(140, 40, 40)),
                );
                pending(ui, app, "action-tone");
                pending(ui, app, "action-cw");
                pending(ui, app, "action-fskid");
            }
            Tab::History => {
                pending(ui, app, "action-save");
                pending(ui, app, "action-copy");
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            pending(ui, app, "action-zoom");
            match app.tab {
                Tab::Receive => pending(ui, app, "action-save"),
                Tab::Transmit => pending(ui, app, "action-paste"),
                Tab::History => {}
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
    receive_controls(ui, app);
    ui.add_space(16.0);
    let title = app.i18n.text("section-qso");
    section(ui, &title, |ui| qso_panel(ui, app));
}

fn receive_controls(ui: &mut Ui, app: &mut App) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        heading(ui, &app.i18n.text("section-rx-status"));
        rx_status(ui, app);
        ui.add_space(12.0);
        heading(ui, &app.i18n.text("section-mode"));
        mode_panel(ui, app);
        ui.add_space(12.0);
        heading(ui, &app.i18n.text("section-dsp"));
        dsp_panel(ui, app);
    });
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
        Tab::Receive | Tab::History => (app.rx_mode, app.rx_modes.clone()),
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
            Tab::Receive | Tab::History => app.select_rx_mode(chosen),
        }
    }
}

fn dsp_panel(ui: &mut Ui, app: &mut App) {
    let mut toggled = None;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 16.0) / 3.0;
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
    egui::Grid::new("qso").num_columns(2).show(ui, |ui| {
        ui.label(RichText::new(app.i18n.text("qso-call")).size(SMALL));
        if ui.text_edit_singleline(&mut app.qso.call).changed() {
            app.normalize_call();
        }
        ui.end_row();

        ui.label(RichText::new(app.i18n.text("qso-rsv")).size(SMALL));
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut app.qso.rsv);
            ui.label(RichText::new(app.i18n.text("qso-nr")).size(SMALL));
            ui.text_edit_singleline(&mut app.qso.number);
        });
        ui.end_row();
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

fn library(ui: &mut Ui, app: &mut App) {
    ui.horizontal_top(|ui| {
        let labels = ListLabels::new(app, "section-templates");
        match entry_list(ui, &labels, &app.templates, &mut app.template) {
            Some(ListAction::Reveal) => app.reveal_templates(),
            Some(ListAction::Refresh) => app.refresh_templates(),
            None => {}
        }

        let labels = ListLabels::new(app, "section-stocks");
        match entry_list(ui, &labels, &app.stocks, &mut app.stock) {
            Some(ListAction::Reveal) => app.reveal_stocks(),
            Some(ListAction::Refresh) => app.refresh_stocks(),
            None => {}
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
    entries: &[Entry],
    selected: &mut Option<usize>,
) -> Option<ListAction> {
    let mut action = None;
    ui.allocate_ui(egui::vec2(LIST_WIDTH, ui.available_height()), |ui| {
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
            ScrollArea::vertical()
                .id_salt(&labels.title)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if entries.is_empty() {
                        ui.label(RichText::new(&labels.empty).size(LABEL).weak());
                    }
                    for (index, entry) in entries.iter().enumerate() {
                        let row = ui.selectable_label(
                            *selected == Some(index),
                            RichText::new(&entry.name).size(SMALL),
                        );
                        if !entry.geometry.is_empty() {
                            row.clone()
                                .on_hover_text(RichText::new(&entry.geometry).size(LABEL));
                        }
                        if row.clicked() {
                            *selected = Some(index);
                        }
                    }
                });
        });
    });
    action
}

fn composite(ui: &mut Ui, app: &mut App) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            heading(ui, &app.i18n.text("section-composite"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                pending(ui, app, "action-set-transmit");
                pending(ui, app, "action-edit");
            });
        });
        canvas::image_view(ui, &mut app.tx_raster, 1.0);
    });
}

fn status_bar(ui: &mut Ui, app: &App) {
    let snapshot = app.audio.snapshot();
    let status = if snapshot.progress.is_active() {
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

    ui.horizontal(|ui| {
        ui.label(RichText::new(status).size(LABEL));
        ui.label(RichText::new(audio).size(LABEL));
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
