use iced::widget::{
    Space, button, canvas, checkbox, column, container, pick_list, progress_bar, row, scrollable,
    stack, text, text_input, toggler,
};
use iced::{Alignment, Element, Length};

use crate::app::{
    App, Dsp, Entry, LibraryMessage, Message, ModeChoice, QsoMessage, RxMessage, Tab, TxMessage,
};
use crate::canvas::ImageCanvas;
use crate::i18n::{Locale, number, text as arg};

const SIDE_PANEL_WIDTH: f32 = 320.0;
const LIBRARY_HEIGHT: f32 = 246.0;
const LIST_WIDTH: f32 = 236.0;

fn filler() -> Space {
    Space::new().width(Length::Fill)
}

pub fn view(app: &App) -> Element<'_, Message> {
    column![
        menu_bar(app),
        toolbar(app),
        row![main_pane(app), side_panel(app)].height(Length::Fill),
        library(app),
        status_bar(app),
    ]
    .into()
}

fn menu_bar(app: &App) -> Element<'_, Message> {
    let entries = [
        "menu-file",
        "menu-edit",
        "menu-view",
        "menu-settings",
        "menu-rig",
        "menu-help",
    ];
    let mut bar = row![].spacing(2).padding(4);
    for key in entries {
        bar = bar.push(button(text(app.i18n.text(key)).size(13)).style(button::text));
    }
    bar.into()
}

fn toolbar(app: &App) -> Element<'_, Message> {
    let mut tabs = row![].spacing(2);
    for tab in Tab::ALL {
        let style = if tab == app.tab {
            button::primary
        } else {
            button::text
        };
        tabs = tabs.push(
            button(text(app.i18n.text(tab.label_key())).size(13))
                .style(style)
                .on_press(Message::TabSelected(tab)),
        );
    }
    row![
        tabs,
        filler(),
        text(app.i18n.text("input-device")).size(12),
        pick_list(
            app.audio.devices.as_slice(),
            app.audio.device.as_ref(),
            Message::DeviceSelected
        ),
        pick_list(
            Locale::ALL.as_slice(),
            Some(app.i18n.locale()),
            Message::LocaleSelected
        ),
    ]
    .spacing(8)
    .padding(8)
    .align_y(Alignment::Center)
    .into()
}

fn main_pane(app: &App) -> Element<'_, Message> {
    let viewport = stack![
        canvas(ImageCanvas::new(
            &app.main_cache,
            app.active_raster(),
            app.decoded_fraction(),
        ))
        .width(Length::Fill)
        .height(Length::Fill),
        container(text(badge(app)).size(11))
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill),
    ];
    column![
        container(viewport).width(Length::Fill).height(Length::Fill),
        action_bar(app),
    ]
    .spacing(10)
    .padding(14)
    .width(Length::Fill)
    .into()
}

fn badge(app: &App) -> String {
    let raster = app.active_raster();
    let mode = arg(raster.mode().spec().name());
    match app.tab {
        Tab::Receive => {
            let percent = (app.simulation.decoded_fraction * 100.0).round();
            app.i18n.text_with(
                "badge-receiving",
                &[("mode", mode), ("percent", number(percent))],
            )
        }
        Tab::Transmit => app
            .i18n
            .text_with("badge-transmit-ready", &[("mode", mode)]),
        Tab::History => app.i18n.text_with("badge-history", &[("mode", mode)]),
    }
}

fn geometry_label(app: &App) -> String {
    let raster = app.active_raster();
    app.i18n.text_with(
        "geometry",
        &[
            ("mode", arg(raster.mode().spec().name())),
            ("width", number(raster.size().width() as u32)),
            ("height", number(raster.size().height() as u32)),
        ],
    )
}

