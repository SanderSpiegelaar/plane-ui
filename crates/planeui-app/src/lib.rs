//! PlaneUI's window, views and wiring. Domain logic lives in `planeui-core`.

mod binding_draft;
mod binding_form;
pub mod config_model;
pub mod debug_control;
pub mod fields;
mod gaps_view;
pub mod keybindings;
pub mod shell;
pub mod snapshots;
pub mod startup;
#[cfg(test)]
mod test_support;

use anyhow::Context as _;
use gpui_kit::component::Theme;
use gpui_kit::{
    App, AppContext as _, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, px,
    size,
};

use crate::shell::SECTIONS_CONTEXT;
use crate::startup::{MainWindow, load_session};

actions!(planeui, [Quit, Undo, Redo, PreviousSection, NextSection]);

/// Starts the application and opens the main window.
pub fn run() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        bind_keys(cx);

        if let Err(error) = open_main_window(cx) {
            log::error!("{error:#}");
            cx.quit();
        }
    });
}

/// Binds PlaneUI's shortcuts. Text inputs keep their own Cmd-Z for typing.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("up", PreviousSection, Some(SECTIONS_CONTEXT)),
        KeyBinding::new("down", NextSection, Some(SECTIONS_CONTEXT)),
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
