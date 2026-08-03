//! The application menu, described once and rendered per platform.
//!
//! muda can only attach a native menu to a window the platform owns, which
//! rules out Linux: it needs a gtk window there and winit does not create one.
//! Rather than maintain two menu definitions, the menu is built as a
//! platform-independent [model](model) that the native and in-window renderers
//! both consume, so the two paths cannot drift apart.

use crate::app::App;
use crate::i18n::Locale;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use native::Native;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use in_window::Native;

/// What activating a menu entry asks the application to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SelectDevice(String),
    SelectLocale(Locale),
    Quit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Item {
    Submenu {
        label: String,
        items: Vec<Item>,
    },
    Check {
        label: String,
        checked: bool,
        action: Action,
    },
    Command {
        label: String,
        action: Action,
    },
    /// An entry whose behavior arrives with a later feature.
    ///
    /// It is shown disabled rather than wired to a placeholder action, so the
    /// menu is reviewable without implying working commands.
    Pending(String),
    Separator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Menu {
    pub label: String,
    pub items: Vec<Item>,
}

/// Describes the menu as it should currently appear.
///
/// Rebuilt every frame from application state, so check marks and labels
/// follow the interface without anything having to invalidate them.
pub fn model(app: &App) -> Vec<Menu> {
    let text = |key: &str| app.i18n.text(key);
    vec![
        Menu {
            label: text("menu-file"),
            items: vec![
                Item::Pending(text("action-save")),
                Item::Separator,
                Item::Command {
                    label: text("menu-quit"),
                    action: Action::Quit,
                },
            ],
        },
        Menu {
            label: text("menu-edit"),
            items: vec![
                Item::Pending(text("action-copy")),
                Item::Pending(text("action-paste")),
            ],
        },
        Menu {
            label: text("menu-view"),
            items: vec![Item::Pending(text("action-zoom"))],
        },
        Menu {
            label: text("menu-settings"),
            items: vec![
                Item::Submenu {
                    label: text("input-device"),
                    items: device_items(app),
                },
                Item::Submenu {
                    label: text("menu-language"),
                    items: locale_items(app),
                },
            ],
        },
        Menu {
            label: text("menu-rig"),
            items: vec![Item::Pending(text("menu-rig"))],
        },
        Menu {
            label: text("menu-help"),
            items: vec![Item::Pending(text("menu-help"))],
        },
    ]
}

fn device_items(app: &App) -> Vec<Item> {
    if app.audio.devices.is_empty() {
        return vec![Item::Pending(app.i18n.text("status-no-audio"))];
    }
    let selected = app.audio.device.as_ref().map(|device| device.name());
    app.audio
        .devices
        .iter()
        .map(|device| Item::Check {
            label: device.name().to_owned(),
            checked: selected == Some(device.name()),
            action: Action::SelectDevice(device.name().to_owned()),
        })
        .collect()
}

fn locale_items(app: &App) -> Vec<Item> {
    Locale::ALL
        .into_iter()
        .map(|locale| Item::Check {
            label: locale.to_string(),
            checked: locale == app.i18n.locale(),
            action: Action::SelectLocale(locale),
        })
        .collect()
}

/// Applies `action` to the application.
///
/// Returns whether the application was asked to close.
pub fn apply(app: &mut App, action: Action) -> bool {
    match action {
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::Quit => return true,
    }
    false
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native {
    use std::collections::HashMap;

    use muda::{CheckMenuItem, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

    use super::{Action, Item, Menu};

    /// The platform menu bar, kept in step with the model.
    ///
    /// The menu is built once and then updated in place. Rebuilding it every
    /// frame would be visible to the window manager, and on Windows would
    /// re-measure the client area on each pass.
    pub struct Native {
        menu: muda::Menu,
        entries: Vec<Entry>,
        actions: HashMap<MenuId, Action>,
        model: Vec<Menu>,
    }

    /// One activatable entry, in the order the model produced it.
    enum Entry {
        Check(CheckMenuItem),
        Command(MenuItem),
        Pending(MenuItem),
        Submenu(Submenu),
    }

    impl Native {
        /// Builds the menu and attaches it to the platform.
        ///
        /// A failure here is reported rather than fatal: the application is
        /// usable without a menu bar, and the in-window controls still work.
        pub fn install(
            cc: &eframe::CreationContext<'_>,
            model: &[Menu],
        ) -> Result<Self, muda::Error> {
            let menu = muda::Menu::new();
            let mut native = Self {
                menu,
                entries: Vec::new(),
                actions: HashMap::new(),
                model: Vec::new(),
            };
            native.build(model)?;
            native.attach(cc)?;
            Ok(native)
        }

        #[cfg(target_os = "windows")]
        fn attach(&self, cc: &eframe::CreationContext<'_>) -> Result<(), muda::Error> {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

            let Ok(handle) = cc.window_handle() else {
                return Ok(());
            };
            if let RawWindowHandle::Win32(window) = handle.as_raw() {
                // muda installs its own window subclass on this handle and
                // answers WM_COMMAND itself, so the winit event loop needs no
                // cooperation for menu clicks.
                unsafe { self.menu.init_for_hwnd(window.hwnd.get()) }?;
            }
            Ok(())
        }

        #[cfg(target_os = "macos")]
        fn attach(&self, _cc: &eframe::CreationContext<'_>) -> Result<(), muda::Error> {
            self.menu.init_for_nsapp();
            Ok(())
        }

        /// Replaces every menu entry from `model`.
        fn build(&mut self, model: &[Menu]) -> Result<(), muda::Error> {
            while self.menu.remove_at(0).is_some() {}
            self.entries.clear();
            self.actions.clear();

            for menu in model {
                let submenu = Submenu::new(&menu.label, true);
                self.menu.append(&submenu)?;
                self.entries.push(Entry::Submenu(submenu.clone()));
                self.append_items(&submenu, &menu.items)?;
            }
            self.model = model.to_vec();
            Ok(())
        }

        fn append_items(&mut self, parent: &Submenu, items: &[Item]) -> Result<(), muda::Error> {
            for item in items {
                match item {
                    Item::Submenu { label, items } => {
                        let submenu = Submenu::new(label, true);
                        parent.append(&submenu)?;
                        self.entries.push(Entry::Submenu(submenu.clone()));
                        self.append_items(&submenu, items)?;
                    }
                    Item::Check {
                        label,
                        checked,
                        action,
                    } => {
                        let entry = CheckMenuItem::new(label, true, *checked, None);
                        parent.append(&entry)?;
                        self.actions.insert(entry.id().clone(), action.clone());
                        self.entries.push(Entry::Check(entry));
                    }
                    Item::Command { label, action } => {
                        let entry = MenuItem::new(label, true, None);
                        parent.append(&entry)?;
                        self.actions.insert(entry.id().clone(), action.clone());
                        self.entries.push(Entry::Command(entry));
                    }
                    Item::Pending(label) => {
                        let entry = MenuItem::new(label, false, None);
                        parent.append(&entry)?;
                        self.entries.push(Entry::Pending(entry));
                    }
                    Item::Separator => parent.append(&PredefinedMenuItem::separator())?,
                }
            }
            Ok(())
        }

        /// Brings the platform menu in line with `model`.
        ///
        /// Labels and check marks are written in place while the structure
        /// matches; a structural change, such as a device appearing, falls
        /// back to a rebuild.
        pub fn sync(&mut self, model: &[Menu]) {
            if self.model == model {
                return;
            }
            if structure_of(&self.model) != structure_of(model) {
                let _ = self.build(model);
                return;
            }
            let mut entries = self.entries.iter();
            for menu in model {
                if let Some(Entry::Submenu(submenu)) = entries.next() {
                    submenu.set_text(&menu.label);
                }
                update_items(&mut entries, &menu.items);
            }
            self.model = model.to_vec();
        }

        /// Returns whatever the operator activated since the last frame.
        pub fn poll(&self) -> Vec<Action> {
            let mut actions = Vec::new();
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if let Some(action) = self.actions.get(&event.id) {
                    actions.push(action.clone());
                }
            }
            actions
        }
    }

    fn update_items<'a>(entries: &mut impl Iterator<Item = &'a Entry>, items: &[Item]) {
        for item in items {
            match (entries.next(), item) {
                (Some(Entry::Submenu(submenu)), Item::Submenu { label, items }) => {
                    submenu.set_text(label);
                    update_items(entries, items);
                }
                (Some(Entry::Check(entry)), Item::Check { label, checked, .. }) => {
                    entry.set_text(label);
                    entry.set_checked(*checked);
                }
                (Some(Entry::Command(entry)), Item::Command { label, .. })
                | (Some(Entry::Pending(entry)), Item::Pending(label)) => entry.set_text(label),
                _ => {}
            }
        }
    }

    /// Reduces a model to the shape a rebuild would be needed to change.
    fn structure_of(model: &[Menu]) -> Vec<Vec<Shape>> {
        model.iter().map(|menu| shapes(&menu.items)).collect()
    }

    #[derive(Eq, PartialEq)]
    enum Shape {
        Submenu(Vec<Shape>),
        Check,
        Command,
        Pending,
        Separator,
    }

    fn shapes(items: &[Item]) -> Vec<Shape> {
        items
            .iter()
            .map(|item| match item {
                Item::Submenu { items, .. } => Shape::Submenu(shapes(items)),
                Item::Check { .. } => Shape::Check,
                Item::Command { .. } => Shape::Command,
                Item::Pending(_) => Shape::Pending,
                Item::Separator => Shape::Separator,
            })
            .collect()
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod in_window {
    use super::{Action, Menu};

    /// A placeholder for the platforms muda cannot serve.
    ///
    /// The menu is drawn by [`super::bar`] instead; this type exists so the
    /// application does not need to know which path is in use.
    pub struct Native;

    impl Native {
        pub fn install(
            _cc: &eframe::CreationContext<'_>,
            _model: &[Menu],
        ) -> Result<Self, std::convert::Infallible> {
            Ok(Self)
        }

        pub fn sync(&mut self, _model: &[Menu]) {}

        pub fn poll(&self) -> Vec<Action> {
            Vec::new()
        }
    }
}

/// Whether the menu has to be drawn inside the window.
pub const fn is_in_window() -> bool {
    !cfg!(any(target_os = "windows", target_os = "macos"))
}

/// Draws the menu bar as egui widgets.
///
/// Used where the platform has no menu bar to attach to.
pub fn bar(ui: &mut egui::Ui, model: &[Menu]) -> Option<Action> {
    let mut activated = None;
    egui::MenuBar::new().ui(ui, |ui| {
        for menu in model {
            ui.menu_button(&menu.label, |ui| {
                if let Some(action) = items(ui, &menu.items) {
                    activated = Some(action);
                }
            });
        }
    });
    activated
}

fn items(ui: &mut egui::Ui, items: &[Item]) -> Option<Action> {
    let mut activated = None;
    for item in items {
        match item {
            Item::Submenu {
                label,
                items: nested,
            } => {
                ui.menu_button(label, |ui| {
                    if let Some(action) = self::items(ui, nested) {
                        activated = Some(action);
                    }
                });
            }
            Item::Check {
                label,
                checked,
                action,
            } => {
                let mut checked = *checked;
                if ui.checkbox(&mut checked, label).clicked() {
                    activated = Some(action.clone());
                    ui.close();
                }
            }
            Item::Command { label, action } => {
                if ui.button(label).clicked() {
                    activated = Some(action.clone());
                    ui.close();
                }
            }
            Item::Pending(label) => {
                ui.add_enabled(false, egui::Button::new(label));
            }
            Item::Separator => {
                ui.separator();
            }
        }
    }
    activated
}
