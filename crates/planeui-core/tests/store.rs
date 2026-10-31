use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use planeui_core::store::{LocateError, PathEnv, ResolvedConfig, resolve_config_path};
use tempfile::TempDir;

/// A temp dir with symlinks in its own path resolved (macOS puts temp dirs under `/var`, a link
/// to `/private/var`), so resolved paths compare equal to paths built from it.
fn temp_root() -> io::Result<(TempDir, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    Ok((dir, root))
}

fn write(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn env_without_xdg(root: &Path) -> PathEnv {
    PathEnv {
        home: root.join("home"),
        xdg_config_home: None,
    }
}

#[test]
fn finds_the_home_config() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let home_config = root.join("home/.aerospace.toml");
    write(&home_config, "").unwrap();

    let resolved = resolve_config_path(&env, None).unwrap();

    assert_eq!(resolved, ResolvedConfig::new_file(home_config));
}

#[test]
fn finds_the_xdg_config_under_dot_config_by_default() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let xdg_config = root.join("home/.config/aerospace/aerospace.toml");
    write(&xdg_config, "").unwrap();

    let resolved = resolve_config_path(&env, None).unwrap();

    assert_eq!(resolved, ResolvedConfig::new_file(xdg_config));
}

#[test]
fn finds_the_xdg_config_under_xdg_config_home() {
    let (_dir, root) = temp_root().unwrap();
    let env = PathEnv {
        home: root.join("home"),
        xdg_config_home: Some(root.join("xdg")),
    };
    let xdg_config = root.join("xdg/aerospace/aerospace.toml");
    write(&xdg_config, "").unwrap();
    write(&root.join("home/.config/aerospace/aerospace.toml"), "").unwrap();

    let resolved = resolve_config_path(&env, None).unwrap();

    assert_eq!(resolved, ResolvedConfig::new_file(xdg_config));
}

#[test]
fn refuses_when_both_configs_exist() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let home_config = root.join("home/.aerospace.toml");
    let xdg_config = root.join("home/.config/aerospace/aerospace.toml");
    write(&home_config, "").unwrap();
    write(&xdg_config, "").unwrap();

    let error = resolve_config_path(&env, None).unwrap_err();

    assert!(
        matches!(&error, LocateError::Ambiguous { home, xdg }
            if *home == home_config && *xdg == xdg_config),
        "{error:?}"
    );
    assert!(error.to_string().contains("ambiguous"));
}

#[test]
fn suggests_the_xdg_path_when_no_config_exists() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    fs::create_dir_all(root.join("home")).unwrap();

    let error = resolve_config_path(&env, None).unwrap_err();

    assert!(
        matches!(&error, LocateError::NotFound { suggested }
            if *suggested == root.join("home/.config/aerospace/aerospace.toml")),
        "{error:?}"
    );
}

#[test]
fn resolves_a_symlink_into_a_dotfiles_dir_and_keeps_the_link_for_display() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let real = root.join("dotfiles/aerospace.toml");
    write(&real, "").unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    let link = root.join("home/.aerospace.toml");
    symlink(&real, &link).unwrap();

    let resolved = resolve_config_path(&env, None).unwrap();

    assert_eq!(resolved.display_path, link);
    assert_eq!(resolved.target_path, real);
    assert!(link.symlink_metadata().unwrap().is_symlink());
}

#[test]
fn a_dangling_symlink_counts_as_missing() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    fs::create_dir_all(root.join("home")).unwrap();
    symlink(root.join("gone.toml"), root.join("home/.aerospace.toml")).unwrap();

    let error = resolve_config_path(&env, None).unwrap_err();

    assert!(matches!(error, LocateError::NotFound { .. }), "{error:?}");
}

#[test]
fn the_server_path_overrides_both_default_locations() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    write(&root.join("home/.aerospace.toml"), "").unwrap();
    write(&root.join("home/.config/aerospace/aerospace.toml"), "").unwrap();
    let custom = root.join("custom/aerospace.toml");
    write(&custom, "").unwrap();

    let resolved = resolve_config_path(&env, Some(custom.clone())).unwrap();

    assert_eq!(resolved, ResolvedConfig::new_file(custom));
}

#[test]
fn a_missing_server_path_is_not_found() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let custom = root.join("custom/aerospace.toml");

    let error = resolve_config_path(&env, Some(custom.clone())).unwrap_err();

    assert!(
        matches!(&error, LocateError::NotFound { suggested } if *suggested == custom),
        "{error:?}"
    );
}
