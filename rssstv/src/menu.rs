//! The application menu, described once and rendered per platform.
//!
//! muda can only attach a native menu to a window the platform owns, which
//! rules out Linux: it needs a gtk window there and winit does not create one.
//! Rather than maintain two menu definitions, the menu is built as a
//! platform-independent [model](model) that the native and in-window renderers
//! both consume, so the two paths cannot drift apart.

use crate::{app::App, i18n::Locale, paths::Folder};

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use native::Native;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use in_window::Native;

/// What activating a menu entry asks the application to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SelectDevice(String),
    SelectOutputDevice(String),
    SelectLocale(Locale),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Reveal(Folder),
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
            items: folder_items(app),
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
            items: vec![
                Item::Command {
                    label: text("menu-zoom-in"),
                    action: Action::ZoomIn,
                },
                Item::Command {
                    label: text("menu-zoom-out"),
                    action: Action::ZoomOut,
                },
                Item::Command {
                    label: app.i18n.text_with(
                        "menu-zoom-reset",
                        &[("percent", crate::i18n::number(ui_scale_percent(app)))],
                    ),
                    action: Action::ZoomReset,
                },
            ],
        },
        Menu {
            label: text("menu-settings"),
            items: vec![
                Item::Submenu {
                    label: text("input-device"),
                    items: device_items(app),
                },
                Item::Submenu {
                    label: text("output-device"),
                    items: output_device_items(app),
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

/// The File menu: every directory the application keeps, and then Quit.
///
/// The application stores nothing of its own, so opening a folder is the whole
/// of what File has to offer; the entries are built from [`Folder::ALL`] so a
/// new directory cannot be added without also being reachable.
fn folder_items(app: &App) -> Vec<Item> {
    Folder::ALL
        .into_iter()
        .map(|folder| Item::Command {
            label: app.i18n.text(folder.label_key()),
            action: Action::Reveal(folder),
        })
        .chain([
            Item::Separator,
            Item::Command {
                label: app.i18n.text("menu-quit"),
                action: Action::Quit,
            },
        ])
        .collect()
}

fn ui_scale_percent(app: &App) -> f32 {
    (app.ui_scale * 100.0).round()
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

fn output_device_items(app: &App) -> Vec<Item> {
    if app.audio.output_devices.is_empty() {
        return vec![Item::Pending(app.i18n.text("status-no-output"))];
    }
    let selected = app.audio.output_device.as_ref().map(|device| device.name());
    app.audio
        .output_devices
        .iter()
        .map(|device| Item::Check {
            label: device.name().to_owned(),
            checked: selected == Some(device.name()),
            action: Action::SelectOutputDevice(device.name().to_owned()),
        })
        .collect()
}

/// Applies `action` to the application.
///
/// Returns whether the application was asked to close.
pub fn apply(app: &mut App, action: Action) -> bool {
    match action {
        Action::SelectDevice(name) => app.select_device_named(&name),
        Action::SelectOutputDevice(name) => app.select_output_device_named(&name),
        Action::SelectLocale(locale) => app.select_locale(locale),
        Action::ZoomIn => app.zoom_by(ZOOM_STEP),
        Action::ZoomOut => app.zoom_by(-ZOOM_STEP),
        Action::ZoomReset => app.set_ui_scale(crate::config::DEFAULT_UI_SCALE),
        Action::Reveal(folder) => app.reveal(folder),
        Action::Quit => return true,
    }
    false
}

/// Matches the step egui's own zoom shortcuts take.
const ZOOM_STEP: f32 = 0.1;

/// Every item of every menu, in the order a renderer creates them.
///
/// Building the platform menu and updating it later both walk this, so the
/// two cannot disagree about which entry corresponds to which item. They did
/// once: separators were created but not counted, which shifted every later
/// label onto the wrong entry.
///
/// The in-window bar draws straight from the model, so this is built only
/// where the native menu is, and for the tests that cover it everywhere.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
pub fn flatten(menus: &[Menu]) -> Vec<&Item> {
    fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
        for item in items {
            out.push(item);
            if let Item::Submenu { items, .. } = item {
                walk(items, out);
            }
        }
    }

    let mut out = Vec::new();
    for menu in menus {
        walk(&menu.items, &mut out);
    }
    out
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
        /// The top-level menus, in bar order.
        bar: Vec<Submenu>,
        /// Every entry below the bar, in [`super::flatten`] order.
        ///
        /// Held apart from `bar` so each vector lines up with one sequence
        /// from the model. Interleaving the two is what once let the update
        /// walk drift and write labels onto the wrong entries.
        items: Vec<Entry>,
        actions: HashMap<MenuId, Action>,
        model: Vec<Menu>,
        #[cfg(target_os = "windows")]
        hwnd: Option<isize>,
    }

    /// One created menu entry, positionally matched to a model item.
    enum Entry {
        Check(CheckMenuItem),
        Command(MenuItem),
        Pending(MenuItem),
        Submenu(Submenu),
        Separator,
    }

    impl Entry {
        fn update(&self, item: &Item) {
            match (self, item) {
                (Self::Submenu(entry), Item::Submenu { label, .. }) => entry.set_text(label),
                (Self::Check(entry), Item::Check { label, checked, .. }) => {
                    entry.set_text(label);
                    entry.set_checked(*checked);
                }
                (Self::Command(entry), Item::Command { label, .. })
                | (Self::Pending(entry), Item::Pending(label)) => entry.set_text(label),
                (Self::Separator, Item::Separator) => {}
                _ => debug_assert!(false, "menu entries drifted from the model"),
            }
        }
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
                bar: Vec::new(),
                items: Vec::new(),
                actions: HashMap::new(),
                model: Vec::new(),
                #[cfg(target_os = "windows")]
                hwnd: None,
            };
            native.build(model)?;
            native.attach(cc)?;
            Ok(native)
        }

        #[cfg(target_os = "windows")]
        fn attach(&mut self, cc: &eframe::CreationContext<'_>) -> Result<(), muda::Error> {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

            let Ok(handle) = cc.window_handle() else {
                return Ok(());
            };
            if let RawWindowHandle::Win32(window) = handle.as_raw() {
                // muda installs its own window subclass on this handle and
                // answers WM_COMMAND itself, so the winit event loop needs no
                // cooperation for menu clicks.
                let hwnd = window.hwnd.get();
                unsafe { self.menu.init_for_hwnd(hwnd) }?;
                self.hwnd = Some(hwnd);
            }
            Ok(())
        }

        #[cfg(target_os = "macos")]
        fn attach(&mut self, _cc: &eframe::CreationContext<'_>) -> Result<(), muda::Error> {
            self.menu.init_for_nsapp();
            Ok(())
        }

        #[cfg(target_os = "windows")]
        pub fn prepare_for_close(&self) {
            use windows_sys::Win32::{
                Foundation::HWND,
                UI::WindowsAndMessaging::{SW_HIDE, ShowWindow},
            };

            if let Some(hwnd) = self.hwnd {
                unsafe { ShowWindow(hwnd as HWND, SW_HIDE) };
            }
        }

        #[cfg(target_os = "macos")]
        pub fn prepare_for_close(&self) {}

        /// Replaces every menu entry from `model`.
        fn build(&mut self, model: &[Menu]) -> Result<(), muda::Error> {
            while self.menu.remove_at(0).is_some() {}
            self.bar.clear();
            self.items.clear();
            self.actions.clear();

            for menu in model {
                let submenu = Submenu::new(&menu.label, true);
                self.menu.append(&submenu)?;
                self.bar.push(submenu.clone());
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
                        self.items.push(Entry::Submenu(submenu.clone()));
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
                        self.items.push(Entry::Check(entry));
                    }
                    Item::Command { label, action } => {
                        let entry = MenuItem::new(label, true, None);
                        parent.append(&entry)?;
                        self.actions.insert(entry.id().clone(), action.clone());
                        self.items.push(Entry::Command(entry));
                    }
                    Item::Pending(label) => {
                        let entry = MenuItem::new(label, false, None);
                        parent.append(&entry)?;
                        self.items.push(Entry::Pending(entry));
                    }
                    Item::Separator => {
                        parent.append(&PredefinedMenuItem::separator())?;
                        self.items.push(Entry::Separator);
                    }
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
            for (submenu, menu) in self.bar.iter().zip(model) {
                submenu.set_text(&menu.label);
            }
            for (entry, item) in self.items.iter().zip(super::flatten(model)) {
                entry.update(item);
            }
            self.model = model.to_vec();
        }

        /// Builds the menu without attaching it to a window, for tests.
        #[cfg(test)]
        pub fn detached(model: &[Menu]) -> Self {
            let mut native = Self {
                menu: muda::Menu::new(),
                bar: Vec::new(),
                items: Vec::new(),
                actions: HashMap::new(),
                model: Vec::new(),
                #[cfg(target_os = "windows")]
                hwnd: None,
            };
            native.build(model).expect("a detached menu can be built");
            native
        }

        /// Returns the label each entry is currently showing, in model order.
        #[cfg(test)]
        pub fn labels(&self) -> Vec<String> {
            let bar = self.bar.iter().map(Submenu::text);
            let items = self.items.iter().map(|entry| match entry {
                Entry::Check(entry) => entry.text(),
                Entry::Command(entry) | Entry::Pending(entry) => entry.text(),
                Entry::Submenu(entry) => entry.text(),
                Entry::Separator => "-".to_owned(),
            });
            bar.chain(items).collect()
        }

        /// Returns whatever the operator activated since the last frame.
        pub fn poll(&self) -> Vec<Action> {
            let mut actions = Vec::new();
            let mut activated = false;
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                activated = true;
                if let Some(action) = self.actions.get(&event.id) {
                    actions.push(action.clone());
                }
            }
            if activated {
                self.restore_checks();
            }
            actions
        }

        /// Rewrites the check marks the platform flipped on its own.
        ///
        /// A check entry toggles itself when it is activated, so choosing the
        /// device or the language that is already selected clears its mark
        /// even though the selection did not change. The next [`Self::sync`]
        /// cannot put it back: an unchanged selection produces the model that
        /// is already applied, which is exactly the case sync skips. The model
        /// is the authority, so the marks are written back from it here.
        pub(super) fn restore_checks(&self) {
            for (entry, item) in self.items.iter().zip(super::flatten(&self.model)) {
                if let (Entry::Check(entry), Item::Check { checked, .. }) = (entry, item) {
                    entry.set_checked(*checked);
                }
            }
        }

        /// Flips every check mark, as the platform does when one is activated.
        #[cfg(test)]
        pub fn flip_checks(&self) {
            for entry in &self.items {
                if let Entry::Check(entry) = entry {
                    entry.set_checked(!entry.is_checked());
                }
            }
        }

        /// Returns the state of each check entry, in model order.
        #[cfg(test)]
        pub fn checks(&self) -> Vec<bool> {
            self.items
                .iter()
                .filter_map(|entry| match entry {
                    Entry::Check(entry) => Some(entry.is_checked()),
                    _ => None,
                })
                .collect()
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

        pub fn prepare_for_close(&self) {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn count(items: &[Item]) -> usize {
        items
            .iter()
            .map(|item| match item {
                Item::Submenu { items, .. } => 1 + count(items),
                _ => 1,
            })
            .sum()
    }

    /// The platform renderer creates one entry per item and later matches them
    /// by position, so a missed item shifts every later label onto the wrong
    /// entry. Separators used to be skipped, which corrupted the submenus of
    /// every menu after the first separator.
    #[test]
    fn flattening_counts_every_item_including_separators() {
        let model = model(&App::headless());
        let expected: usize = model.iter().map(|menu| count(&menu.items)).sum();
        assert_eq!(flatten(&model).len(), expected);
        assert!(
            model
                .iter()
                .any(|menu| menu.items.contains(&Item::Separator)),
            "the model needs a separator for this to be worth asserting"
        );
    }

    #[test]
    fn flattening_visits_a_submenu_before_its_contents() {
        let model = vec![Menu {
            label: "settings".to_owned(),
            items: vec![
                Item::Separator,
                Item::Submenu {
                    label: "outer".to_owned(),
                    items: vec![Item::Pending("inner".to_owned())],
                },
                Item::Pending("after".to_owned()),
            ],
        }];
        let flat = flatten(&model);
        assert!(matches!(flat[0], Item::Separator));
        assert!(matches!(flat[1], Item::Submenu { .. }));
        assert!(matches!(flat[2], Item::Pending(label) if label == "inner"));
        assert!(matches!(flat[3], Item::Pending(label) if label == "after"));
    }

    /// Every directory the application keeps is reachable from one menu, or a
    /// folder the operator is expected to work in has no way in.
    #[test]
    fn the_file_menu_offers_every_folder() {
        let app = App::headless();
        let model = model(&app);
        let offered: Vec<Folder> = model[0]
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Command {
                    action: Action::Reveal(folder),
                    ..
                } => Some(*folder),
                _ => None,
            })
            .collect();

        assert_eq!(offered, Folder::ALL);
        for folder in Folder::ALL {
            let key = folder.label_key();
            assert_ne!(app.i18n.text(key), key, "{key} is not translated");
        }
    }

    /// Every action the model offers has to be handled, or a menu entry does
    /// nothing when clicked.
    #[test]
    fn every_action_in_the_model_is_applicable() {
        let mut app = App::headless();
        for item in flatten(&model(&App::headless())) {
            let action = match item {
                Item::Check { action, .. } | Item::Command { action, .. } => action.clone(),
                _ => continue,
            };
            let quits = matches!(action, Action::Quit);
            assert_eq!(apply(&mut app, action), quits);
        }
    }
}

