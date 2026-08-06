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
        transmit::{TUNE_FREQUENCY_HZ, TxGain, TxPhase, TxProgress},
    },
};

mod dialogs;
mod library;
mod panels;
mod status_bar;

use dialogs::{custom_variable_dialog, device_fault_modal, station_dialog};
use library::library;
use panels::side_panel;
use status_bar::status_bar;

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
    // Fixed rather than resizable: everything in it is laid out from the width
    // it is given, so there is nothing in there a wider panel would show more
    // of, and the picture beside it is what the rest of the window is for.
    // Exact rather than a default width: a panel keeps the width it was last
    // laid out at, and a window dragged narrow enough to squeeze it stores the
    // squeezed width and stays there once the window is given its size back.
    Panel::right(Id::new("side-panel"))
        .resizable(false)
        .exact_size(SIDE_PANEL_WIDTH)
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
        Tab::Transmit if app.is_tuning() => app.i18n.text("state-transmit-tone"),
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
            // Nothing is being listened for while the station transmits, so the
            // line says so rather than reporting a wait that is not happening.
            if app.audio.is_muted_for_transmit() {
                return app.i18n.text("state-rx-muted");
            }
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

/// The transmit trigger, and the tune tone beside it.
///
/// The tone takes a quarter of the row: it is pressed far less often than the
/// trigger is, and it is the width its caption needs rather than a share of
/// what the operator reaches for.
fn transmit_controls(ui: &mut Ui, app: &mut App, height: f32) {
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let full = ui.available_width();
        let tone = ((full - gap) / 4.0).max(0.0);
        transmit_button(ui, app, full - gap - tone, height);
        tone_button(ui, app, tone, height);
    });
}

/// Starts or stops a transmission.
///
/// It sits where the receive tab keeps its DSP toggles, so the control that
/// acts on the picture is in the panel beside it rather than under it.
fn transmit_button(ui: &mut Ui, app: &mut App, width: f32, height: f32) {
    // A tone is transmitting too, but not this: stopping it is the other
    // button's, so this one is disabled and says what is holding the rig.
    let active = app.tx_snapshot.phase.is_active() && !app.is_tuning();
    let label = app.i18n.text(if active {
        "action-stop-transmit"
    } else {
        "action-transmit"
    });
    let size = egui::vec2(width, height);
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

/// Sends the steady tone a repeater is opened with, for as long as it is on.
///
/// Captioned with the frequency itself, the way MMSSTV captions it: the button
/// is asked for by that number rather than by anything it could be called.
fn tone_button(ui: &mut Ui, app: &mut App, width: f32, height: f32) {
    let active = app.is_tuning();
    let caption = TUNE_FREQUENCY_HZ.to_string();
    let hint = app
        .i18n
        .text_with("action-tone", &[("frequency", arg(&caption))]);
    let size = egui::vec2(width, height);
    let problem = (!active).then(|| app.tone_problem()).flatten();
    let button = egui::Button::new(RichText::new(caption).size(SMALL)).selected(active);
    let mut response = ui
        .add_enabled_ui(problem.is_none(), |ui| ui.add_sized(size, button))
        .inner;
    if let Some(problem) = problem {
        response = response.on_disabled_hover_text(problem);
    }
    if response.on_hover_text(hint).clicked() {
        if active {
            app.stop_tone();
        } else {
            app.start_tone();
        }
    }
}

fn heading(ui: &mut Ui, label: &str) {
    ui.label(RichText::new(label).size(LABEL).weak());
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

enum ListAction {
    Reveal,
    Refresh,
}

#[cfg(test)]
mod tests;
