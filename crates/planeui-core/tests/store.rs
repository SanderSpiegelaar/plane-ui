use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use planeui_core::store::{
    CheckError, ConfigStore, DEFAULT_CONFIG, DiskStamp, LoadError, LocateError, PathEnv,
    ResolvedConfig, SNAPSHOT_KEEP, SnapshotError, SnapshotId, SnapshotMeta, SnapshotStore,
    WriteError, create_default_config, resolve_config_path,
};
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

fn store_at(path: &Path) -> ConfigStore {
    ConfigStore::new(&ResolvedConfig::new_file(path.to_path_buf()))
}

fn directory_entries(directory: &Path) -> io::Result<Vec<String>> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// Root ignores directory permissions, so read-only tests are meaningless there.
fn read_only_is_enforced(directory: &Path) -> io::Result<bool> {
    let probe = directory.join("probe");
    match fs::write(&probe, "") {
        Ok(()) => {
            fs::remove_file(probe)?;
            Ok(false)
        }
        Err(_) => Ok(true),
    }
}

#[test]
fn load_returns_the_exact_text_and_its_stamp() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "a = 1\r\nb = 2\r\n").unwrap();

    let loaded = store_at(&path).load().unwrap();

    assert_eq!(loaded.text, "a = 1\r\nb = 2\r\n");
    assert_eq!(loaded.stamp, DiskStamp::of(b"a = 1\r\nb = 2\r\n"));
}

#[test]
fn load_reports_a_missing_file() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");

    let error = store_at(&path).load().unwrap_err();

    assert!(
        matches!(&error, LoadError::NotFound { path: missing } if *missing == path),
        "{error:?}"
    );
}

#[test]
fn load_rejects_non_utf8_bytes() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    fs::write(&path, [0xff, 0xfe]).unwrap();

    let error = store_at(&path).load().unwrap_err();

    assert!(matches!(error, LoadError::NotUtf8 { .. }), "{error:?}");
}

#[test]
fn write_atomic_replaces_the_content_byte_for_byte() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "old = true\n").unwrap();
    let store = store_at(&path);

    let stamp = store.write_atomic(b"new = true\r\n").unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"new = true\r\n");
    assert_eq!(stamp, store.load().unwrap().stamp);
    assert_eq!(directory_entries(&root).unwrap(), ["aerospace.toml"]);
}

#[test]
fn write_atomic_keeps_the_file_permissions() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "old = true\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    store_at(&path).write_atomic(b"new = true\n").unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn write_atomic_through_a_symlink_changes_the_target_and_keeps_the_link() {
    let (_dir, root) = temp_root().unwrap();
    let real = root.join("dotfiles/aerospace.toml");
    write(&real, "old = true\n").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    let link = root.join("home/.aerospace.toml");
    symlink(&real, &link).unwrap();
    let env = env_without_xdg(&root);

    for store in [
        ConfigStore::new(&resolve_config_path(&env, None).unwrap()),
        store_at(&link),
    ] {
        store.write_atomic(b"new = true\n").unwrap();

        assert!(link.symlink_metadata().unwrap().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), real);
        assert_eq!(fs::read(&real).unwrap(), b"new = true\n");
        let mode = fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
        fs::write(&real, "old = true\n").unwrap();
    }
    assert_eq!(
        directory_entries(&root.join("home")).unwrap(),
        [".aerospace.toml"]
    );
    assert_eq!(
        directory_entries(&root.join("dotfiles")).unwrap(),
        ["aerospace.toml"]
    );
}

#[test]
fn write_atomic_creates_a_missing_file_and_its_directory() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let suggested = env.xdg_config_path();
    assert!(matches!(
        resolve_config_path(&env, None),
        Err(LocateError::NotFound { suggested: path }) if path == suggested
    ));

    let store = ConfigStore::new(&ResolvedConfig::new_file(suggested.clone()));
    store.write_atomic(b"start-at-login = false\n").unwrap();

    assert_eq!(fs::read(&suggested).unwrap(), b"start-at-login = false\n");
    assert_eq!(
        resolve_config_path(&env, None).unwrap().target_path,
        suggested
    );
}

