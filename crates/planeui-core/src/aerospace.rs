//! The [`Aerospace`] trait: every call PlaneUI makes to AeroSpace goes through it, so tests use
//! [`FakeAerospace`] instead of the real CLI.

#[cfg(any(test, feature = "test-support"))]
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use std::{env, fmt, thread};

use crate::commands::WorkspaceName;

/// A running application available to rule pickers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct App {
    /// macOS bundle identifier.
    #[serde(rename = "app-bundle-id")]
    pub bundle_id: String,
    /// Application display name.
    #[serde(rename = "app-name")]
    pub name: String,
}

/// A connected monitor in AeroSpace's left-to-right sequence order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Monitor {
    /// One-based sequence number.
    #[serde(rename = "monitor-id")]
    pub id: u32,
    /// Display name reported by macOS.
    #[serde(rename = "monitor-name")]
    pub name: String,
    /// Whether macOS considers this the main display.
    #[serde(rename = "monitor-is-main")]
    pub is_main: bool,
}

/// Talks to AeroSpace.
///
/// Calls block until the CLI answers or times out, so the app runs them off the UI thread.
pub trait Aerospace: Send + Sync + fmt::Debug {
    /// Running applications from `list-apps --json`, ignoring unknown JSON fields.
    ///
    /// # Errors
    /// Returns a CLI error, including [`CliError::NotRunning`] when the server is unavailable.
    fn list_apps(&self) -> Result<Vec<App>, CliError>;

    /// Connected monitors including the main flag, requested explicitly through `--format`.
    ///
    /// # Errors
    /// Returns a CLI error, including [`CliError::NotRunning`] when the server is unavailable.
    fn list_monitors(&self) -> Result<Vec<Monitor>, CliError>;

    /// Known workspaces from `list-workspaces --all --json` in server order.
    ///
    /// # Errors
    /// Returns a CLI error for an unavailable server, malformed JSON, or invalid workspace name.
    fn list_workspaces(&self) -> Result<Vec<WorkspaceName>, CliError>;

    /// The installed version, from `aerospace --version`.
    ///
    /// # Errors
    ///
    /// Returns a [`CliError`] when the CLI cannot run or prints no recognizable version.
    fn version(&self) -> Result<AerospaceVersion, CliError>;

    /// The config file the running server loaded, from `aerospace config --config-path`.
    /// `Ok(None)` when the server is not running.
    ///
    /// # Errors
    ///
    /// Returns a [`CliError`] when the CLI cannot run or answers with something other than a
    /// path or the not-running message.
    fn config_path(&self) -> Result<Option<PathBuf>, CliError>;

    /// Makes the running server reload its config, with `aerospace reload-config --no-gui`.
    ///
    /// # Errors
    ///
    /// Returns a [`CliError`] when the CLI cannot run or fails without saying why. A config
    /// AeroSpace rejects and a server that is not running are outcomes, not errors.
    fn reload_config(&self) -> Result<ReloadOutcome, CliError>;
}

/// What happened when AeroSpace was asked to reload its config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// AeroSpace reloaded the config without errors. Warnings are logged, not reported.
    Applied,
    /// AeroSpace reported errors. It may still have applied the parts it could parse.
    Failed {
        /// One entry per error AeroSpace printed, in its order.
        errors: Vec<ReloadMessage>,
        /// Everything the CLI printed, stdout then stderr.
        raw: String,
    },
    /// The AeroSpace server is not running; the config applies when it next starts.
    NotRunning,
}

/// One error AeroSpace printed while reloading the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadMessage {
    /// The config key the error is about, such as `gaps.inner.vertical` or
    /// `on-window-detected[0].run`. `None` for errors about the whole file.
    pub key_path: Option<String>,
    /// AeroSpace's description of the problem; may span several lines.
    pub message: String,
}

impl ReloadMessage {
    fn from_block(block: &str) -> Self {
        // File-level messages can contain ": " themselves; key paths never contain whitespace.
        match block.split_once(": ") {
            Some((key_path, message))
                if !key_path.is_empty() && !key_path.contains(char::is_whitespace) =>
            {
                Self {
                    key_path: Some(key_path.to_owned()),
                    message: message.to_owned(),
                }
            }
            _ => Self {
                key_path: None,
                message: block.to_owned(),
            },
        }
    }
}

