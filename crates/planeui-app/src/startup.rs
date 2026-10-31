//! The main window's root view: finds and reads the config off the UI thread, then shows the
//! editor, or a screen for each way locating the config can fail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Theme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, TestSupportExt as _,
    Window, div,
};
use planeui_core::aerospace::{Aerospace, AerospaceVersion, CliAerospace, CliError, ReloadOutcome};
use planeui_core::apply::ApplyContext;
use planeui_core::prefs::{Prefs, PrefsStore};
use planeui_core::store::{
    ConfigStore, Loaded, LocateError, PathEnv, SnapshotStore, create_default_config,
    resolve_config_path,
};

use crate::config_model::ConfigModel;
use crate::shell::Shell;

/// Everything the editor starts from.
#[derive(Debug)]
pub struct Session {
    /// The config path as the user knows it.
    pub display_path: PathBuf,
    /// Where writes, snapshots and reloads go.
    pub context: ApplyContext,
    /// The config as read at startup.
    pub loaded: Loaded,
    /// Where PlaneUI's own preferences live.
    pub prefs_store: PrefsStore,
    /// The preferences as read at startup.
    pub prefs: Prefs,
}

/// Locates and reads the config the way AeroSpace does, using this process's environment.
/// Blocks on the file system and the `aerospace` CLI, so run it in the background.
///
/// # Errors
///
/// As [`open_session`], or there is no home or data directory.
pub fn load_session() -> anyhow::Result<Session> {
    let aerospace: Arc<dyn Aerospace> = match CliAerospace::locate() {
        Ok(cli) => Arc::new(cli),
        Err(error) => {
            log::warn!("{error}");
            Arc::new(MissingAerospace)
        }
    };
    let env = PathEnv::from_process().context("could not determine the home directory")?;
    let snapshot_dir =
        SnapshotStore::default_dir().context("could not determine the data directory")?;
    let prefs_path =
        PrefsStore::default_path().context("could not determine the data directory")?;
    open_session(
        &env,
        aerospace,
        SnapshotStore::new(snapshot_dir),
        PrefsStore::new(prefs_path),
    )
}