#[test]
fn check_unchanged_accepts_the_file_as_loaded() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "a = 1\n").unwrap();
    let store = store_at(&path);
    let loaded = store.load().unwrap();

    store.check_unchanged(&loaded.stamp).unwrap();
}

#[test]
fn check_unchanged_detects_an_external_edit_between_load_and_write() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "a = 1\n").unwrap();
    let store = store_at(&path);
    let loaded = store.load().unwrap();

    fs::write(&path, "a = 2\n").unwrap();
    let error = store.check_unchanged(&loaded.stamp).unwrap_err();

    let CheckError::Changed { current } = error else {
        unreachable!("expected Changed, got {error:?}");
    };
    assert_eq!(current.text, "a = 2\n");
    assert_eq!(current.stamp, DiskStamp::of(b"a = 2\n"));
}

#[test]
fn check_unchanged_follows_our_own_writes() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "a = 1\n").unwrap();
    let store = store_at(&path);
    let loaded = store.load().unwrap();

    let written = store.write_atomic(b"a = 2\n").unwrap();

    assert!(matches!(
        store.check_unchanged(&loaded.stamp),
        Err(CheckError::Changed { .. })
    ));
    store.check_unchanged(&written).unwrap();
}

#[test]
fn check_unchanged_reports_a_deleted_file() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "a = 1\n").unwrap();
    let store = store_at(&path);
    let loaded = store.load().unwrap();

    fs::remove_file(&path).unwrap();
    let error = store.check_unchanged(&loaded.stamp).unwrap_err();

    assert!(
        matches!(error, CheckError::Load(LoadError::NotFound { .. })),
        "{error:?}"
    );
}

#[test]
fn write_atomic_into_a_read_only_directory_fails_and_leaves_no_trace() {
    let (_dir, root) = temp_root().unwrap();
    let directory = root.join("config");
    let path = directory.join("aerospace.toml");
    write(&path, "old = true\n").unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    if !read_only_is_enforced(&directory).unwrap() {
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let result = store_at(&path).write_atomic(b"new = true\n");
    let entries = directory_entries(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

    let error = result.unwrap_err();
    assert!(matches!(error, WriteError::CreateTemp { .. }), "{error:?}");
    assert!(error.to_string().contains(&directory.display().to_string()));
    assert_eq!(fs::read(&path).unwrap(), b"old = true\n");
    assert_eq!(entries, ["aerospace.toml"]);
}

#[test]
fn a_failed_rename_removes_the_temp_file() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path.join("occupied"), "").unwrap();

    let error = store_at(&path).write_atomic(b"new = true\n").unwrap_err();

    assert!(
        matches!(&error, WriteError::Rename { path: target, .. } if *target == path),
        "{error:?}"
    );
    assert_eq!(directory_entries(&root).unwrap(), ["aerospace.toml"]);
    assert_eq!(directory_entries(&path).unwrap(), ["occupied"]);
}

#[test]
fn the_config_store_can_move_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ConfigStore>();
    assert_send_sync::<SnapshotStore>();
}

/// 2026-10-31T17:15:03.042Z.
fn base_time() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_793_466_903_042)
}

fn seconds_after_base(seconds: u64) -> SystemTime {
    base_time() + Duration::from_secs(seconds)
}

fn listed_ids(store: &SnapshotStore) -> Result<Vec<String>, SnapshotError> {
    Ok(store
        .list()?
        .into_iter()
        .map(|snapshot| snapshot.id.to_string())
        .collect())
}

#[test]
fn the_default_snapshot_dir_is_in_planeui_application_data() {
    let dir = SnapshotStore::default_dir().unwrap();

    assert!(dir.ends_with("PlaneUI/snapshots"), "{}", dir.display());
}

