//! PlaneUI's own preferences, stored as JSON next to the snapshots. Nothing here touches the
//! AeroSpace config.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{ConfigStore, ResolvedConfig, WriteError};

/// Preferences that outlive a session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Prefs {
    /// Config paths for which the user chose to keep `auto-reload-config = true`.
    pub dismissed_auto_reload: Vec<PathBuf>,
}

impl Prefs {
    /// Whether the auto-reload banner was dismissed for `config`.
    #[must_use]
    pub fn is_auto_reload_dismissed(&self, config: &Path) -> bool {
        self.dismissed_auto_reload.iter().any(|path| path == config)
    }

    /// Records that the user keeps auto-reload on for `config`.
    pub fn dismiss_auto_reload(&mut self, config: &Path) {
        if !self.is_auto_reload_dismissed(config) {
            self.dismissed_auto_reload.push(config.to_path_buf());
        }
    }
}

/// Why preferences could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PrefsError {
    /// The file exists but could not be read.
    #[error("could not read preferences at {path}: {source}")]
    Read {
        /// The prefs file.
        path: PathBuf,
        /// The I/O failure.
        source: io::Error,
    },
    /// The file is not valid preferences JSON.
    #[error("preferences at {path} are not valid: {source}")]
    Parse {
        /// The prefs file.
        path: PathBuf,
        /// The JSON failure.
        source: serde_json::Error,
    },
    /// Serializing failed.
    #[error("could not encode preferences: {0}")]
    Encode(serde_json::Error),
    /// The atomic write failed.
    #[error(transparent)]
    Write(#[from] WriteError),
}

/// Reads and writes [`Prefs`] at one path.
#[derive(Debug, Clone)]
pub struct PrefsStore {
    path: PathBuf,
}

impl PrefsStore {
    /// A store for `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `dirs::data_dir()/PlaneUI/prefs.json` (Application Support on macOS).
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|directory| directory.join("PlaneUI").join("prefs.json"))
    }

    /// Loads the preferences; a missing file means defaults.
    ///
    /// # Errors
    ///
    /// Returns [`PrefsError`] when the file cannot be read or parsed.
    pub fn load(&self) -> Result<Prefs, PrefsError> {
        match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| PrefsError::Parse {
                path: self.path.clone(),
                source,
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Prefs::default()),
            Err(source) => Err(PrefsError::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }

    /// Writes the preferences atomically, creating the directory if needed.
    ///
    /// # Errors
    ///
    /// Returns [`PrefsError`] when encoding or writing fails.
    pub fn save(&self, prefs: &Prefs) -> Result<(), PrefsError> {
        let mut bytes = serde_json::to_vec_pretty(prefs).map_err(PrefsError::Encode)?;
        bytes.push(b'\n');
        ConfigStore::new(&ResolvedConfig::new_file(self.path.clone())).write_atomic(&bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let store = PrefsStore::new(directory.path().join("PlaneUI/prefs.json"));
        assert_eq!(store.load().unwrap(), Prefs::default());
    }

    #[test]
    fn dismissal_survives_a_save_and_load() {
        let directory = tempfile::tempdir().unwrap();
        let store = PrefsStore::new(directory.path().join("PlaneUI/prefs.json"));
        let config = Path::new("/Users/someone/.config/aerospace/aerospace.toml");
        let mut prefs = Prefs::default();
        prefs.dismiss_auto_reload(config);
        prefs.dismiss_auto_reload(config);
        store.save(&prefs).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.is_auto_reload_dismissed(config));
        assert_eq!(loaded.dismissed_auto_reload.len(), 1);
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("prefs.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(matches!(
            PrefsStore::new(path).load(),
            Err(PrefsError::Parse { .. })
        ));
    }
}
