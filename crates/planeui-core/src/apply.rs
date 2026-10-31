//! The change pipeline: validate, check the disk, snapshot, write atomically, reload. One
//! synchronous function so it is testable without a UI; the app runs it off the UI thread.

use std::sync::Arc;
use std::time::Instant;

use crate::aerospace::{Aerospace, CliError, ReloadOutcome};
use crate::store::{
    ConfigStore, DiskStamp, LoadError, Loaded, SnapshotError, SnapshotStore, WriteError,
};
use crate::validate::{ValidationReport, validate_text};

/// What the pipeline writes to and reloads.
#[derive(Debug, Clone)]
pub struct ApplyContext {
    /// The config file.
    pub store: ConfigStore,
    /// Where pre-write copies go.
    pub snapshots: SnapshotStore,
    /// The AeroSpace CLI (or a fake in tests).
    pub aerospace: Arc<dyn Aerospace>,
}

/// How one apply ended. Every variant except [`ApplyOutcome::Written`] left the disk untouched.
#[derive(Debug)]
pub enum ApplyOutcome {
    /// Validation found errors; nothing was written and no snapshot taken.
    Rejected(ValidationReport),
    /// The file changed on disk since it was loaded; it holds `current` now.
    ExternalChange {
        /// What is on disk now.
        current: Box<Loaded>,
    },
    /// Reading the file for the external-change check failed.
    CheckFailed(LoadError),
    /// The pre-write snapshot failed, so the write was skipped.
    SnapshotFailed(SnapshotError),
    /// The atomic write failed; the old file is intact.
    WriteFailed(WriteError),
    /// The file was written. `reload` says whether AeroSpace took it.
    Written {
        /// The reload result, or why the CLI could not be run.
        reload: Result<ReloadOutcome, CliError>,
        /// The stamp of the bytes just written, for the next external-change check.
        new_stamp: DiskStamp,
    },
}

