//! Config path resolution, atomic writes, and snapshots.

mod snapshot;

pub use snapshot::{
    SNAPSHOT_KEEP, SnapshotError, SnapshotId, SnapshotMeta, SnapshotStore, UtcDateTime,
    line_diff_stats,
};

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The environment AeroSpace consults when it looks for its config file.
///
/// Tests build this directly so they never touch the real home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEnv {
    /// The user's home directory (`HOME`).
    pub home: PathBuf,
    /// `XDG_CONFIG_HOME`, when set to a non-empty value.
    pub xdg_config_home: Option<PathBuf>,
}

impl PathEnv {
    /// Reads `HOME` and `XDG_CONFIG_HOME` from the current process.
    ///
    /// Returns `None` when the home directory cannot be determined.
    #[must_use]
    pub fn from_process() -> Option<Self> {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir)?;
        Some(Self {
            home,
            xdg_config_home: non_empty_path(std::env::var_os("XDG_CONFIG_HOME")),
        })
    }

    /// `~/.aerospace.toml`.
    #[must_use]
    pub fn home_config_path(&self) -> PathBuf {
        self.home.join(".aerospace.toml")
    }

    /// `${XDG_CONFIG_HOME:-~/.config}/aerospace/aerospace.toml`.
    #[must_use]
    pub fn xdg_config_path(&self) -> PathBuf {
        self.xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
            .join("aerospace")
            .join("aerospace.toml")
    }
}

fn non_empty_path(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// AeroSpace's default config, vendored byte-for-byte, for creating a config where none
/// exists.
pub const DEFAULT_CONFIG: &str = include_str!("../tests/fixtures/aerospace-default.toml");

/// Writes [`DEFAULT_CONFIG`] to `path` atomically, creating its directory. A file that appeared
/// at `path` since it was found missing is left alone, so the caller can simply open it.
///
/// # Errors
///
/// `path` cannot be inspected, or the write fails.
pub fn create_default_config(path: &Path) -> Result<(), WriteError> {
    ConfigStore::new(&ResolvedConfig::new_file(path.to_path_buf()))
        .write_new(DEFAULT_CONFIG.as_bytes())?;
    Ok(())
}

/// The config file PlaneUI edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// The path the user knows the file by; may be a symlink.
    pub display_path: PathBuf,
    /// The symlink-resolved file that reads and writes go to.
    pub target_path: PathBuf,
}

impl ResolvedConfig {
    /// A config at `path` that does not exist yet, such as `LocateError::NotFound`'s suggestion.
    #[must_use]
    pub fn new_file(path: PathBuf) -> Self {
        Self {
            display_path: path.clone(),
            target_path: path,
        }
    }
}

/// Why no single config file could be chosen.
#[derive(Debug, thiserror::Error)]
pub enum LocateError {
    /// Both candidate files exist; AeroSpace refuses to start in that case.
    #[error(
        "both {home} and {xdg} exist; AeroSpace treats that as ambiguous and refuses to load \
         either, so remove one of them"
    )]
    Ambiguous {
        /// `~/.aerospace.toml`.
        home: PathBuf,
        /// `${XDG_CONFIG_HOME:-~/.config}/aerospace/aerospace.toml`.
        xdg: PathBuf,
    },
    /// No config file exists yet.
    #[error("no AeroSpace config found; one can be created at {suggested}")]
    NotFound {
        /// Where a new config should be created.
        suggested: PathBuf,
    },
    /// The file system could not be queried.
    #[error("could not inspect {path}: {source}")]
    Io {
        /// The path being inspected.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// Finds the config file AeroSpace uses, mirroring its lookup rules.
///
/// `server_path` is what the running server reports via `aerospace config --config-path`;
/// it wins over the default locations because the server may run with a custom path.
///
/// # Errors
///
/// `Ambiguous` when both default locations exist, `NotFound` when the chosen file does not
/// exist, and `Io` when the file system cannot be queried.
pub fn resolve_config_path(
    env: &PathEnv,
    server_path: Option<PathBuf>,
) -> Result<ResolvedConfig, LocateError> {
    if let Some(path) = server_path {
        return if exists(&path)? {
            resolve_existing(path)
        } else {
            Err(LocateError::NotFound { suggested: path })
        };
    }
    let home = env.home_config_path();
    let xdg = env.xdg_config_path();
    match (exists(&home)?, exists(&xdg)?) {
        (true, true) => Err(LocateError::Ambiguous { home, xdg }),
        (true, false) => resolve_existing(home),
        (false, true) => resolve_existing(xdg),
        (false, false) => Err(LocateError::NotFound { suggested: xdg }),
    }
}

/// Follows symlinks, so a dangling link counts as missing, as it does for AeroSpace.
fn exists(path: &Path) -> Result<bool, LocateError> {
    path.try_exists().map_err(|source| LocateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn resolve_existing(display_path: PathBuf) -> Result<ResolvedConfig, LocateError> {
    let target_path = display_path
        .canonicalize()
        .map_err(|source| LocateError::Io {
            path: display_path.clone(),
            source,
        })?;
    Ok(ResolvedConfig {
        display_path,
        target_path,
    })
}

/// The exact bytes of the config as last seen on disk.
///
/// External edits are detected by comparing content, not mtime, so a touch without a change is
/// not a conflict and an edit within the mtime resolution is not missed.
#[derive(Clone, PartialEq, Eq)]
pub struct DiskStamp(Vec<u8>);

impl DiskStamp {
    /// The stamp of a file holding `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(bytes.to_vec())
    }
}

impl fmt::Debug for DiskStamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DiskStamp({} bytes)", self.0.len())
    }
}