#[test]
fn a_snapshot_creates_its_directory_and_keeps_the_bytes_exactly() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.join("support/PlaneUI/snapshots"));
    let bytes = b"a = 1\r\n# \xff not utf-8\n";

    let id = store.take_at(bytes, base_time()).unwrap();

    assert_eq!(id.to_string(), "20261031T171503.042Z");
    assert_eq!(store.read(&id).unwrap(), bytes);
    assert_eq!(
        directory_entries(store.dir()).unwrap(),
        ["20261031T171503.042Z.toml"]
    );
}

#[test]
fn take_uses_the_current_time() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.clone());
    let before = SystemTime::now() - Duration::from_millis(1);

    let id = store.take(b"a = 1\n").unwrap();

    let snapshot = store.list().unwrap().remove(0);
    assert_eq!(snapshot.id, id);
    assert!(snapshot.taken_at >= before && snapshot.taken_at <= SystemTime::now());
}

#[test]
fn list_is_newest_first_with_time_and_size() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.clone());
    let older = store.take_at(b"1", seconds_after_base(0)).unwrap();
    let newer = store.take_at(b"22", seconds_after_base(1)).unwrap();

    let snapshots = store.list().unwrap();

    assert_eq!(
        snapshots,
        [
            SnapshotMeta {
                id: newer,
                taken_at: seconds_after_base(1),
                size: 2,
            },
            SnapshotMeta {
                id: older,
                taken_at: seconds_after_base(0),
                size: 1,
            },
        ]
    );
}

#[test]
fn listing_a_missing_directory_is_empty() {
    let (_dir, root) = temp_root().unwrap();

    let snapshots = SnapshotStore::new(root.join("missing")).list().unwrap();

    assert_eq!(snapshots, []);
}

#[test]
fn the_eleventh_snapshot_deletes_the_oldest() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.clone());
    let ids: Vec<SnapshotId> = (0..=10)
        .map(|second| {
            store
                .take_at(second.to_string().as_bytes(), seconds_after_base(second))
                .unwrap()
        })
        .collect();

    let snapshots = store.list().unwrap();

    assert_eq!(snapshots.len(), SNAPSHOT_KEEP);
    let expected: Vec<SnapshotId> = ids.iter().skip(1).rev().cloned().collect();
    let listed: Vec<SnapshotId> = snapshots.into_iter().map(|snapshot| snapshot.id).collect();
    assert_eq!(listed, expected);
    assert!(store.read(ids.first().unwrap()).is_err());
    assert_eq!(directory_entries(&root).unwrap().len(), SNAPSHOT_KEEP);
}

#[test]
fn stray_files_are_neither_counted_nor_deleted() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.clone());
    let strays = [
        "20000101T000000.000Z.toml.bak",
        "20000101T000000.000Z.txt",
        "2000-01-01.toml",
        "20000230T000000.000Z.toml",
        "20000101T000000.000Z-01.toml",
        "notes.md",
    ];
    for stray in strays {
        write(&root.join(stray), "keep me").unwrap();
    }
    fs::create_dir(root.join("20000101T000000.000Z.toml")).unwrap();

    for second in 0..12 {
        store.take_at(b"x", seconds_after_base(second)).unwrap();
    }

    assert_eq!(store.list().unwrap().len(), SNAPSHOT_KEEP);
    for stray in strays {
        assert_eq!(fs::read_to_string(root.join(stray)).unwrap(), "keep me");
    }
    assert!(root.join("20000101T000000.000Z.toml").is_dir());
}

