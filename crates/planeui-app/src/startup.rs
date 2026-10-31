//! The main window's root view: finds and reads the config off the UI thread, then shows the
//! editor. A config that cannot be opened shows its error text; PUI-022 turns those into real
//! startup screens.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div,
};
use planeui_core::aerospace::{Aerospace, AerospaceVersion, CliAerospace, CliError, ReloadOutcome};
use planeui_core::apply::ApplyContext;
use planeui_core::store::{ConfigStore, Loaded, PathEnv, SnapshotStore, resolve_config_path};

use crate::config_model::ConfigModel;
use crate::debug_control::AccordionPaddingControl;
use crate::{Redo, Undo};

/// Everything the editor starts from.
#[derive(Debug)]
pub struct Session {
    /// The config path as the user knows it.
    pub display_path: PathBuf,
    /// Where writes, snapshots and reloads go.
    pub context: ApplyContext,
    /// The config as read at startup.
    pub loaded: Loaded,
}

/// Locates and reads the config the way AeroSpace does, using this process's environment.
/// Blocks on the file system and the `aerospace` CLI, so run it in the background.
///
/// # Errors
///
/// The config cannot be located (a [`planeui_core::store::LocateError`] can be downcast from
/// the error) or read, or there is no home or data directory.
pub fn load_session() -> anyhow::Result<Session> {
    let aerospace: Arc<dyn Aerospace> = match CliAerospace::locate() {
        Ok(cli) => Arc::new(cli),
        Err(error) => {
            log::warn!("{error}");
            Arc::new(MissingAerospace)
        }
    };
    let server_path = aerospace.config_path().unwrap_or_else(|error| {
        log::warn!("could not ask AeroSpace for its config path: {error}");
        None
    });
    let env = PathEnv::from_process().context("could not determine the home directory")?;
    let resolved = resolve_config_path(&env, server_path)?;
    let snapshot_dir =
        SnapshotStore::default_dir().context("could not determine the data directory")?;
    let store = ConfigStore::new(&resolved);
    let loaded = store.load()?;
    Ok(Session {
        display_path: resolved.display_path,
        context: ApplyContext {
            store,
            snapshots: SnapshotStore::new(snapshot_dir),
            aerospace,
        },
        loaded,
    })
}

/// Stands in for AeroSpace when its CLI is not installed: edits are still written, and every
/// call reports [`CliError::BinaryNotFound`].
#[derive(Debug)]
struct MissingAerospace;

impl Aerospace for MissingAerospace {
    fn version(&self) -> Result<AerospaceVersion, CliError> {
        Err(CliError::BinaryNotFound)
    }

    fn config_path(&self) -> Result<Option<PathBuf>, CliError> {
        Err(CliError::BinaryNotFound)
    }

    fn reload_config(&self) -> Result<ReloadOutcome, CliError> {
        Err(CliError::BinaryNotFound)
    }
}

#[derive(Debug)]
enum Screen {
    Loading,
    Failed(SharedString),
    Ready {
        model: Entity<ConfigModel>,
        control: Entity<AccordionPaddingControl>,
    },
}

/// The main window's root view.
#[derive(Debug)]
pub struct MainWindow {
    screen: Screen,
    focus_handle: FocusHandle,
    _load: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    /// Shows a loading state while `load` runs in the background, then the editor or the
    /// reason it cannot open.
    pub fn new(
        load: impl FnOnce() -> anyhow::Result<Session> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let appearance = window.observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        // Undo and Redo dispatch from the focused element, so the root takes focus until a
        // control does.
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let load = cx.spawn_in(window, async move |this, cx| {
            let session = cx.background_spawn(async move { load() }).await;
            if let Err(error) =
                this.update_in(cx, |this, window, cx| this.open(session, window, cx))
            {
                log::debug!("the main window closed while loading: {error}");
            }
        });
        Self {
            screen: Screen::Loading,
            focus_handle,
            _load: load,
            _subscriptions: vec![appearance],
        }
    }

    fn open(
        &mut self,
        session: anyhow::Result<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.screen = match session.and_then(|session| {
            ConfigModel::open(session.display_path, session.context, session.loaded)
                .context("the config is not valid TOML")
        }) {
            Ok(model) => {
                let model = cx.new(|_| model);
                model.update(cx, ConfigModel::refresh_aerospace);
                let control = cx.new(|cx| AccordionPaddingControl::new(model.clone(), window, cx));
                Screen::Ready { model, control }
            }
            Err(error) => {
                log::error!("{error:#}");
                Screen::Failed(format!("{error:#}").into())
            }
        };
        cx.notify();
    }

    fn update_model(
        &self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut ConfigModel, &mut Context<ConfigModel>),
    ) {
        if let Screen::Ready { model, .. } = &self.screen {
            model.update(cx, update);
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let content = match &self.screen {
            Screen::Loading => div()
                .text_color(theme.muted_foreground)
                .child("Loading the AeroSpace config…"),
            Screen::Failed(message) => div()
                .flex()
                .flex_col()
                .gap_2()
                .max_w_128()
                .child("PlaneUI cannot open the config")
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(message.clone()),
                ),
            Screen::Ready { control, .. } => div().child(control.clone()),
        };
        div()
            .track_focus(&self.focus_handle)
            .on_action(
                cx.listener(|this, _: &Undo, _, cx| this.update_model(cx, ConfigModel::undo)),
            )
            .on_action(
                cx.listener(|this, _: &Redo, _, cx| this.update_model(cx, ConfigModel::redo)),
            )
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(content)
    }
}
