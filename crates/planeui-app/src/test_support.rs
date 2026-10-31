//! A `ConfigModel` over a temporary config file, a temporary snapshot directory and a fake
//! AeroSpace, so tests never touch the real home directory or the real `aerospace`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, Point, Render, TestAppContext,
    Window, WindowBounds, WindowOptions, px, size,
};
use planeui_core::aerospace::FakeAerospace;
use planeui_core::apply::ApplyContext;
use planeui_core::config_doc::KeyPath;
use planeui_core::prefs::PrefsStore;
use planeui_core::store::{ConfigStore, PathEnv, ResolvedConfig, SnapshotStore};
use tempfile::TempDir;

use crate::config_model::ConfigModel;
use crate::live_data::LiveData;
use crate::startup::{Session, open_session};

pub struct Fixture {
    _directory: TempDir,
    pub config: PathBuf,
    pub snapshot_dir: PathBuf,
    pub prefs_path: PathBuf,
    pub aerospace: Arc<FakeAerospace>,
}

impl Fixture {
    pub fn new(text: &str) -> Self {
        Self::with_aerospace(text, FakeAerospace::new())
    }

    pub fn with_aerospace(text: &str, aerospace: FakeAerospace) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("aerospace.toml");
        fs::write(&config, text).unwrap();
        Self {
            config,
            snapshot_dir: directory.path().join("snapshots"),
            prefs_path: directory.path().join("prefs.json"),
            aerospace: Arc::new(aerospace),
            _directory: directory,
        }
    }

    pub fn model(&self, cx: &mut TestAppContext) -> Entity<ConfigModel> {
        let resolved = ResolvedConfig::new_file(self.config.clone());
        let store = ConfigStore::new(&resolved);
        let loaded = store.load().unwrap();
        let context = ApplyContext {
            store,
            snapshots: SnapshotStore::new(self.snapshot_dir.clone()),
            aerospace: self.aerospace.clone(),
        };
        let model = ConfigModel::open(resolved.display_path, context, loaded).unwrap();
        cx.new(|_| model)
    }

    pub fn prefs_store(&self) -> PrefsStore {
        PrefsStore::new(self.prefs_path.clone())
    }

    pub fn live_data(&self, cx: &mut TestAppContext) -> Entity<LiveData> {
        cx.new(|cx| LiveData::new(self.aerospace.clone(), cx))
    }

    pub fn disk(&self) -> String {
        fs::read_to_string(&self.config).unwrap()
    }

    pub fn write_disk(&self, text: &str) {
        fs::write(&self.config, text).unwrap();
    }

    /// Snapshot contents, oldest first.
    pub fn snapshots(&self) -> Vec<String> {
        let store = SnapshotStore::new(self.snapshot_dir.clone());
        let Ok(mut snapshots) = store.list() else {
            return Vec::new();
        };
        snapshots.reverse();
        snapshots
            .iter()
            .map(|meta| String::from_utf8(store.read(&meta.id).unwrap()).unwrap())
            .collect()
    }

    /// A file where the snapshot directory belongs makes every snapshot fail.
    pub fn block_snapshots(&self) {
        fs::write(&self.snapshot_dir, "not a directory").unwrap();
    }

    pub fn unblock_snapshots(&self) {
        fs::remove_file(&self.snapshot_dir).unwrap();
    }
}

/// A temporary home directory and data directory for the startup flow, with a fake AeroSpace.
pub struct StartupFixture {
    _directory: TempDir,
    pub env: PathEnv,
    pub snapshot_dir: PathBuf,
    pub prefs_path: PathBuf,
    pub aerospace: Arc<FakeAerospace>,
}

impl StartupFixture {
    pub fn new(aerospace: FakeAerospace) -> Self {
        let directory = tempfile::tempdir().unwrap();
        // Resolved, so paths built from it match the canonical paths startup reports.
        let root = directory.path().canonicalize().unwrap();
        Self {
            env: PathEnv {
                home: root.join("home"),
                xdg_config_home: None,
            },
            snapshot_dir: root.join("PlaneUI/snapshots"),
            prefs_path: root.join("PlaneUI/prefs.json"),
            aerospace: Arc::new(aerospace),
            _directory: directory,
        }
    }

    pub fn loader(&self) -> impl Fn() -> anyhow::Result<Session> + Send + Sync + 'static {
        let env = self.env.clone();
        let aerospace = self.aerospace.clone();
        let snapshot_dir = self.snapshot_dir.clone();
        let prefs_path = self.prefs_path.clone();
        move || {
            open_session(
                &env,
                aerospace.clone(),
                SnapshotStore::new(snapshot_dir.clone()),
                PrefsStore::new(prefs_path.clone()),
            )
        }
    }
}

pub fn write_file(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub fn set_padding(model: &Entity<ConfigModel>, padding: i64, cx: &mut TestAppContext) {
    model.update(cx, |model, cx| {
        model
            .edit(format!("Set accordion-padding to {padding}"), cx, |doc| {
                doc.set_value(&KeyPath::from(["accordion-padding"]), padding.into())
            })
            .unwrap();
    });
}

/// Opens a window holding the view `build` makes, with gpui-kit and PlaneUI's key bindings set
/// up.
pub fn open_window<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V,
) -> (AnyWindowHandle, Entity<V>) {
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::bind_keys(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(960.), px(640.)),
            })),
            ..WindowOptions::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| build(window, cx))).unwrap()
    });
    // GPUI delivers focus and blur only to an active window.
    in_window(cx, window, |window, _| window.activate_window());
    cx.run_until_parked();
    (window, view)
}

pub fn in_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    update: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| update(window, cx))
        .unwrap()
}