/// An AeroSpace release version such as `0.21.3-Beta`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AerospaceVersion {
    /// Major version.
    pub major: u32,
    /// Minor version.
    pub minor: u32,
    /// Patch version.
    pub patch: u32,
    /// Pre-release label after the `-`, such as `Beta`.
    pub pre_release: Option<String>,
}

impl AerospaceVersion {
    fn parse(text: &str) -> Option<Self> {
        let (core, pre_release) = match text.split_once('-') {
            Some((core, pre_release)) if !pre_release.is_empty() => {
                (core, Some(pre_release.to_owned()))
            }
            Some(_) => return None,
            None => (text, None),
        };
        let mut numbers = core.split('.').map(str::parse::<u32>);
        let (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) = (
            numbers.next(),
            numbers.next(),
            numbers.next(),
            numbers.next(),
        ) else {
            return None;
        };
        Some(Self {
            major,
            minor,
            patch,
            pre_release,
        })
    }
}

impl fmt::Display for AerospaceVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)?;
        match &self.pre_release {
            Some(pre_release) => write!(formatter, "-{pre_release}"),
            None => Ok(()),
        }
    }
}

/// Whether the installed major/minor series is newer than PlaneUI's command catalogue.
/// Patch releases and pre-release labels do not change compatibility warnings.
#[must_use]
pub fn newer_than_catalogue(installed: &AerospaceVersion) -> bool {
    AerospaceVersion::parse(crate::commands::CATALOGUE_VERSION).is_some_and(|catalogue| {
        (installed.major, installed.minor) > (catalogue.major, catalogue.minor)
    })
}

/// Why a call to the AeroSpace CLI produced no answer.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The CLI is installed, but its server cannot be reached.
    #[error("AeroSpace is not running")]
    NotRunning,
    /// No `aerospace` binary on `PATH`, in `/opt/homebrew/bin`, or in `/usr/local/bin`.
    #[error("the aerospace command was not found; is AeroSpace installed?")]
    BinaryNotFound,
    /// The CLI did not finish in time and was killed.
    #[error("`aerospace {command}` did not finish within {after:?}")]
    Timeout {
        /// The arguments passed to `aerospace`.
        command: String,
        /// The time limit that ran out.
        after: Duration,
    },
    /// Starting the CLI or reading its output failed.
    #[error("could not run aerospace: {0}")]
    Io(#[source] io::Error),
    /// The CLI answered in a way PlaneUI does not understand.
    #[error("unexpected output from `aerospace {command}`: {detail}")]
    UnexpectedOutput {
        /// The arguments passed to `aerospace`.
        command: String,
        /// The exit status and everything the CLI printed.
        detail: String,
    },
}

/// Where GUI apps find `aerospace` when it is not on their `PATH`, which they do not inherit
/// from the user's shell.
const FALLBACK_BINARIES: [&str; 2] = ["/opt/homebrew/bin/aerospace", "/usr/local/bin/aerospace"];

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const NOT_RUNNING_MESSAGE: &str = "Can't connect to AeroSpace server";
const CLIENT_VERSION_PREFIX: &str = "aerospace CLI client version: ";
const ERROR_PREFIX: &str = "[ERROR] ";

/// [`Aerospace`] backed by the `aerospace` command-line client.
#[derive(Debug, Clone)]
pub struct CliAerospace {
    binary: PathBuf,
    timeout: Duration,
}

impl CliAerospace {
    /// How long a CLI call may take before it is killed, unless changed with
    /// [`with_timeout`](Self::with_timeout).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

    /// Finds `aerospace` on `PATH`, then at `/opt/homebrew/bin/aerospace`, then at
    /// `/usr/local/bin/aerospace`.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::BinaryNotFound`] when none of those exist.
    pub fn locate() -> Result<Self, CliError> {
        let fallbacks = FALLBACK_BINARIES.map(Path::new);
        find_binary(env::var_os("PATH").as_deref(), &fallbacks)
            .map(Self::with_binary)
            .ok_or(CliError::BinaryNotFound)
    }

    /// Uses the given binary as `aerospace`.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Replaces the per-call time limit.
    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// The binary this client runs.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    fn run(&self, args: &[&str]) -> Result<CliOutput, CliError> {
        let command = args.join(" ");
        let deadline = Instant::now() + self.timeout;
        let timeout = || CliError::Timeout {
            command: command.clone(),
            after: self.timeout,
        };

