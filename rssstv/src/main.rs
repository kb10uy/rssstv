#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{error::Error, sync::Arc};

use egui::{FontData, FontDefinitions, FontFamily};

mod app;
mod audio;
mod canvas;
mod config;
mod i18n;
mod menu;
mod paths;
mod raster;
mod receive;
mod view;

use app::App;

/// Font families the interface is drawn with, in priority order.
///
/// The platform's own UI face is named rather than left to the font crate's
/// list, which puts `Noto Sans JP` first. On Windows that resolves to
/// `NotoSansJP-VF.ttf`, a variable font whose weight axis defaults to Thin;
/// egui does not apply variable axes, so the whole interface would be drawn
/// hairline.
#[cfg(target_os = "windows")]
const UI_FONTS: [&str; 3] = ["Yu Gothic UI", "Meiryo UI", "Segoe UI"];
#[cfg(target_os = "macos")]
const UI_FONTS: [&str; 2] = ["Hiragino Sans", "Helvetica Neue"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const UI_FONTS: [&str; 3] = ["Noto Sans CJK JP", "Noto Sans", "DejaVu Sans"];

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
        eprintln!("no system UI font matched; using the bundled fonts");
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

fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "windows")]
    menu::allow_dark_mode_for_app();

    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 940.0]),
        ..Default::default()
    };
    eframe::run_native(
        concat!("RSSSTV ", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|cc| Ok(Box::new(Interface::new(cc, paths)))),
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
    /// Set when the operator chose to quit, so the frame can finish drawing
    /// and persist before the window closes.
    quitting: bool,
}

impl Interface {
    fn new(cc: &eframe::CreationContext<'_>, paths: paths::AppPaths) -> Self {
        install_fonts(&cc.egui_ctx);

        let app = App::new(paths);
        cc.egui_ctx.set_zoom_factor(app.ui_scale);
        let model = menu::model(&app);
        let menu = match menu::Native::install(cc, &model) {
            Ok(menu) => Some(menu),
            Err(error) => {
                eprintln!("could not install the platform menu bar: {error}");
                None
            }
        };
        Self {
            title: app.title(),
            app,
            menu,
            quitting: false,
        }
    }
}

impl eframe::App for Interface {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The receive worker runs ahead of the interface, so a repaint is
        // requested unconditionally: a decoded row must not wait for input.
        ui.ctx().request_repaint();
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