/// Writes `new_text` if it is valid and the disk still holds what `stamp` describes, then
/// reloads AeroSpace. Aborts at the first failing step.
#[must_use]
pub fn apply(context: &ApplyContext, stamp: &DiskStamp, new_text: &str) -> ApplyOutcome {
    let started = Instant::now();
    let step = |name: &str| log::debug!("apply: {name} done after {:?}", started.elapsed());

    match validate_text(new_text) {
        Ok((_, _, report)) if !report.blocks_write() => {}
        Ok((_, _, report)) | Err(report) => return ApplyOutcome::Rejected(report),
    }
    step("validate");

    // One read serves both the external-change check and the snapshot, so the snapshot holds
    // exactly the bytes that were verified.
    let current = match context.store.load() {
        Ok(current) if current.stamp == *stamp => current,
        Ok(current) => {
            return ApplyOutcome::ExternalChange {
                current: Box::new(current),
            };
        }
        Err(error) => return ApplyOutcome::CheckFailed(error),
    };
    step("external-change check");

    if let Err(error) = context.snapshots.take(current.text.as_bytes()) {
        return ApplyOutcome::SnapshotFailed(error);
    }
    step("snapshot");

    let new_stamp = match context.store.write_atomic(new_text.as_bytes()) {
        Ok(new_stamp) => new_stamp,
        Err(error) => return ApplyOutcome::WriteFailed(error),
    };
    step("write");

    let reload = context.aerospace.reload_config();
    step("reload");
    ApplyOutcome::Written { reload, new_stamp }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::aerospace::{FakeAerospace, ReloadMessage};
    use crate::store::ResolvedConfig;

    const ORIGINAL: &str = "accordion-padding = 30 # px\n";
    const EDITED: &str = "accordion-padding = 12 # px\n";

    struct Setup {
        _directory: tempfile::TempDir,
        config: PathBuf,
        snapshots: PathBuf,
        aerospace: Arc<FakeAerospace>,
        context: ApplyContext,
        stamp: DiskStamp,
    }

    fn setup() -> Setup {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("aerospace.toml");
        fs::write(&config, ORIGINAL).unwrap();
        let snapshots = directory.path().join("snapshots");
        let resolved = ResolvedConfig {
            display_path: config.clone(),
            target_path: config.clone(),
        };
        let store = ConfigStore::new(&resolved);
        let stamp = store.load().unwrap().stamp;
        let aerospace = Arc::new(FakeAerospace::new());
        let context = ApplyContext {
            store,
            snapshots: SnapshotStore::new(snapshots.clone()),
            aerospace: aerospace.clone(),
        };
        Setup {
            _directory: directory,
            config,
            snapshots,
            aerospace,
            context,
            stamp,
        }
    }

    fn snapshot_count(directory: &Path) -> usize {
        fs::read_dir(directory).map_or(0, Iterator::count)
    }

    fn assert_untouched(setup: &Setup, disk: &str) {
        assert_eq!(fs::read_to_string(&setup.config).unwrap(), disk);
        assert_eq!(snapshot_count(&setup.snapshots), 0, "no snapshot taken");
        assert_eq!(setup.aerospace.reload_count(), 0, "no reload");
    }

    #[test]
    fn success_snapshots_writes_and_reloads() {
        let setup = setup();
        let outcome = apply(&setup.context, &setup.stamp, EDITED);
        assert!(
            matches!(
                &outcome,
                ApplyOutcome::Written { reload: Ok(ReloadOutcome::Applied), new_stamp }
                    if *new_stamp == DiskStamp::of(EDITED.as_bytes())
            ),
            "{outcome:?}"
        );
        assert_eq!(fs::read_to_string(&setup.config).unwrap(), EDITED);
        let snapshots = setup.context.snapshots.list().unwrap();
        assert_eq!(snapshots.len(), 1);
        let snapshot = setup
            .context
            .snapshots
            .read(&snapshots.first().unwrap().id)
            .unwrap();
        assert_eq!(snapshot, ORIGINAL.as_bytes());
        assert_eq!(setup.aerospace.reload_count(), 1);
    }

    #[test]
    fn validation_error_leaves_disk_untouched() {
        let setup = setup();
        for invalid in ["accordion-padding = = 1\n", "accordion-padding = 'wide'\n"] {
            let outcome = apply(&setup.context, &setup.stamp, invalid);
            assert!(matches!(outcome, ApplyOutcome::Rejected(ref report) if report.blocks_write()));
            assert_untouched(&setup, ORIGINAL);
        }
    }

    #[test]
    fn external_change_leaves_disk_untouched() {
        let setup = setup();
        let external = "accordion-padding = 99\n";
        fs::write(&setup.config, external).unwrap();
        let outcome = apply(&setup.context, &setup.stamp, EDITED);
        assert!(
            matches!(&outcome, ApplyOutcome::ExternalChange { current } if current.text == external),
            "{outcome:?}"
        );
        assert_untouched(&setup, external);
    }

    #[test]
    fn snapshot_failure_blocks_the_write() {
        let setup = setup();
        fs::write(
            &setup.snapshots,
            "a file where the snapshot directory should be",
        )
        .unwrap();
        let outcome = apply(&setup.context, &setup.stamp, EDITED);
        assert!(
            matches!(outcome, ApplyOutcome::SnapshotFailed(_)),
            "{outcome:?}"
        );
        assert_eq!(fs::read_to_string(&setup.config).unwrap(), ORIGINAL);
        assert_eq!(setup.aerospace.reload_count(), 0);
    }

    #[test]
    fn reload_failure_still_reports_written() {
        let setup = setup();
        let failed = ReloadOutcome::Failed {
            errors: vec![ReloadMessage {
                key_path: Some("accordion-padding".to_owned()),
                message: "bad value".to_owned(),
            }],
            raw: "[ERROR] accordion-padding: bad value".to_owned(),
        };
        setup.aerospace.queue_reload(Ok(failed.clone()));
        let outcome = apply(&setup.context, &setup.stamp, EDITED);
        assert!(
            matches!(outcome, ApplyOutcome::Written { reload: Ok(ref reload), .. } if *reload == failed),
            "{outcome:?}"
        );
        assert_eq!(fs::read_to_string(&setup.config).unwrap(), EDITED);
    }

    #[test]
    fn aerospace_not_running_reports_written() {
        let setup = setup();
        setup.aerospace.queue_reload(Ok(ReloadOutcome::NotRunning));
        let outcome = apply(&setup.context, &setup.stamp, EDITED);
        assert!(
            matches!(
                outcome,
                ApplyOutcome::Written {
                    reload: Ok(ReloadOutcome::NotRunning),
                    ..
                }
            ),
            "{outcome:?}"
        );
    }
}
