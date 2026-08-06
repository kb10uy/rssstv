//! The line along the bottom, which reports rather than offers anything.

use super::*;

/// What is true of the session rather than of the picture.
///
/// What the tab is doing reads on the state line under the image, so it is not
/// repeated here: faults run from the left while audio facts run from the right.
pub(super) fn status_bar(ui: &mut Ui, app: &App) {
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
                app.library.error.as_deref(),
                app.config_error(),
            ]
            .into_iter()
            .flatten()
            .map(str::to_owned),
        ) {
            ui.label(RichText::new(error).size(LABEL).color(error_color));
        }
        if let Some(notice) = app.notice.as_deref() {
            ui.label(RichText::new(notice.to_owned()).size(LABEL));
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