/// Locates the config under `env`, asking `aerospace` for the path its server uses, and reads
/// it and the preferences. Blocks, so run it in the background.
///
/// # Errors
///
/// The config cannot be located (a [`LocateError`] can be downcast from the error) or read.
pub fn open_session(
    env: &PathEnv,
    aerospace: Arc<dyn Aerospace>,
    snapshots: SnapshotStore,
    prefs_store: PrefsStore,
) -> anyhow::Result<Session> {
    let server_path = aerospace.config_path().unwrap_or_else(|error| {
        if !matches!(error, CliError::BinaryNotFound) {
            log::warn!("could not ask AeroSpace for its config path: {error}");
        }
        None
    });
    let resolved = resolve_config_path(env, server_path)?;
    let store = ConfigStore::new(&resolved);
    let loaded = store.load()?;
    // Preferences only hide offers, so unreadable ones must not keep the config from opening.
    let prefs = prefs_store.load().unwrap_or_else(|error| {
        log::warn!("{error}; using default preferences");
        Prefs::default()
    });
    Ok(Session {
        display_path: resolved.display_path,
        context: ApplyContext {
            store,
            snapshots,
            aerospace,
        },
        loaded,
        prefs_store,
        prefs,
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

type Loader = Arc<dyn Fn() -> anyhow::Result<Session> + Send + Sync>;

#[derive(Debug)]
enum Screen {
    Loading,
    /// Both default config files exist, so AeroSpace loads neither.
    Ambiguous {
        home: PathBuf,
        xdg: PathBuf,
    },
    /// No config exists; one can be created at `suggested`. `error` says why creating failed.
    NotFound {
        suggested: PathBuf,
        error: Option<SharedString>,
    },
    Failed(SharedString),
    Ready(Entity<Shell>),
}

impl Screen {
    fn after_failure(error: anyhow::Error) -> Self {
        match error.downcast::<LocateError>() {
            Ok(LocateError::Ambiguous { home, xdg }) => Self::Ambiguous { home, xdg },
            Ok(LocateError::NotFound { suggested }) => Self::NotFound {
                suggested,
                error: None,
            },
            Ok(error) => Self::Failed(error.to_string().into()),
            Err(error) => Self::Failed(format!("{error:#}").into()),
        }
    }
}

/// The main window's root view.
pub struct MainWindow {
    load: Loader,
    screen: Screen,
    task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for MainWindow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MainWindow")
            .field("screen", &self.screen)
            .finish_non_exhaustive()
    }
}

impl MainWindow {
    /// Shows a loading state while `load` runs in the background, then the editor or the
    /// screen for why it cannot open. "Check again" runs `load` again.
    pub fn new(
        load: impl Fn() -> anyhow::Result<Session> + Send + Sync + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let appearance = window.observe_window_appearance(|window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });
        let mut this = Self {
            load: Arc::new(load),
            screen: Screen::Loading,
            task: Task::ready(()),
            _subscriptions: vec![appearance],
        };
        this.start_loading(window, cx);
        this
    }

    fn start_loading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Loading;
        let load = self.load.clone();
        self.task = cx.spawn_in(window, async move |this, cx| {
            // Parsing and validating a large config is real work too, so the model is built
            // off the UI thread as well.
            let opened = cx
                .background_spawn(async move {
                    let session = load()?;
                    let model =
                        ConfigModel::open(session.display_path, session.context, session.loaded)
                            .context("the config is not valid TOML")?;
                    anyhow::Ok((model, session.prefs_store, session.prefs))
                })
                .await;
            if let Err(error) = this.update_in(cx, |this, window, cx| this.open(opened, window, cx))
            {
                log::debug!("the main window closed while loading: {error}");
            }
        });
        cx.notify();
    }

    fn open(
        &mut self,
        opened: anyhow::Result<(ConfigModel, PrefsStore, Prefs)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.screen = match opened {
            Ok((model, prefs_store, prefs)) => {
                let model = cx.new(|_| model);
                model.update(cx, ConfigModel::refresh_aerospace);
                Screen::Ready(cx.new(|cx| Shell::new(model, prefs_store, prefs, window, cx)))
            }
            Err(error) => {
                log::warn!("{error:#}");
                Screen::after_failure(error)
            }
        };
        cx.notify();
    }

    fn create_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::NotFound { suggested, .. } = &self.screen else {
            return;
        };
        let suggested = suggested.clone();
        self.screen = Screen::Loading;
        self.task = cx.spawn_in(window, async move |this, cx| {
            let target = suggested.clone();
            let created = cx
                .background_spawn(async move { create_default_config(&target) })
                .await;
            let shown = this.update_in(cx, |this, window, cx| match created {
                Ok(()) => this.start_loading(window, cx),
                Err(error) => {
                    log::error!("{error}");
                    this.screen = Screen::NotFound {
                        suggested,
                        error: Some(error.to_string().into()),
                    };
                    cx.notify();
                }
            });
            if let Err(error) = shown {
                log::debug!("the main window closed while creating the config: {error}");
            }
        });
        cx.notify();
    }

    fn check_again_button(cx: &mut Context<Self>) -> Button {
        Button::new("check-again")
            .label("Check again")
            .on_click(cx.listener(|this, _, window, cx| this.start_loading(window, cx)))
    }

    fn render_ambiguous(home: &Path, xdg: &Path, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(div().text_lg().child("Two AeroSpace configs found"))
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .children([home.display().to_string(), xdg.display().to_string()]),
            )
            .child(div().text_color(muted).child(
                "AeroSpace refuses to start when both files exist, so PlaneUI edits neither. \
                 Remove or rename one of them, then check again.",
            ))
            .child(h_flex().child(Self::check_again_button(cx)))
            .into_any_element()
    }

    fn render_not_found(
        suggested: &Path,
        error: Option<&SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .gap_3()
            .child(div().text_lg().child("No AeroSpace config yet"))
            .child(div().text_color(muted).child(format!(
                "PlaneUI can create {} from AeroSpace's default config.",
                suggested.display()
            )))
            .when_some(error, |this, error| {
                this.child(
                    div().id("create-error").test_support().child(
                        Alert::error("create-error-alert", error.clone())
                            .title("Could not create the config"),
                    ),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("create-default")
                            .primary()
                            .label("Create from AeroSpace's default config")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_default(window, cx)),
                            ),
                    )
                    .child(Self::check_again_button(cx)),
            )
            .into_any_element()
    }

    fn render_failed(message: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_3()
            .child(div().text_lg().child("PlaneUI cannot open the config"))
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(message.clone()),
            )
            .child(h_flex().child(Self::check_again_button(cx)))
            .into_any_element()
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.screen {
            Screen::Ready(shell) => return div().size_full().child(shell.clone()),
            Screen::Loading => div()
                .text_color(cx.theme().muted_foreground)
                .child("Loading the AeroSpace config…")
                .into_any_element(),
            Screen::Ambiguous { home, xdg } => Self::render_ambiguous(home, xdg, cx),
            Screen::NotFound { suggested, error } => {
                Self::render_not_found(suggested, error.as_ref(), cx)
            }
            Screen::Failed(message) => Self::render_failed(message, cx),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(div().max_w_128().child(content))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, TestAppContext};
    use planeui_core::aerospace::FakeAerospace;
    use planeui_core::store::DEFAULT_CONFIG;

    use super::*;
    use crate::test_support::{StartupFixture, in_window, open_window, write_file};

    fn open_main(
        cx: &mut TestAppContext,
        fixture: &StartupFixture,
    ) -> (AnyWindowHandle, Entity<MainWindow>) {
        let load = fixture.loader();
        open_window(cx, |window, cx| MainWindow::new(load, window, cx))
    }

    fn is_ready(cx: &mut TestAppContext, main: &Entity<MainWindow>) -> bool {
        main.read_with(cx, |main, _| matches!(main.screen, Screen::Ready(_)))
    }

    fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn shows_a_loading_state_until_the_config_is_read(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        write_file(&fixture.env.xdg_config_path(), "accordion-padding = 4\n");
        let load = fixture.loader();
        let (_, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| MainWindow::new(load, window, cx))
            })
            .unwrap()
        });
        assert!(main.read_with(cx, |main, _| matches!(main.screen, Screen::Loading)));

        cx.run_until_parked();
        assert!(is_ready(cx, &main));
    }

    #[gpui_kit::test]
    fn two_configs_open_nothing_until_one_is_removed(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        let home = fixture.env.home_config_path();
        let xdg = fixture.env.xdg_config_path();
        write_file(&home, "accordion-padding = 1\n");
        write_file(&xdg, "accordion-padding = 2\n");
        let (window, main) = open_main(cx, &fixture);

        let screen = main.read_with(cx, |main, _| match &main.screen {
            Screen::Ambiguous { home, xdg } => Some((home.clone(), xdg.clone())),
            _ => None,
        });
        assert_eq!(screen, Some((home.clone(), xdg.clone())));

        click(cx, window, "check-again");
        assert!(!is_ready(cx, &main), "still ambiguous");

        fs::remove_file(&home).unwrap();
        click(cx, window, "check-again");
        assert!(is_ready(cx, &main));
        assert_eq!(fs::read_to_string(&xdg).unwrap(), "accordion-padding = 2\n");
    }

    #[gpui_kit::test]
    fn creates_the_default_config_and_opens_it(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        let (window, main) = open_main(cx, &fixture);
        let suggested = fixture.env.xdg_config_path();
        assert!(main.read_with(cx, |main, _| matches!(
            &main.screen,
            Screen::NotFound { suggested: path, error: None } if *path == suggested
        )));

        click(cx, window, "create-default");

        let vendored = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../planeui-core/tests/fixtures/aerospace-default.toml"),
        )
        .unwrap();
        assert_eq!(fs::read(&suggested).unwrap(), vendored);
        assert_eq!(DEFAULT_CONFIG.as_bytes(), vendored);
        assert!(is_ready(cx, &main));
        assert_eq!(
            fixture.aerospace.reload_count(),
            0,
            "creating is not an edit"
        );
    }

    #[gpui_kit::test]
    fn a_failed_create_stays_on_the_offer_and_says_why(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        // A read-only parent lets the config be found missing but not created.
        let config_home = fixture.env.home.join(".config");
        fs::create_dir_all(&config_home).unwrap();
        fs::set_permissions(&config_home, fs::Permissions::from_mode(0o555)).unwrap();
        let (window, main) = open_main(cx, &fixture);

        click(cx, window, "create-default");

        assert!(main.read_with(cx, |main, _| matches!(
            &main.screen,
            Screen::NotFound { error: Some(_), .. }
        )));
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("create-error").is_some());
        });
    }

    #[gpui_kit::test]
    fn opens_the_config_without_aerospace_and_says_so(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::missing());
        write_file(&fixture.env.xdg_config_path(), "accordion-padding = 4\n");
        let (window, main) = open_main(cx, &fixture);

        assert!(is_ready(cx, &main));
        let state = in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window
                .find("aerospace-state")
                .label()
                .unwrap_or_default()
                .to_owned()
        });
        assert_eq!(state, "AeroSpace not installed");
    }

    fn auto_reload_banner_shown(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
        in_window(cx, window, |window, cx| {
            window.render_frame(cx);
            window.try_find("auto-reload-banner").is_some()
        })
    }

    #[gpui_kit::test]
    fn keeping_auto_reload_on_is_remembered_across_launches(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        let config = fixture.env.xdg_config_path();
        let text = "auto-reload-config = true\n";
        write_file(&config, text);
        let (window, _) = open_main(cx, &fixture);
        assert!(auto_reload_banner_shown(cx, window));

        click(cx, window, "keep-auto-reload");
        assert!(!auto_reload_banner_shown(cx, window));
        assert_eq!(fs::read_to_string(&config).unwrap(), text);
        assert_eq!(fixture.aerospace.reload_count(), 0);

        let (relaunched, main) = open_main(cx, &fixture);
        assert!(is_ready(cx, &main));
        assert!(!auto_reload_banner_shown(cx, relaunched));
        let prefs = PrefsStore::new(fixture.prefs_path.clone()).load().unwrap();
        assert!(prefs.is_auto_reload_dismissed(&config));
    }

    #[gpui_kit::test]
    fn an_unreadable_config_shows_why(cx: &mut TestAppContext) {
        let fixture = StartupFixture::new(FakeAerospace::new());
        write_file(&fixture.env.xdg_config_path(), "accordion-padding = [\n");
        let (_, main) = open_main(cx, &fixture);

        assert!(main.read_with(cx, |main, _| matches!(
            &main.screen,
            Screen::Failed(message) if message.contains("not valid TOML")
        )));
    }
}
