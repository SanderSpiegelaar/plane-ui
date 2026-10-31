#![cfg(unix)]
#![expect(
    clippy::unwrap_used,
    reason = "shared fixture setup runs outside #[test] functions; a failure should fail the test"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use planeui_core::aerospace::{
    Aerospace, AerospaceVersion, CliAerospace, CliError, ReloadMessage, ReloadOutcome,
};
use tempfile::TempDir;

// Every fake binary is written before any test spawns one: on Linux, a script that another
// thread's fork still holds open for writing fails to exec with ETXTBSY.
static FAKE_BINARIES: LazyLock<TempDir> = LazyLock::new(|| {
    let binaries = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli");
    for entry in fs::read_dir(&fixtures).unwrap() {
        let case = entry.unwrap().path();
        if !case.is_dir() {
            continue;
        }
        let name = case.file_name().unwrap().to_str().unwrap();
        let exit_code = fs::read_to_string(case.join("exit-code")).unwrap();
        let args = binaries.path().join(format!("{name}.args"));
        write_script(
            &binaries.path().join(name),
            &format!(
                "printf '%s\\n' \"$*\" > '{}'\ncat '{}'\ncat '{}' >&2\nexit {}\n",
                args.display(),
                case.join("stdout").display(),
                case.join("stderr").display(),
                exit_code.trim(),
            ),
        );
    }
    write_script(&binaries.path().join("hangs"), "sleep 5\n");
    write_script(
        &binaries.path().join("floods-stderr"),
        &format!(
            "head -c 1000000 /dev/zero | tr '\\0' x >&2\ncat '{}'\n",
            fixtures.join("version/stdout").display()
        ),
    );
    binaries
});

fn write_script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn replaying(case: &str) -> CliAerospace {
    CliAerospace::with_binary(FAKE_BINARIES.path().join(case))
}

fn args_passed_to(case: &str) -> String {
    fs::read_to_string(FAKE_BINARIES.path().join(format!("{case}.args"))).unwrap()
}

#[test]
fn version_comes_from_the_recorded_client_line() {
    let version = replaying("version").version().unwrap();

    assert_eq!(
        version,
        AerospaceVersion {
            major: 0,
            minor: 21,
            patch: 3,
            pre_release: Some("Beta".to_owned()),
        }
    );
    assert_eq!(version.to_string(), "0.21.3-Beta");
    assert_eq!(args_passed_to("version"), "--version\n");
}

#[test]
fn version_is_known_while_the_server_is_down() {
    let version = replaying("version-not-running-synthesized")
        .version()
        .unwrap();

    assert_eq!(version.to_string(), "0.21.3-Beta");
}

#[test]
fn unknown_version_output_is_unexpected() {
    let result = replaying("version-unknown-synthesized").version();

    assert!(
        matches!(result, Err(CliError::UnexpectedOutput { .. })),
        "{result:?}"
    );
}

#[test]
fn config_path_is_the_path_the_server_loaded() {
    let path = replaying("config-path").config_path().unwrap();

    assert_eq!(
        path,
        Some(PathBuf::from(
            "/Users/s.spiegelaar/.config/aerospace/aerospace.toml"
        ))
    );
    assert_eq!(args_passed_to("config-path"), "config --config-path\n");
}

#[test]
fn config_path_is_none_while_the_server_is_down() {
    let path = replaying("config-path-not-running-synthesized")
        .config_path()
        .unwrap();

    assert_eq!(path, None);
}

#[test]
fn clean_reload_is_applied_without_the_gui() {
    let outcome = replaying("reload-applied").reload_config().unwrap();

    assert_eq!(outcome, ReloadOutcome::Applied);
    assert_eq!(args_passed_to("reload-applied"), "reload-config --no-gui\n");
}

#[test]
fn reload_with_only_warnings_is_applied() {
    let outcome = replaying("reload-warnings-synthesized")
        .reload_config()
        .unwrap();

    assert_eq!(outcome, ReloadOutcome::Applied);
}

#[test]
fn reload_errors_keep_their_key_paths() {
    let outcome = replaying("reload-errors-synthesized")
        .reload_config()
        .unwrap();

    let message = |key_path: Option<&str>, message: &str| ReloadMessage {
        key_path: key_path.map(str::to_owned),
        message: message.to_owned(),
    };
    let fixture = |stream: &str| {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/cli/reload-errors-synthesized")
            .join(stream);
        fs::read_to_string(path).unwrap().trim_end().to_owned()
    };
    assert_eq!(
        outcome,
        ReloadOutcome::Failed {
            errors: vec![
                message(Some("gapz"), "Unknown top-level key"),
                message(Some("gaps.inner.vertikal"), "Unknown key"),
                message(
                    Some("default-root-container-layout"),
                    "Can't parse layout 'grid'"
                ),
                message(
                    None,
                    "The config contains:\n\
                     1. usage of 'split' command\n\
                     2. enable-normalization-flatten-containers = true\n\
                     These two settings don't play nicely together. 'split' command has no \
                     effect when enable-normalization-flatten-containers is disabled.\n\
                     \n\
                     My recommendation: keep the normalizations enabled, and prefer 'join-with' \
                     over 'split'."
                ),
            ],
            raw: format!("{}\n{}", fixture("stdout"), fixture("stderr")),
        }
    );
}

#[test]
fn reload_while_the_server_is_down_is_not_running() {
    let outcome = replaying("reload-not-running-synthesized")
        .reload_config()
        .unwrap();

    assert_eq!(outcome, ReloadOutcome::NotRunning);
}

#[test]
fn missing_binary_is_binary_not_found() {
    let aerospace = CliAerospace::with_binary(FAKE_BINARIES.path().join("does-not-exist"));

    assert!(matches!(aerospace.version(), Err(CliError::BinaryNotFound)));
    assert!(matches!(
        aerospace.reload_config(),
        Err(CliError::BinaryNotFound)
    ));
}

#[test]
fn hanging_binary_times_out_and_is_killed() {
    let aerospace = CliAerospace::with_binary(FAKE_BINARIES.path().join("hangs"))
        .with_timeout(Duration::from_millis(300));
    let started = Instant::now();

    let result = aerospace.reload_config();

    let elapsed = started.elapsed();
    assert!(
        matches!(
            &result,
            Err(CliError::Timeout { command, after })
                if command == "reload-config --no-gui" && *after == Duration::from_millis(300)
        ),
        "{result:?}"
    );
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
}

#[test]
fn output_larger_than_a_pipe_buffer_does_not_block() {
    let version = replaying("floods-stderr").version().unwrap();

    assert_eq!(version.to_string(), "0.21.3-Beta");
}