#[test]
fn same_millisecond_snapshots_get_a_suffix_and_sort_in_order_taken() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root);
    let ids: Vec<String> = (0..12)
        .map(|index| {
            store
                .take_at(format!("{index}").as_bytes(), base_time())
                .unwrap()
                .to_string()
        })
        .collect();

    let expected_names: Vec<String> = std::iter::once("20261031T171503.042Z".to_owned())
        .chain((1..12).map(|sequence| format!("20261031T171503.042Z-{sequence}")))
        .collect();
    assert_eq!(ids, expected_names);
    let expected: Vec<String> = ids.iter().skip(2).rev().cloned().collect();
    assert_eq!(listed_ids(&store).unwrap(), expected);
    let newest = store.list().unwrap().remove(0);
    assert_eq!(store.read(&newest.id).unwrap(), b"11");
}

#[test]
fn a_snapshot_taken_after_the_clock_went_back_still_sorts_newest() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root);
    for second in 10..20 {
        store.take_at(b"x", seconds_after_base(second)).unwrap();
    }

    let id = store.take_at(b"latest", seconds_after_base(0)).unwrap();

    assert_eq!(id.to_string(), "20261031T171522.042Z-1");
    assert_eq!(store.read(&id).unwrap(), b"latest");
    let listed = listed_ids(&store).unwrap();
    assert_eq!(listed.len(), SNAPSHOT_KEEP);
    assert_eq!(listed.first(), Some(&id.to_string()));
    assert!(!listed.contains(&"20261031T171513.042Z".to_owned()));
}

#[test]
fn a_time_before_1970_cannot_be_named() {
    let (_dir, root) = temp_root().unwrap();
    let store = SnapshotStore::new(root.clone());

    let error = store
        .take_at(b"x", UNIX_EPOCH - Duration::from_secs(1))
        .unwrap_err();

    assert!(
        matches!(error, SnapshotError::UnsupportedTime { .. }),
        "{error:?}"
    );
    assert_eq!(directory_entries(&root).unwrap(), [] as [String; 0]);
}

#[test]
fn a_snapshot_into_a_read_only_directory_fails() {
    let (_dir, root) = temp_root().unwrap();
    let directory = root.join("snapshots");
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o555)).unwrap();
    if !read_only_is_enforced(&directory).unwrap() {
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let result = SnapshotStore::new(directory.clone()).take_at(b"x", base_time());
    let entries = directory_entries(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

    let error = result.unwrap_err();
    assert!(matches!(error, SnapshotError::Write { .. }), "{error:?}");
    assert!(error.to_string().contains(&directory.display().to_string()));
    assert_eq!(entries, [] as [String; 0]);
}

#[test]
fn a_snapshot_directory_that_cannot_be_created_fails() {
    let (_dir, root) = temp_root().unwrap();
    let parent = root.join("support");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    if !read_only_is_enforced(&parent).unwrap() {
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let result = SnapshotStore::new(parent.join("snapshots")).take_at(b"x", base_time());
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

    let error = result.unwrap_err();
    assert!(
        matches!(error, SnapshotError::CreateDir { .. }),
        "{error:?}"
    );
}

#[test]
fn creates_the_default_config_byte_for_byte_where_none_exists() {
    let (_dir, root) = temp_root().unwrap();
    let env = env_without_xdg(&root);
    let suggested = env.xdg_config_path();
    assert!(matches!(
        resolve_config_path(&env, None),
        Err(LocateError::NotFound { suggested: path }) if path == suggested
    ));

    create_default_config(&suggested).unwrap();

    let fixture = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aerospace-default.toml"),
    )
    .unwrap();
    assert_eq!(fs::read(&suggested).unwrap(), fixture);
    assert_eq!(DEFAULT_CONFIG.as_bytes(), fixture);
    assert_eq!(
        resolve_config_path(&env, None).unwrap().display_path,
        suggested
    );
}

#[test]
fn creating_the_default_config_keeps_a_file_that_appeared_meanwhile() {
    let (_dir, root) = temp_root().unwrap();
    let path = root.join("aerospace.toml");
    write(&path, "# mine\n").unwrap();

    create_default_config(&path).unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "# mine\n");
}