fn action_bar(app: &App) -> Element<'_, Message> {
    let bar = match app.tab {
        Tab::Receive => row![
            pending(app, "action-lock"),
            pending(app, "action-resync"),
            checkbox(app.auto_history)
                .label(app.i18n.text("action-auto-history"))
                .text_size(12)
                .on_toggle(|value| Message::Rx(RxMessage::AutoHistoryToggled(value))),
        ],
        Tab::Transmit => row![
            button(text(app.i18n.text("action-transmit")).size(12)).style(button::danger),
            pending(app, "action-tone"),
            pending(app, "action-cw"),
            pending(app, "action-fskid"),
        ],
        Tab::History => row![pending(app, "action-save"), pending(app, "action-copy")],
    };
    let trailing = match app.tab {
        Tab::Receive => row![pending(app, "action-save"), pending(app, "action-zoom")],
        Tab::Transmit => row![pending(app, "action-paste"), pending(app, "action-zoom")],
        Tab::History => row![pending(app, "action-zoom")],
    };
    row![
        bar.spacing(8).align_y(Alignment::Center),
        filler(),
        text(geometry_label(app)).size(12),
        trailing.spacing(6),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// A control whose behavior arrives with the audio boundary.
///
/// It is rendered disabled rather than wired to a placeholder action so the
/// layout is reviewable without implying working transmit or receive.
fn pending<'a>(app: &App, key: &str) -> Element<'a, Message> {
    button(text(app.i18n.text(key)).size(12))
        .style(button::secondary)
        .into()
}

fn side_panel(app: &App) -> Element<'_, Message> {
    column![
        section(app, "section-rx-status", rx_status(app)),
        section(app, "section-mode", mode_panel(app)),
        section(app, "section-dsp", dsp_panel(app)),
        section(app, "section-qso", qso_panel(app)),
    ]
    .spacing(16)
    .padding(14)
    .width(SIDE_PANEL_WIDTH)
    .into()
}

fn section<'a>(app: &App, key: &str, content: Element<'a, Message>) -> Element<'a, Message> {
    column![
        text(app.i18n.text(key)).size(11),
        container(content)
            .padding(12)
            .style(container::bordered_box),
    ]
    .spacing(8)
    .into()
}

fn rx_status(app: &App) -> Element<'_, Message> {
    let receiving = app.tab == Tab::Receive && app.audio.is_capturing();
    let sync_key = if receiving {
        "label-signal-detected"
    } else {
        "label-no-signal"
    };
    let percent = (app.simulation.sync_strength * 100.0).round();
    column![
        row![
            text(app.i18n.text("label-input-level")).size(12),
            filler(),
            text(format!("{:.0} dBFS", level_dbfs(app.audio.level()))).size(11),
        ],
        progress_bar(0.0..=1.0, app.audio.level()),
        row![
            text(app.i18n.text(sync_key)).size(12),
            filler(),
            text(
                app.i18n
                    .text_with("label-sync", &[("percent", number(percent))])
            )
            .size(11),
        ],
    ]
    .spacing(8)
    .into()
}

fn level_dbfs(level: f32) -> f32 {
    if level <= 0.0 {
        return -60.0;
    }
    20.0 * level.log10()
}

fn mode_panel(app: &App) -> Element<'_, Message> {
    let (selected, options, hint) = match app.tab {
        Tab::Transmit => (
            app.tx_mode,
            app.tx_modes.as_slice(),
            app.i18n.text("hint-tx-mode"),
        ),
        Tab::Receive | Tab::History => (
            app.rx_mode,
            app.rx_modes.as_slice(),
            app.i18n.text("hint-auto-mode"),
        ),
    };
    let on_select: fn(ModeChoice) -> Message = match app.tab {
        Tab::Transmit => |mode| Message::Tx(TxMessage::ModeSelected(mode)),
        Tab::Receive | Tab::History => |mode| Message::Rx(RxMessage::ModeSelected(mode)),
    };
    let dropdown = pick_list(options, Some(selected), on_select);
    let mut panel = column![].spacing(10);
    if app.tab != Tab::Transmit {
        panel = panel.push(
            toggler(app.auto_mode)
                .label(app.i18n.text("label-auto-vis"))
                .text_size(13)
                .on_toggle(|value| Message::Rx(RxMessage::AutoModeToggled(value))),
        );
    }
    panel.push(dropdown).push(text(hint).size(11)).into()
}

fn dsp_panel(app: &App) -> Element<'_, Message> {
    let mut panel = row![].spacing(8);
    for (dsp, key) in [
        (Dsp::Afc, "dsp-afc"),
        (Dsp::Lms, "dsp-lms"),
        (Dsp::Slant, "dsp-slant"),
    ] {
        let style = if app.dsp.get(dsp) {
            button::primary
        } else {
            button::secondary
        };
        panel = panel.push(
            button(text(app.i18n.text(key)).size(12).center())
                .width(Length::Fill)
                .style(style)
                .on_press(Message::Rx(RxMessage::DspToggled(dsp))),
        );
    }
    panel.into()
}

