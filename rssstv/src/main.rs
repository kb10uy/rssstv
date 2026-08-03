#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;

use egui_system_fonts::{FontRegion, FontStyle};

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
        // egui ships Latin glyphs only, so the Japanese locale needs the
        // platform's own fonts appended before any text is laid out.
        egui_system_fonts::add_with_region(&cc.egui_ctx, FontRegion::Japanese, FontStyle::Sans);

        let app = App::new(paths);
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
