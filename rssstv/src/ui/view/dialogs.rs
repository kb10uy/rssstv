//! The windows that open over the interface.
//!
//! Each is modal and each edits something set once and then left alone, which
//! is why none of them sits in a panel.

use super::*;

/// Edits what the station says about itself.
///
/// Kept out of the QSO panel and behind the Settings menu because none of it
/// belongs to the contact being worked: it is set once for the operator and
/// then left alone, while the panel beside the image is for the station on the
/// air right now.
pub(super) fn station_dialog(ui: &mut Ui, app: &mut App) {
    if !app.station.open {
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
        finished = station_field(ui, &labels[0], &mut app.station.callsign, width);
        finished |= station_field(ui, &labels[1], &mut app.station.qth, width);
        finished |= station_field(ui, &labels[2], &mut app.station.grid, width);
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
        app.station.open = false;
    }
}

/// Edits the variables the operator invented for their own templates.
///
/// Everything else a template can read is something the application already
/// knows; these are the ones only the operator does, so this is the one place
/// where both the name and the value are typed.
pub(super) fn custom_variable_dialog(ui: &mut Ui, app: &mut App) {
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
pub(super) fn station_field(ui: &mut Ui, label: &str, text: &mut String, width: f32) -> bool {
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
pub(super) fn device_fault_modal(ui: &mut Ui, app: &mut App) {
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
