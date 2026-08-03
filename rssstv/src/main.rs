#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

use egui_system_fonts::{FontPreset, FontStyle};

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
/// The families are installed at the front of egui's list rather than as a
/// fallback, so Latin and Japanese come from one face instead of mixing the
/// bundled font with a system one. The crate's own presets follow as a
/// backstop; when nothing at all matches, egui keeps its bundled fonts.
fn install_fonts(ctx: &egui::Context) {
    let families = UI_FONTS.iter().map(|name| (*name).to_owned()).collect();
    let applied = egui_system_fonts::set_with_presets(
        ctx,
        [
            FontPreset::Custom(families),
            FontPreset::Japanese,
            FontPreset::Latin,
        ],
        FontStyle::Sans,
    );
    if applied.is_empty() {
        eprintln!("no system UI font matched; using the bundled fonts");
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let paths = paths::AppPaths::discover()?;
    paths.initialize()?;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 940.0]),
        ..Default::default()
    };
    eframe::run_native(
        "rssstv",
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
