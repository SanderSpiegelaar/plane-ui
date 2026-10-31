//! Config path resolution, atomic writes, and snapshots.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

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