/// Exercises the platform renderer itself, where the drift actually happened.
#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
mod native_tests {
    use super::*;
    use crate::{app::App, i18n::Locale};

    /// The labels a menu should be showing, in the order [`Native::labels`]
    /// reports them.
    fn expected(model: &[Menu]) -> Vec<String> {
        let bar = model.iter().map(|menu| menu.label.clone());
        let items = flatten(model).into_iter().map(|item| match item {
            Item::Submenu { label, .. }
            | Item::Check { label, .. }
            | Item::Command { label, .. }
            | Item::Pending(label) => label.clone(),
            Item::Separator => "-".to_owned(),
        });
        bar.chain(items).collect()
    }

    /// The check marks a menu should be showing, in [`Native::checks`] order.
    fn expected_checks(model: &[Menu]) -> Vec<bool> {
        flatten(model)
            .into_iter()
            .filter_map(|item| match item {
                Item::Check { checked, .. } => Some(*checked),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_freshly_built_menu_shows_the_model() {
        let model = model(&App::headless());
        assert_eq!(Native::detached(&model).labels(), expected(&model));
    }

    /// Activating a check entry flips its mark, so choosing the device or the
    /// language that is already selected clears it. The selection did not
    /// change, which leaves the model equal to the applied one and `sync` with
    /// nothing to do, so the mark has to be written back where the activation
    /// was noticed.
    #[test]
    fn choosing_the_selected_entry_again_keeps_its_mark() {
        let mut app = App::headless();
        let english = model(&app);
        let mut native = Native::detached(&english);
        assert!(
            expected_checks(&english).contains(&true),
            "a selected entry is needed for this to be worth asserting"
        );

        native.flip_checks();
        assert_ne!(
            native.checks(),
            expected_checks(&english),
            "the platform's own toggle is what has to be undone"
        );
        native.restore_checks();
        assert_eq!(native.checks(), expected_checks(&english));

        // The mark still has to follow a selection that did change.
        app.select_locale(Locale::Ja);
        let switched = model(&app);
        native.sync(&switched);
        assert_eq!(native.checks(), expected_checks(&switched));
    }

    /// Switching the language relabels every entry in place. Getting this
    /// wrong wrote each label onto its neighbour, which destroyed the
    /// submenus rather than merely mislabelling the bar.
    #[test]
    fn relabelling_lands_on_the_right_entries() {
        let mut app = App::headless();
        let english = model(&app);
        let mut native = Native::detached(&english);

        app.select_locale(Locale::Ja);
        let japanese = model(&app);
        native.sync(&japanese);

        assert_ne!(expected(&english), expected(&japanese));
        assert_eq!(native.labels(), expected(&japanese));
    }

    #[test]
    fn repeated_syncs_do_not_accumulate_drift() {
        let mut app = App::headless();
        let mut native = Native::detached(&model(&app));
        for locale in [Locale::Ja, Locale::En, Locale::Ja, Locale::En] {
            app.select_locale(locale);
            native.sync(&model(&app));
        }
        // The zoom label carries a percentage, so a scale change relabels one
        // entry without touching the structure.
        app.zoom_by(0.5);
        let scaled = model(&app);
        native.sync(&scaled);
        assert_eq!(native.labels(), expected(&scaled));
    }
}
