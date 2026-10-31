//! A `ConfigModel` over a temporary config file, a temporary snapshot directory and a fake
//! AeroSpace, so tests never touch the real home directory or the real `aerospace`.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{AppContext as _, Entity, TestAppContext};
use planeui_core::aerospace::FakeAerospace;
use planeui_core::apply::ApplyContext;
use planeui_core::config_doc::KeyPath;
use planeui_core::store::{ConfigStore, ResolvedConfig, SnapshotStore};
use tempfile::TempDir;

use crate::config_model::ConfigModel;

pub struct Fixture {
    _directory: TempDir,
    pub config: PathBuf,
    pub snapshot_dir: PathBuf,
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
