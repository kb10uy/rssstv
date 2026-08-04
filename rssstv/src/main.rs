#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{error::Error, sync::Arc};

use egui::{FontData, FontDefinitions, FontFamily};

mod app;
mod i18n;
mod platform;
mod storage;
mod ui;
mod worker;

use app::App;
use platform::UI_FONTS;
use storage::{log, paths};
use ui::{menu, view};

/// Draws the interface with the platform's UI font.
///
/// The families go at the front of egui's list rather than being appended as
/// a fallback, so Latin and Japanese come from one face instead of mixing the
/// bundled font with a system one. egui's own fonts stay behind them, so a
/// machine with none of these installed still renders text.
///
/// The font database is queried directly because the face index matters:
/// Windows ships `Yu Gothic UI` as the second face of a collection whose
/// first face is `Yu Gothic`, and that first face carries a half-em line gap.
/// Loading it would leave every label sitting high in its row.
fn install_fonts(ctx: &egui::Context) {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    ctx.set_fonts(font_definitions(&database));
}

/// Puts whichever of [`UI_FONTS`] the system has in front of egui's own.
fn font_definitions(database: &fontdb::Database) -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    let mut installed = Vec::new();
    for family in UI_FONTS {
        let Some((data, index)) = load_face(database, family) else {
            continue;
        };
        definitions.font_data.insert(
            (*family).to_owned(),
            Arc::new(FontData {
                font: data.into(),
                index,
                tweak: egui::FontTweak::default(),
            }),
        );
        installed.push((*family).to_owned());
    }

    if installed.is_empty() {
        log::note("no system UI font matched; using the bundled fonts");
    }
    for family in installed.iter().rev() {
        for target in [FontFamily::Proportional, FontFamily::Monospace] {
            definitions
                .families
                .entry(target)
                .or_default()
                .insert(0, family.clone());
        }
    }
    definitions
}

/// Returns the bytes and face index of `family`, if the system has it.
fn load_face(database: &fontdb::Database, family: &str) -> Option<(Vec<u8>, u32)> {
    let id = database.query(&fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    })?;
    let (source, index) = database.face_source(id)?;
    let data = match source {
        fontdb::Source::File(path) => std::fs::read(path).ok()?,
        fontdb::Source::Binary(data) => data.as_ref().as_ref().to_vec(),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    Some((data, index))
}

/// The window size the interface is laid out for, in points.
const DEFAULT_WINDOW_SIZE: [f32; 2] = [1024.0, 768.0];

/// How much of the monitor the window may take up when it first opens.
///
/// The rest is left to the panels and window decorations the desktop puts
/// around it, whose sizes the application cannot ask for.
const MONITOR_FRACTION: f32 = 0.92;

fn main() -> Result<(), Box<dyn Error>> {
    platform::prepare_process();

    // A second copy would fail to open the audio devices the first one holds,
    // which is harder to understand than not opening at all. The running copy
    // has already been asked to come forward by the time this returns.
    let Some(instance) = platform::claim_single_instance() else {
        return Ok(());
    };

    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;
    if let Err(error) = log::open(paths.log_file()) {
        eprintln!("could not open the log file: {error}");
    }

    // eframe's own clamp converts the monitor to points with the scale factor
    // winit reports before the window exists. A Wayland compositor only sends
    // the fractional scale once the surface is mapped, so a fractionally
    // scaled output reads as the next whole number until then and the window
    // opens at a fraction of the size asked for. `fit_to_monitor` does the
    // same job once the real scale has arrived.
    let mut viewport = egui::ViewportBuilder::default()
        .with_clamp_size_to_monitor_size(false)
        .with_inner_size(DEFAULT_WINDOW_SIZE);
    match platform::window_icon() {
        Some(icon) => viewport = viewport.with_icon(icon),
        None => log::note("could not load the application icon; using the platform default"),
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        concat!("RSSSTV ", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|cc| Ok(Box::new(Interface::new(cc, paths, instance)))),
    )?;
    Ok(())
}