fn qso_panel(app: &App) -> Element<'_, Message> {
    column![
        row![
            text(app.i18n.text("qso-call")).size(12).width(62.0),
            text_input("", &app.qso.call)
                .size(13)
                .on_input(|value| Message::Qso(QsoMessage::CallChanged(value))),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            text(app.i18n.text("qso-rsv")).size(12).width(62.0),
            text_input("", &app.qso.rsv)
                .size(13)
                .on_input(|value| Message::Qso(QsoMessage::RsvChanged(value))),
            text(app.i18n.text("qso-nr")).size(12),
            text_input("", &app.qso.number)
                .size(13)
                .width(74.0)
                .on_input(|value| Message::Qso(QsoMessage::NumberChanged(value))),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            pending(app, "qso-record"),
            button(text(app.i18n.text("qso-clear")).size(12))
                .style(button::secondary)
                .on_press(Message::Qso(QsoMessage::Cleared)),
        ]
        .spacing(6),
    ]
    .spacing(8)
    .into()
}

fn library(app: &App) -> Element<'_, Message> {
    row![
        entry_list(
            app,
            "section-templates",
            &app.templates,
            app.template,
            LibraryMessage::TemplateSelected,
        ),
        entry_list(
            app,
            "section-stocks",
            &app.stocks,
            app.stock,
            LibraryMessage::StockSelected,
        ),
        composite(app),
    ]
    .spacing(12)
    .padding(14)
    .height(LIBRARY_HEIGHT)
    .into()
}

fn entry_list<'a>(
    app: &'a App,
    key: &str,
    entries: &'a [Entry],
    selected: usize,
    on_select: fn(usize) -> LibraryMessage,
) -> Element<'a, Message> {
    let mut list = column![].spacing(2);
    for (index, entry) in entries.iter().enumerate() {
        let style = if index == selected {
            button::primary
        } else {
            button::text
        };
        list = list.push(
            button(
                row![
                    text(entry.name.as_str()).size(12),
                    filler(),
                    text(entry.geometry.as_str()).size(10),
                ]
                .spacing(8),
            )
            .width(Length::Fill)
            .style(style)
            .on_press(Message::Library(on_select(index))),
        );
    }
    column![
        text(app.i18n.text(key)).size(11),
        container(scrollable(list))
            .height(Length::Fill)
            .style(container::bordered_box),
    ]
    .spacing(8)
    .width(LIST_WIDTH)
    .into()
}

fn composite(app: &App) -> Element<'_, Message> {
    column![
        row![
            text(app.i18n.text("section-composite")).size(11),
            filler(),
            pending(app, "action-edit"),
            pending(app, "action-set-transmit"),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        container(
            canvas(ImageCanvas::new(&app.preview_cache, &app.tx_raster, 1.0))
                .width(Length::Fill)
                .height(Length::Fill)
        )
        .height(Length::Fill),
    ]
    .spacing(8)
    .width(Length::Fill)
    .into()
}

fn status_bar(app: &App) -> Element<'_, Message> {
    let percent = (app.simulation.decoded_fraction * 100.0).round();
    let status = if app.tab == Tab::Receive {
        app.i18n
            .text_with("status-receiving", &[("percent", number(percent))])
    } else {
        app.i18n.text("status-idle")
    };
    let audio = match app.audio.sample_rate_hz() {
        Some(rate) => app
            .i18n
            .text_with("status-audio", &[("rate", number(rate))]),
        None => app.i18n.text("status-no-audio"),
    };
    let mut bar = row![
        text(status).size(11),
        text(audio).size(11),
        text(app.i18n.text("status-simulated")).size(11),
    ];
    let dropped = app.audio.dropped_samples();
    if dropped > 0 {
        bar = bar.push(
            text(
                app.i18n
                    .text_with("status-dropped", &[("samples", number(dropped as u32))]),
            )
            .size(11),
        );
    }
    if let Some(error) = &app.audio.error {
        bar = bar.push(text(error.as_str()).size(11));
    }
    bar.push(filler()).spacing(16).padding([4, 12]).into()
}
