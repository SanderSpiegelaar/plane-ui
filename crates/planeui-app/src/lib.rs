//! PlaneUI's window, views and wiring. Domain logic lives in `planeui-core`.

mod binding_draft;
mod binding_form;
mod command_form;
pub mod config_model;
pub mod fields;
mod gaps_view;
mod general_view;
pub mod keybindings;
pub mod live_data;
mod monitor_picker;
mod rule_form;
mod rules_view;
pub mod shell;
pub mod snapshots;
pub mod startup;
#[cfg(test)]
mod test_support;
mod workspaces_view;

use anyhow::Context as _;
use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
use gpui_kit::component::{Theme, WindowExt as _};
use gpui_kit::{
    Action, App, AppContext as _, KeyBinding, Menu, MenuItem, TitlebarOptions, Window,
    WindowBounds, WindowOptions, actions, px, size,
};

use crate::shell::{SECTIONS_CONTEXT, Section};
use crate::startup::{MainWindow, load_session};

actions!(
    planeui,
    [
        About,
        Quit,
        Minimize,
        Zoom,
        Undo,
        Redo,
        PreviousSection,
        NextSection
    ]
);

/// Shows a sidebar section; bound to Cmd-1 through Cmd-6 in sidebar order.
#[derive(Debug, Clone, PartialEq, Action)]
#[action(namespace = planeui, no_json)]
pub struct ShowSection(pub Section);

/// Starts the application and opens the main window.
pub fn run() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &About, cx| in_main_window(cx, show_about));
        cx.on_action(|_: &Minimize, cx| in_main_window(cx, |window, _| window.minimize_window()));
        cx.on_action(|_: &Zoom, cx| in_main_window(cx, |window, _| window.zoom_window()));
        bind_keys(cx);
        cx.set_menus(menus());

        if let Err(error) = open_main_window(cx) {
            log::error!("{error:#}");
            cx.quit();
        }
    });
}

/// The menu bar. Undo and Redo dispatch the same actions as Cmd-Z and Cmd-Shift-Z, which the
/// shell hands to a focused text field so the menu behaves exactly like the keys. There is no
/// Save: every valid change is already written.
fn menus() -> Vec<Menu> {
    vec![
        Menu::new("PlaneUI").items([
            MenuItem::action("About PlaneUI", About),
            MenuItem::separator(),
            MenuItem::action("Quit PlaneUI", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::action("Undo", Undo),
            MenuItem::action("Redo", Redo),
            MenuItem::separator(),
            MenuItem::action("Cut", Cut),
            MenuItem::action("Copy", Copy),
            MenuItem::action("Paste", Paste),
            MenuItem::action("Select All", SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
    ]
}

/// PlaneUI has one window, so a menu action still reaches it while another app is active.
fn in_main_window(cx: &mut App, action: impl FnOnce(&mut Window, &mut App)) {
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        return;
    };
    if let Err(error) = window.update(cx, |_, window, cx| action(window, cx)) {
        log::debug!("the active window closed before a menu action ran: {error}");
    }
}

fn show_about(window: &mut Window, cx: &mut App) {
    window.open_alert_dialog(cx, |alert, _, _| {
        alert
            .title("PlaneUI")
            .description(format!("Version {}", env!("CARGO_PKG_VERSION")))
    });
}

/// Binds PlaneUI's shortcuts. Text inputs keep their own Cmd-Z for typing.
pub fn bind_keys(cx: &mut App) {
    let section_keys: [&str; Section::ALL.len()] =
        ["cmd-1", "cmd-2", "cmd-3", "cmd-4", "cmd-5", "cmd-6"];
    cx.bind_keys(
        Section::ALL
            .into_iter()
            .zip(section_keys)
            .map(|(section, keys)| KeyBinding::new(keys, ShowSection(section), None)),
    );
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("up", PreviousSection, Some(SECTIONS_CONTEXT)),
        KeyBinding::new("down", NextSection, Some(SECTIONS_CONTEXT)),
        KeyBinding::new(
            "cmd-alt-up",
            rules_view::MoveRuleUp,
            Some(rules_view::RULE_CONTEXT),
        ),
        KeyBinding::new(
            "cmd-alt-down",
            rules_view::MoveRuleDown,
            Some(rules_view::RULE_CONTEXT),
        ),
        KeyBinding::new(
            "enter",
            binding_form::CommitBinding,
            Some(binding_form::FORM_CONTEXT),
        ),
        KeyBinding::new(
            "escape",
            binding_form::CancelBinding,
            Some(binding_form::FORM_CONTEXT),
        ),
    ]);
}

fn open_main_window(cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("PlaneUI".into()),
            ..TitlebarOptions::default()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(960.), px(640.)), cx)),
        is_resizable: true,
        ..WindowOptions::default()
    };
    let (window, _view) = gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|cx| MainWindow::new(load_session, window, cx))
    })
    .context("failed to open the PlaneUI window")?;
    window
        .update(cx, |_, window, _| window.activate_window())
        .context("failed to activate the PlaneUI window")?;
    cx.activate(true);
    Ok(())
}