/// The eframe entry point, holding the application and its menu bar.
struct Interface {
    app: App,
    menu: Option<menu::Native>,
    /// The title the window is currently showing.
    ///
    /// Held so the localized title is only pushed to the platform when it
    /// actually changes, rather than on every frame.
    title: String,
    /// Set when the opening size has been measured against the monitor.
    ///
    /// The monitor is not known on the first frame, so this stays clear until
    /// a frame reports one.
    fitted: bool,
    /// Set when the operator chose to quit, so the frame can finish drawing
    /// and persist before the window closes.
    quitting: bool,
    /// The claim on being the only running copy.
    ///
    /// Held here so it lasts as long as the interface does, and released with
    /// it so the next launch is let through.
    _instance: platform::SingleInstance,
}

/// Shrinks the window to fit the monitor it opened on.
///
/// Returns whether the monitor was known yet, not whether anything was
/// resized: a window that already fits is left alone.
fn fit_to_monitor(ctx: &egui::Context) -> bool {
    let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else {
        return false;
    };
    let wanted = egui::Vec2::from(DEFAULT_WINDOW_SIZE);
    let fitted = wanted.min(monitor * MONITOR_FRACTION);
    if fitted != wanted {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(fitted));
    }
    true
}

impl Interface {
    fn new(
        cc: &eframe::CreationContext<'_>,
        paths: paths::AppPaths,
        instance: platform::SingleInstance,
    ) -> Self {
        install_fonts(&cc.egui_ctx);
        platform::prepare_window(cc);
        instance.publish_window(cc);

        let app = App::new(paths);
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        let model = menu::model(&app);
        let menu = match menu::Native::install(cc, &model) {
            Ok(menu) => Some(menu),
            Err(error) => {
                log::note(&format!("could not install the platform menu bar: {error}"));
                None
            }
        };
        Self {
            title: app.title(),
            app,
            menu,
            fitted: false,
            quitting: false,
            _instance: instance,
        }
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        if let Some(menu) = &self.menu {
            menu.prepare_for_close();
        }
    }
}

impl eframe::App for Interface {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The receive worker runs ahead of the interface, so a repaint is
        // requested unconditionally: a decoded row must not wait for input.
        ui.ctx().request_repaint();
        if !self.fitted {
            self.fitted = fit_to_monitor(ui.ctx());
        }
        self.app.poll_audio();

        // egui handles Ctrl+Plus/Minus itself, so the factor it holds is
        // adopted before the menu can change it. Whichever route the operator
        // took, the result is one value that gets persisted.
        self.app.set_ui_scale(ui.ctx().zoom_factor());

        let model = menu::model(&self.app);
        if let Some(native) = self.menu.as_mut() {
            native.sync(&model);
            for action in native.poll() {
                self.quitting |= menu::apply(&mut self.app, action);
            }
        }
        if let Some(action) = view::view(ui, &mut self.app, &model) {
            self.quitting |= menu::apply(&mut self.app, action);
        }

        if ui.ctx().zoom_factor() != self.app.ui_scale {
            ui.ctx().set_zoom_factor(self.app.ui_scale);
        }

        let title = self.app.title();
        if title != self.title {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        self.app.persist();
        if self.quitting {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system with none of the wanted families still has to render text.
    #[test]
    fn an_empty_database_leaves_the_bundled_fonts_usable() {
        let definitions = font_definitions(&fontdb::Database::new());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            assert!(!definitions.families[&family].is_empty());
        }
    }

    /// The wanted families have to come first, or Latin keeps rendering in
    /// egui's bundled font while only Japanese falls through to the system.
    #[test]
    fn a_matched_family_is_installed_ahead_of_the_bundled_fonts() {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let Some(wanted) = UI_FONTS
            .iter()
            .find(|family| load_face(&database, family).is_some())
        else {
            return; // No system font to check against on this machine.
        };

        let definitions = font_definitions(&database);
        let bundled = FontDefinitions::default();
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let installed = &definitions.families[&family];
            assert_eq!(installed.first().map(String::as_str), Some(*wanted));
            assert!(
                installed.len() > bundled.families[&family].len(),
                "the bundled fonts should still be behind the system ones"
            );
        }
    }
}
