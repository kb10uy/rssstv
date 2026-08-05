//! The row of lists the operator picks a template and a picture from.

use super::*;

/// The template and stock lists, which together decide the transmit image.
///
/// A transmission is sending the image these lists produced, so they are
/// disabled while one is running rather than letting the operator choose
/// something that could not take effect until it ends.
pub(super) fn library(ui: &mut Ui, app: &mut App) {
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