        let mut child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound => CliError::BinaryNotFound,
                _ => CliError::Io(error),
            })?;
        let stdout = read_in_background(child.stdout.take());
        let stderr = read_in_background(child.stderr.take());

        let status = match wait_until(&mut child, deadline) {
            Ok(Some(status)) => status,
            Ok(None) => {
                kill_and_reap(&mut child);
                return Err(timeout());
            }
            Err(error) => {
                kill_and_reap(&mut child);
                return Err(CliError::Io(error));
            }
        };
        // A process the CLI started can keep the pipes open after it exits, so reading the
        // output shares the deadline instead of waiting for end-of-file.
        let receive = |output: &Receiver<io::Result<Vec<u8>>>| match output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(Ok(bytes)) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(Err(error)) => Err(CliError::Io(error)),
            Err(RecvTimeoutError::Timeout) => Err(timeout()),
            Err(RecvTimeoutError::Disconnected) => Err(CliError::Io(io::Error::other(
                "the thread reading aerospace output stopped",
            ))),
        };
        Ok(CliOutput {
            command: command.clone(),
            status,
            stdout: receive(&stdout)?,
            stderr: receive(&stderr)?,
        })
    }

    fn list<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<Vec<T>, CliError> {
        let output = self.run(args)?;
        if output.server_not_running() {
            return Err(CliError::NotRunning);
        }
        if !output.status.success() {
            return Err(output.unexpected());
        }
        serde_json::from_str(&output.stdout).map_err(|_| output.unexpected())
    }
}

impl Aerospace for CliAerospace {
    fn list_apps(&self) -> Result<Vec<App>, CliError> {
        self.list(&["list-apps", "--json"])
    }

    fn list_monitors(&self) -> Result<Vec<Monitor>, CliError> {
        self.list(&[
            "list-monitors",
            "--json",
            "--format",
            "%{monitor-id}%{monitor-name}%{monitor-is-main}",
        ])
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceName>, CliError> {
        #[derive(serde::Deserialize)]
        struct WorkspaceJson {
            workspace: String,
        }
        self.list::<WorkspaceJson>(&["list-workspaces", "--all", "--json"])?
            .into_iter()
            .map(|row| {
                WorkspaceName::new(row.workspace).map_err(|error| CliError::UnexpectedOutput {
                    command: "list-workspaces --all --json".into(),
                    detail: error.to_string(),
                })
            })
            .collect()
    }

    fn version(&self) -> Result<AerospaceVersion, CliError> {
        let output = self.run(&["--version"])?;
        // The client line is there even when the server is down; the server line then is not.
        output
            .status
            .success()
            .then(|| {
                output
                    .stdout
                    .lines()
                    .find_map(|line| line.strip_prefix(CLIENT_VERSION_PREFIX))
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(AerospaceVersion::parse)
            })
            .flatten()
            .ok_or_else(|| output.unexpected())
    }

    fn config_path(&self) -> Result<Option<PathBuf>, CliError> {
        let output = self.run(&["config", "--config-path"])?;
        if output.server_not_running() {
            return Ok(None);
        }
        let path = output.stdout.trim_end_matches(['\n', '\r']);
        if output.status.success() && !path.is_empty() {
            Ok(Some(PathBuf::from(path)))
        } else {
            Err(output.unexpected())
        }
    }

    fn reload_config(&self) -> Result<ReloadOutcome, CliError> {
        let output = self.run(&["reload-config", "--no-gui"])?;
        if output.status.success() {
            let warnings = output.stderr.trim_end();
            if !warnings.is_empty() {
                log::warn!("AeroSpace reloaded the config with warnings:\n{warnings}");
            }
            return Ok(ReloadOutcome::Applied);
        }
        if output.server_not_running() {
            return Ok(ReloadOutcome::NotRunning);
        }
        let raw = output.combined();
        let mut errors = parse_error_blocks(&output.stdout);
        if errors.is_empty() {
            if raw.is_empty() {
                return Err(output.unexpected());
            }
            errors.push(ReloadMessage {
                key_path: None,
                message: raw.clone(),
            });
        }
        Ok(ReloadOutcome::Failed { errors, raw })
    }
}

#[derive(Debug)]
struct CliOutput {
    command: String,
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl CliOutput {
    fn server_not_running(&self) -> bool {
        !self.status.success() && self.stderr.contains(NOT_RUNNING_MESSAGE)
    }

    fn combined(&self) -> String {
        match (self.stdout.trim_end(), self.stderr.trim_end()) {
            (stdout, "") => stdout.to_owned(),
            ("", stderr) => stderr.to_owned(),
            (stdout, stderr) => format!("{stdout}\n{stderr}"),
        }
    }

    fn unexpected(&self) -> CliError {
        CliError::UnexpectedOutput {
            command: self.command.clone(),
            detail: format!(
                "{}, stdout {:?}, stderr {:?}",
                self.status, self.stdout, self.stderr
            ),
        }
    }
}

/// Splits AeroSpace's `[ERROR] ` blocks. Blocks are separated by blank lines, but one message
/// can contain blank lines too, so a block runs until the next `[ERROR] ` line.
fn parse_error_blocks(stdout: &str) -> Vec<ReloadMessage> {
    let mut blocks: Vec<String> = Vec::new();
    for line in stdout.lines() {
        if let Some(start) = line.strip_prefix(ERROR_PREFIX) {
            blocks.push(start.to_owned());
        } else if let Some(block) = blocks.last_mut() {
            block.push('\n');
            block.push_str(line);
        }
    }
    blocks
        .iter()
        .map(|block| ReloadMessage::from_block(block.trim_end()))
        .collect()
}

fn find_binary(path_variable: Option<&OsStr>, fallbacks: &[&Path]) -> Option<PathBuf> {
    path_variable
        .into_iter()
        .flat_map(env::split_paths)
        .map(|directory| directory.join("aerospace"))
        .chain(fallbacks.iter().map(|path| path.to_path_buf()))
        .find(|path| path.is_file())
}

fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = match pipe {
            Some(mut pipe) => pipe.read_to_end(&mut bytes).map(|_| bytes),
            None => Ok(bytes),
        };
        if sender.send(result).is_err() {
            log::debug!("dropped aerospace output that arrived after the call timed out");
        }
    });
    receiver
}

fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

fn kill_and_reap(child: &mut Child) {
    if let Err(error) = child.kill() {
        log::warn!("could not kill aerospace process {}: {error}", child.id());
    }
    if let Err(error) = child.wait() {
        log::warn!("could not reap aerospace process {}: {error}", child.id());
    }
}

/// Scripted [`Aerospace`] for tests.
///
/// Starts as an installed AeroSpace 0.21.3-Beta whose `config_path` is `None` and whose
/// reloads apply, unless [`queue_reload`](Self::queue_reload) scripted otherwise.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
pub struct FakeAerospace {
    state: Mutex<FakeState>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct FakeState {
    installed: bool,
    running: bool,
    apps: Vec<App>,
    monitors: Vec<Monitor>,
    workspaces: Vec<WorkspaceName>,
    list_count: usize,
    version: AerospaceVersion,
    config_path: Option<PathBuf>,
    reloads: VecDeque<Result<ReloadOutcome, CliError>>,
    reload_count: usize,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeAerospace {
    /// An installed AeroSpace 0.21.3-Beta.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeState {
                installed: true,
                running: true,
                apps: Vec::new(),
                monitors: Vec::new(),
                workspaces: Vec::new(),
                list_count: 0,
                version: AerospaceVersion {
                    major: 0,
                    minor: 21,
                    patch: 3,
                    pre_release: Some("Beta".to_owned()),
                },
                config_path: None,
                reloads: VecDeque::new(),
                reload_count: 0,
            }),
        }
    }

    /// No AeroSpace installed: every call returns [`CliError::BinaryNotFound`].
    #[must_use]
    pub fn missing() -> Self {
        let fake = Self::new();
        fake.state().installed = false;
        fake
    }

    /// Answers `version` with `version`.
    #[must_use]
    pub fn with_version(self, version: AerospaceVersion) -> Self {
        self.state().version = version;
        self
    }

    /// Sets whether the fake server can answer live-data calls.
    #[must_use]
    pub fn with_running(self, running: bool) -> Self {
        self.state().running = running;
        self
    }

    /// Answers `list_apps` with the supplied applications.
    #[must_use]
    pub fn with_apps(self, apps: Vec<App>) -> Self {
        self.state().apps = apps;
        self
    }

    /// Answers `list_monitors` with the supplied connected displays.
    #[must_use]
    pub fn with_monitors(self, monitors: Vec<Monitor>) -> Self {
        self.state().monitors = monitors;
        self
    }

    /// Answers `list_workspaces` with the supplied workspace names.
    #[must_use]
    pub fn with_workspaces(self, workspaces: Vec<WorkspaceName>) -> Self {
        self.state().workspaces = workspaces;
        self
    }

    /// Number of live-data list calls, including failures.
    pub fn list_count(&self) -> usize {
        self.state().list_count
    }

    fn list<T>(&self, read: impl FnOnce(&FakeState) -> Vec<T>) -> Result<Vec<T>, CliError> {
        let mut state = self.state();
        state.list_count += 1;
        if !state.installed {
            return Err(CliError::BinaryNotFound);
        }
        if !state.running {
            return Err(CliError::NotRunning);
        }
        Ok(read(&state))
    }

    /// Answers `config_path` with `path`, as a running server would.
    #[must_use]
    pub fn with_config_path(self, path: impl Into<PathBuf>) -> Self {
        self.state().config_path = Some(path.into());
        self
    }

    /// Scripts the result of the next unscripted `reload_config` call.
    pub fn queue_reload(&self, result: Result<ReloadOutcome, CliError>) {
        self.state().reloads.push_back(result);
    }

    /// How many times `reload_config` was called.
    pub fn reload_count(&self) -> usize {
        self.state().reload_count
    }

    fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for FakeAerospace {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Aerospace for FakeAerospace {
    fn list_apps(&self) -> Result<Vec<App>, CliError> {
        self.list(|state| state.apps.clone())
    }

    fn list_monitors(&self) -> Result<Vec<Monitor>, CliError> {
        self.list(|state| state.monitors.clone())
    }

    fn list_workspaces(&self) -> Result<Vec<WorkspaceName>, CliError> {
        self.list(|state| state.workspaces.clone())
    }

    fn version(&self) -> Result<AerospaceVersion, CliError> {
        let state = self.state();
        if !state.installed {
            return Err(CliError::BinaryNotFound);
        }
        Ok(state.version.clone())
    }

    fn config_path(&self) -> Result<Option<PathBuf>, CliError> {
        let state = self.state();
        if !state.installed {
            return Err(CliError::BinaryNotFound);
        }
        Ok(state.running.then(|| state.config_path.clone()).flatten())
    }

    fn reload_config(&self) -> Result<ReloadOutcome, CliError> {
        let mut state = self.state();
        state.reload_count += 1;
        if !state.installed {
            return Err(CliError::BinaryNotFound);
        }
        state.reloads.pop_front().unwrap_or_else(|| {
            Ok(if state.running {
                ReloadOutcome::Applied
            } else {
                ReloadOutcome::NotRunning
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn binary_on_path_wins_over_fallbacks() {
        let on_path = tempfile::tempdir().unwrap();
        let fallback = tempfile::tempdir().unwrap();
        fs::write(on_path.path().join("aerospace"), "").unwrap();
        let fallback_binary = fallback.path().join("aerospace");
        fs::write(&fallback_binary, "").unwrap();
        let path_variable =
            env::join_paths([fallback.path().join("empty"), on_path.path().into()]).unwrap();

        assert_eq!(
            find_binary(Some(&path_variable), &[&fallback_binary]),
            Some(on_path.path().join("aerospace"))
        );
    }

    #[test]
    fn fallbacks_are_tried_in_order_when_path_has_no_binary() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        let homebrew = directory.path().join("homebrew-aerospace");
        let local = directory.path().join("local-aerospace");
        fs::write(&homebrew, "").unwrap();
        fs::write(&local, "").unwrap();
        let path_variable = directory.path().as_os_str();

        assert_eq!(
            find_binary(Some(path_variable), &[&missing, &homebrew, &local]),
            Some(homebrew)
        );
        assert_eq!(find_binary(None, &[&missing]), None);
    }

    #[test]
    fn version_displays_like_aerospace() {
        let version = AerospaceVersion::parse("0.21.3-Beta").unwrap();
        assert_eq!(version.to_string(), "0.21.3-Beta");
        assert_eq!(
            AerospaceVersion::parse("1.2.3").unwrap().to_string(),
            "1.2.3"
        );
        for unknown in ["0.21", "0.21.3.1", "0.21.x", "0.21.3-", "v0.21.3"] {
            assert_eq!(AerospaceVersion::parse(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn catalogue_compatibility_compares_major_and_minor_only() {
        for (installed, newer) in [
            ("0.20.99", false),
            ("0.20.99-Beta", false),
            ("0.21.0", false),
            ("0.21.3-Beta", false),
            ("0.21.3", false),
            ("0.21.99", false),
            ("0.21.99-Beta", false),
            ("0.22.0-Beta", true),
            ("0.22.0", true),
            ("1.0.0-Beta", true),
            ("1.0.0", true),
        ] {
            assert_eq!(
                newer_than_catalogue(&AerospaceVersion::parse(installed).unwrap()),
                newer,
                "{installed}"
            );
        }
        assert!(AerospaceVersion::parse(crate::commands::CATALOGUE_VERSION).is_some());
    }

    #[test]
    fn fake_replays_queued_reloads_in_order_then_applies() {
        let fake = FakeAerospace::new();
        fake.queue_reload(Ok(ReloadOutcome::NotRunning));
        fake.queue_reload(Err(CliError::BinaryNotFound));

        assert_eq!(fake.reload_config().unwrap(), ReloadOutcome::NotRunning);
        assert!(matches!(
            fake.reload_config(),
            Err(CliError::BinaryNotFound)
        ));
        assert_eq!(fake.reload_config().unwrap(), ReloadOutcome::Applied);
        assert_eq!(fake.reload_count(), 3);
    }

    #[test]
    fn fake_answers_configured_version_and_path() {
        let fake = FakeAerospace::new()
            .with_version(AerospaceVersion::parse("0.20.0").unwrap())
            .with_config_path("/config/aerospace.toml");

        assert_eq!(fake.version().unwrap().to_string(), "0.20.0");
        assert_eq!(
            fake.config_path().unwrap(),
            Some(PathBuf::from("/config/aerospace.toml"))
        );
        assert_eq!(FakeAerospace::new().config_path().unwrap(), None);
    }

    #[test]
    fn missing_fake_fails_every_call_and_still_counts_reloads() {
        let fake = FakeAerospace::missing();

        assert!(matches!(fake.version(), Err(CliError::BinaryNotFound)));
        assert!(matches!(fake.config_path(), Err(CliError::BinaryNotFound)));
        assert!(matches!(
            fake.reload_config(),
            Err(CliError::BinaryNotFound)
        ));
        assert_eq!(fake.reload_count(), 1);
    }

    #[test]
    fn fake_is_usable_as_a_shared_trait_object() {
        let fake: std::sync::Arc<dyn Aerospace> = std::sync::Arc::new(FakeAerospace::new());
        let worker = std::thread::spawn({
            let fake = fake.clone();
            move || fake.reload_config()
        });
        assert_eq!(worker.join().unwrap().unwrap(), ReloadOutcome::Applied);
    }

    #[test]
    fn fake_live_data_is_configurable_and_reports_unavailable_servers() {
        let apps: Vec<App> =
            serde_json::from_str(include_str!("../tests/fixtures/cli/list-apps/stdout")).unwrap();
        let monitors: Vec<Monitor> =
            serde_json::from_str(include_str!("../tests/fixtures/cli/list-monitors/stdout"))
                .unwrap();
        let workspaces = vec![WorkspaceName::new("A").unwrap()];
        let fake = FakeAerospace::new()
            .with_apps(apps.clone())
            .with_monitors(monitors.clone())
            .with_workspaces(workspaces.clone());
        assert_eq!(fake.list_apps().unwrap(), apps);
        assert_eq!(fake.list_monitors().unwrap(), monitors);
        assert_eq!(fake.list_workspaces().unwrap(), workspaces);
        assert_eq!(fake.list_count(), 3);
        let fake = fake.with_running(false);
        assert!(matches!(fake.list_apps(), Err(CliError::NotRunning)));
        assert!(matches!(fake.list_monitors(), Err(CliError::NotRunning)));
        assert!(matches!(fake.list_workspaces(), Err(CliError::NotRunning)));
        assert_eq!(fake.reload_config().unwrap(), ReloadOutcome::NotRunning);
        let fake = FakeAerospace::missing();
        assert!(matches!(fake.list_apps(), Err(CliError::BinaryNotFound)));
        assert!(matches!(
            fake.list_monitors(),
            Err(CliError::BinaryNotFound)
        ));
        assert!(matches!(
            fake.list_workspaces(),
            Err(CliError::BinaryNotFound)
        ));
    }
}
