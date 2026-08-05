//! The application menu, described once and rendered per platform.
//!
//! muda can only attach a native menu to a window the platform owns, which
//! rules out Linux: it needs a gtk window there and winit does not create one.
//! Rather than maintain two menu definitions, the menu is built as a
//! platform-independent [model](model) that the native and in-window renderers
//! both consume, so the two paths cannot drift apart.

use crate::{
    app::App,
    i18n::Locale,
    storage::{history::HistoryFormat, paths::Folder},
};

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod in_window;

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
    ShowStation,
    ShowCustomVariables,
    ToggleSendFskid,
    ToggleVisRestart,
    WriteRigScript,
    WriteBandPlan,
    ToggleAutoHistory,
    SelectHistoryFormat(HistoryFormat),
    OpenManual,
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
    /// An entry there is nothing to activate on.
    ///
    /// Either a behavior that arrives with a later feature, or something the
    /// menu only has to say: an address, a state, a device list that is empty.
    /// Shown disabled rather than wired to a placeholder action, so the menu is
    /// reviewable without implying working commands.
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
                Item::Command {
                    label: text("menu-station"),
                    action: Action::ShowStation,
                },
                Item::Command {
                    label: text("menu-custom-variables"),
                    action: Action::ShowCustomVariables,
                },
                Item::Separator,
                Item::Submenu {
                    label: text("input-device"),
                    items: device_items(app),
                },
                Item::Submenu {
                    label: text("output-device"),
                    items: output_device_items(app),
                },
                Item::Submenu {
                    label: text("menu-transmit"),
                    items: vec![Item::Check {
                        label: text("action-send-fskid"),
                        checked: app.send_fskid,
                        action: Action::ToggleSendFskid,
                    }],
                },
                Item::Submenu {
                    label: text("menu-receive"),
                    items: vec![Item::Check {
                        label: text("action-vis-restart"),
                        checked: app.vis_restart,
                        action: Action::ToggleVisRestart,
                    }],
                },
                Item::Submenu {
                    label: text("menu-history"),
                    items: history_items(app),
                },
                // Rig control is worked from the radio panel; what is left for
                // a menu is putting the two files it runs on where they can be
                // edited, which is a once-ever thing rather than an operating
                // control.
                Item::Submenu {
                    label: text("menu-rig"),
                    items: vec![
                        Item::Command {
                            label: text("action-rig-write-script"),
                            action: Action::WriteRigScript,
                        },
                        Item::Command {
                            label: text("action-rig-write-bands"),
                            action: Action::WriteBandPlan,
                        },
                    ],
                },
                Item::Separator,
                Item::Submenu {
                    label: text("menu-language"),
                    items: locale_items(app),
                },
            ],
        },
        Menu {
            label: text("menu-help"),
            items: vec![Item::Command {
                label: text("menu-manual"),
                action: Action::OpenManual,
            }],
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

/// The received-image settings: whether to keep receptions, and in what.
fn history_items(app: &App) -> Vec<Item> {
    let mut items = vec![
        Item::Check {
            label: app.i18n.text("action-auto-history"),
            checked: app.auto_history,
            action: Action::ToggleAutoHistory,
        },
        Item::Separator,
    ];
    items.extend(HistoryFormat::ALL.into_iter().map(|format| Item::Check {
        label: app.i18n.text(format.label_key()),
        checked: app.history_format == format,
        action: Action::SelectHistoryFormat(format),
    }));
    items
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
        Action::ShowStation => app.station_open = true,
        Action::ShowCustomVariables => app.open_custom_variables(),
        Action::ToggleSendFskid => app.send_fskid = !app.send_fskid,
        Action::ToggleVisRestart => app.set_vis_restart(!app.vis_restart),
        Action::WriteRigScript => app.write_rig_script(),
        Action::WriteBandPlan => app.write_band_plan(),
        Action::ToggleAutoHistory => app.auto_history = !app.auto_history,
        Action::SelectHistoryFormat(format) => app.history_format = format,
        Action::OpenManual => app.open_manual(),
        Action::ZoomIn => app.zoom_by(ZOOM_STEP),
        Action::ZoomOut => app.zoom_by(-ZOOM_STEP),
        Action::ZoomReset => app.set_ui_scale(crate::storage::config::DEFAULT_UI_SCALE),
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

    /// A manual nothing opens is a manual nobody reads, so the Help menu has
    /// to carry a command rather than the placeholder it started as.
    #[test]
    fn the_help_menu_opens_the_manual() {
        let app = App::headless();
        let model = model(&app);
        let help = model.last().expect("the menu should end with Help");

        assert!(matches!(
            help.items.as_slice(),
            [Item::Command {
                action: Action::OpenManual,
                ..
            }]
        ));
        assert_ne!(app.i18n.text("menu-manual"), "menu-manual");
    }

    /// A build run from the source tree has no manual beside it. The entry
    /// still has to say so, because an entry that reports nothing is
    /// indistinguishable from one that is broken.
    #[test]
    fn a_missing_manual_is_reported() {
        let mut app = App::headless();

        apply(&mut app, Action::OpenManual);

        assert_eq!(
            app.library_error.as_deref(),
            Some(app.i18n.text("error-manual-missing").as_str())
        );
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