/// The config text as read from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// The file contents, line endings untouched.
    pub text: String,
    /// The stamp to compare against before the next write.
    pub stamp: DiskStamp,
}

/// Why the config could not be read.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The file does not exist.
    #[error("{path} does not exist")]
    NotFound {
        /// The file that was read.
        path: PathBuf,
    },
    /// The file is not valid UTF-8, so it cannot be TOML.
    #[error("{path} is not valid UTF-8")]
    NotUtf8 {
        /// The file that was read.
        path: PathBuf,
    },
    /// Reading failed.
    #[error("could not read {path}: {source}")]
    Io {
        /// The file that was read.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// Why a write must not go ahead.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// The file on disk differs from the expected stamp; it was edited outside PlaneUI.
    #[error("the config was changed outside PlaneUI")]
    Changed {
        /// What is on disk now.
        current: Box<Loaded>,
    },
    /// The file could not be read, including when it was deleted.
    #[error(transparent)]
    Load(#[from] LoadError),
}

/// Why an atomic write failed. The original file is untouched in every case.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// The target's symlinks could not be resolved.
    #[error("could not resolve {path}: {source}")]
    Resolve {
        /// The configured target.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The directory holding the target could not be created.
    #[error("could not create {path}: {source}")]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// No temporary file could be created next to the target.
    #[error("could not create a temporary file in {path}: {source}")]
    CreateTemp {
        /// The directory the temporary file belongs in.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The temporary file could not be written, given permissions, or synced.
    #[error("could not write {path}: {source}")]
    WriteTemp {
        /// The temporary file, already removed again.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The temporary file could not replace the target.
    #[error("could not replace {path}: {source}")]
    Rename {
        /// The target.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

/// Reads and writes the config file at a resolved target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigStore {
    target: PathBuf,
}

/// Temp-file names are retried on collision instead of coordinated through shared state.
const TEMP_FILE_ATTEMPTS: u32 = 16;

impl ConfigStore {
    /// A store for `resolved.target_path`.
    #[must_use]
    pub fn new(resolved: &ResolvedConfig) -> Self {
        Self {
            target: resolved.target_path.clone(),
        }
    }

    /// The file this store reads and writes.
    #[must_use]
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// Reads the config and stamps it.
    ///
    /// # Errors
    ///
    /// `NotFound` when the file is missing, `NotUtf8` when it is not text, `Io` otherwise.
    pub fn load(&self) -> Result<Loaded, LoadError> {
        let bytes = fs::read(&self.target).map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                LoadError::NotFound {
                    path: self.target.clone(),
                }
            } else {
                LoadError::Io {
                    path: self.target.clone(),
                    source,
                }
            }
        })?;
        let stamp = DiskStamp::of(&bytes);
        let text = String::from_utf8(bytes).map_err(|_| LoadError::NotUtf8 {
            path: self.target.clone(),
        })?;
        Ok(Loaded { text, stamp })
    }

    /// Confirms the file on disk still matches `expected`.
    ///
    /// # Errors
    ///
    /// `Changed` with the current contents when the file was edited elsewhere, `Load` when it
    /// cannot be read (a deleted file is `Load(LoadError::NotFound)`).
    pub fn check_unchanged(&self, expected: &DiskStamp) -> Result<(), CheckError> {
        let current = self.load()?;
        if current.stamp == *expected {
            Ok(())
        } else {
            Err(CheckError::Changed {
                current: Box::new(current),
            })
        }
    }

    /// Replaces the file with `bytes` so readers see either the old or the new contents.
    ///
    /// Writes a temporary file in the target's directory, syncs it, and renames it over the
    /// target. Symlinks are followed so a link stays a link; existing permissions are kept. A
    /// missing target and its directory are created.
    ///
    /// # Errors
    ///
    /// Any failing step; the temporary file is removed and the target left as it was.
    pub fn write_atomic(&self, bytes: &[u8]) -> Result<DiskStamp, WriteError> {
        self.write_via_temp(bytes, Publish::Replace)?;
        Ok(DiskStamp::of(bytes))
    }

    /// Writes `bytes` only if nothing exists at the target, atomically and without a window
    /// in which a file created by someone else could be replaced. Returns `false`, leaving the
    /// existing file alone, when the target exists.
    ///
    /// # Errors
    ///
    /// Returns [`WriteError`] when any step fails; no partial file is left behind.
    pub fn write_new(&self, bytes: &[u8]) -> Result<bool, WriteError> {
        self.write_via_temp(bytes, Publish::CreateNew)
    }

    fn write_via_temp(&self, bytes: &[u8], publish: Publish) -> Result<bool, WriteError> {
        let target = self.resolved_target()?;
        let directory = target
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        fs::create_dir_all(&directory).map_err(|source| WriteError::CreateDir {
            path: directory.clone(),
            source,
        })?;
        let permissions = match fs::metadata(&target) {
            Ok(metadata) => Some(metadata.permissions()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(WriteError::Resolve {
                    path: target,
                    source,
                });
            }
        };

        let (temp_path, mut temp_file) = create_temp_file(&directory, &target)?;
        let written = (|| {
            temp_file.write_all(bytes)?;
            if let Some(permissions) = permissions {
                temp_file.set_permissions(permissions)?;
            }
            temp_file.sync_all()
        })();
        drop(temp_file);
        if let Err(source) = written {
            remove_temp_file(&temp_path);
            return Err(WriteError::WriteTemp {
                path: temp_path,
                source,
            });
        }
        let published = match publish {
            Publish::Replace => fs::rename(&temp_path, &target),
            // link(2) fails if the target exists, so a file that appeared since the caller
            // checked is never replaced.
            Publish::CreateNew => {
                let linked = fs::hard_link(&temp_path, &target);
                remove_temp_file(&temp_path);
                linked
            }
        };
        match published {
            Ok(()) => Ok(true),
            Err(error)
                if publish == Publish::CreateNew
                    && error.kind() == io::ErrorKind::AlreadyExists =>
            {
                Ok(false)
            }
            Err(source) => {
                remove_temp_file(&temp_path);
                Err(WriteError::Rename {
                    path: target,
                    source,
                })
            }
        }
    }

    /// Resolves at write time too, in case the target became a symlink after it was located.
    fn resolved_target(&self) -> Result<PathBuf, WriteError> {
        match self.target.canonicalize() {
            Ok(path) => Ok(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(self.target.clone()),
            Err(source) => Err(WriteError::Resolve {
                path: self.target.clone(),
                source,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Publish {
    Replace,
    CreateNew,
}

fn create_temp_file(directory: &Path, target: &Path) -> Result<(PathBuf, File), WriteError> {
    let file_name = target
        .file_name()
        .map_or_else(|| "config".into(), |name| name.to_string_lossy());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut last_error = io::Error::from(io::ErrorKind::AlreadyExists);
    for attempt in 0..TEMP_FILE_ATTEMPTS {
        let path = directory.join(format!(
            ".{file_name}.planeui-{}-{nanos}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last_error = error,
            Err(source) => {
                return Err(WriteError::CreateTemp {
                    path: directory.to_path_buf(),
                    source,
                });
            }
        }
    }
    Err(WriteError::CreateTemp {
        path: directory.to_path_buf(),
        source: last_error,
    })
}

fn remove_temp_file(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        log::warn!(
            "could not remove temporary file {}: {error}",
            path.display()
        );
    }
}
