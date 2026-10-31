//! Byte-for-byte copies of the config, named by UTC timestamp, newest 10 kept.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How many snapshots are kept; older ones are deleted after each new snapshot.
pub const SNAPSHOT_KEEP: usize = 10;

/// Upper bound on names tried when they are taken, so a broken directory cannot loop forever.
const MAX_NAME_ATTEMPTS: usize = 1000;

const SNAPSHOT_EXTENSION: &str = ".toml";

/// Byte-for-byte copies of the config taken before each write.
///
/// Files are named `YYYYMMDDTHHMMSS.mmmZ.toml` in UTC, with `-N` before the extension when
/// a snapshot would not otherwise sort after the newest one. Files that do not match that
/// scheme are ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotStore {
    dir: PathBuf,
}

/// Names one snapshot: the file stem, such as `20261031T171503.042Z`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// The file stem.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn file_name(&self) -> String {
        format!("{}{SNAPSHOT_EXTENSION}", self.0)
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A snapshot on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Which snapshot.
    pub id: SnapshotId,
    /// When it was taken, to the millisecond, from its file name.
    pub taken_at: SystemTime,
    /// Its size in bytes.
    pub size: u64,
}

/// Why a snapshot could not be taken, listed, or read.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The clock is before 1970 or after 9999, which the naming scheme cannot express.
    #[error("cannot name a snapshot for the system time {at:?}")]
    UnsupportedTime {
        /// The time the snapshot was for.
        at: SystemTime,
    },
    /// The snapshot directory could not be created.
    #[error("could not create {path}: {source}")]
    CreateDir {
        /// The snapshot directory.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The snapshot file could not be created or written.
    #[error("could not write snapshot {path}: {source}")]
    Write {
        /// The snapshot file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The snapshot directory could not be listed.
    #[error("could not list snapshots in {path}: {source}")]
    List {
        /// The snapshot directory.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// A snapshot could not be read.
    #[error("could not read snapshot {path}: {source}")]
    Read {
        /// The snapshot file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
}

impl SnapshotStore {
    /// A store keeping snapshots in `dir`, which is created on first use.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `~/Library/Application Support/PlaneUI/snapshots` on macOS.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        dirs::data_dir().map(|data| data.join("PlaneUI").join("snapshots"))
    }

    /// The directory snapshots live in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Snapshots `current_bytes` now. See [`SnapshotStore::take_at`].
    ///
    /// # Errors
    ///
    /// As [`SnapshotStore::take_at`].
    pub fn take(&self, current_bytes: &[u8]) -> Result<SnapshotId, SnapshotError> {
        self.take_at(current_bytes, SystemTime::now())
    }

    /// Writes `current_bytes` as a snapshot named for `at`, then deletes the oldest snapshots
    /// beyond [`SNAPSHOT_KEEP`].
    ///
    /// The new snapshot always sorts newest: when `at` is not after the newest existing
    /// snapshot (same millisecond, or the clock went back), it takes that snapshot's time with
    /// the next `-N` suffix.
    ///
    /// # Errors
    ///
    /// When the snapshot is not safely on disk; the caller must not write the config then.
    /// Failing to delete old snapshots is logged, not returned.
    pub fn take_at(
        &self,
        current_bytes: &[u8],
        at: SystemTime,
    ) -> Result<SnapshotId, SnapshotError> {
        let requested = at
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .map(|millis| SnapshotName::new(millis, 0))
            .filter(|name| name.id().is_some())
            .ok_or(SnapshotError::UnsupportedTime { at })?;
        fs::create_dir_all(&self.dir).map_err(|source| SnapshotError::CreateDir {
            path: self.dir.clone(),
            source,
        })?;
        let first = match self.entries()?.first() {
            Some((newest, _)) if *newest >= requested => newest.successor(),
            _ => requested,
        };
        let id = self.write_new(current_bytes, first)?;
        self.rotate();
        Ok(id)
    }

    /// All snapshots, newest first.
    ///
    /// # Errors
    ///
    /// When the directory exists but cannot be listed.
    pub fn list(&self) -> Result<Vec<SnapshotMeta>, SnapshotError> {
        Ok(self.entries()?.into_iter().map(|(_, meta)| meta).collect())
    }

    /// The bytes of snapshot `id`.
    ///
    /// # Errors
    ///
    /// When the snapshot is missing or unreadable.
    pub fn read(&self, id: &SnapshotId) -> Result<Vec<u8>, SnapshotError> {
        let path = self.dir.join(id.file_name());
        fs::read(&path).map_err(|source| SnapshotError::Read { path, source })
    }

    /// Snapshot files with their parsed names, newest first.
    fn entries(&self) -> Result<Vec<(SnapshotName, SnapshotMeta)>, SnapshotError> {
        let list_error = |source| SnapshotError::List {
            path: self.dir.clone(),
            source,
        };
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(list_error(source)),
        };
        let mut snapshots = Vec::new();
        for entry in entries {
            let entry = entry.map_err(list_error)?;
            let Some(name) = entry
                .file_name()
                .to_str()
                .and_then(|file_name| file_name.strip_suffix(SNAPSHOT_EXTENSION))
                .and_then(SnapshotName::parse)
            else {
                continue;
            };
            let metadata = entry.metadata().map_err(list_error)?;
            if !metadata.is_file() {
                continue;
            }
            let Some(id) = name.id() else { continue };
            snapshots.push((
                name,
                SnapshotMeta {
                    id,
                    taken_at: name.taken_at(),
                    size: metadata.len(),
                },
            ));
        }
        snapshots.sort_by_key(|(name, _)| std::cmp::Reverse(*name));
        Ok(snapshots)
    }

    fn write_new(&self, bytes: &[u8], first: SnapshotName) -> Result<SnapshotId, SnapshotError> {
        let sequences = (first.sequence..=u32::MAX).take(MAX_NAME_ATTEMPTS);
        for sequence in sequences {
            let Some(id) = SnapshotName::new(first.millis, sequence).id() else {
                break;
            };
            let path = self.dir.join(id.file_name());
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(SnapshotError::Write { path, source }),
            };
            let written = file.write_all(bytes).and_then(|()| file.sync_all());
            drop(file);
            return match written {
                Ok(()) => Ok(id),
                Err(source) => {
                    if let Err(error) = fs::remove_file(&path) {
                        log::warn!(
                            "could not remove partial snapshot {}: {error}",
                            path.display()
                        );
                    }
                    Err(SnapshotError::Write { path, source })
                }
            };
        }
        Err(SnapshotError::Write {
            path: self.dir.clone(),
            source: io::Error::from(io::ErrorKind::AlreadyExists),
        })
    }

    fn rotate(&self) {
        let snapshots = match self.list() {
            Ok(snapshots) => snapshots,
            Err(error) => {
                log::warn!("could not prune old snapshots: {error}");
                return;
            }
        };
        for snapshot in snapshots.iter().skip(SNAPSHOT_KEEP) {
            let path = self.dir.join(snapshot.id.file_name());
            if let Err(error) = fs::remove_file(&path) {
                log::warn!("could not delete old snapshot {}: {error}", path.display());
            }
        }
    }
}

/// The parsed form of a snapshot file stem; orders oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SnapshotName {
    millis: u64,
    /// 0 for the first snapshot in a millisecond, then the `-N` suffix.
    sequence: u32,
}

const MILLIS_PER_DAY: u64 = 86_400_000;

impl SnapshotName {
    fn new(millis: u64, sequence: u32) -> Self {
        Self { millis, sequence }
    }

    fn successor(self) -> Self {
        Self::new(self.millis, self.sequence.saturating_add(1))
    }

    fn taken_at(self) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(self.millis)
    }

    /// `None` past the year 9999, where the fixed-width stem would stop sorting.
    fn id(self) -> Option<SnapshotId> {
        let days = i64::try_from(self.millis / MILLIS_PER_DAY).ok()?;
        let (year, month, day) = civil_from_days(days);
        if year > 9999 {
            return None;
        }
        let in_day = self.millis % MILLIS_PER_DAY;
        let (hour, minute) = (in_day / 3_600_000, in_day / 60_000 % 60);
        let (second, milli) = (in_day / 1000 % 60, in_day % 1000);
        let suffix = match self.sequence {
            0 => String::new(),
            sequence => format!("-{sequence}"),
        };
        Some(SnapshotId(format!(
            "{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}.{milli:03}Z{suffix}"
        )))
    }

    /// Accepts exactly the stems [`SnapshotName::id`] produces.
    fn parse(stem: &str) -> Option<Self> {
        let number = |range: std::ops::Range<usize>| -> Option<u64> {
            let digits = stem.get(range)?;
            if digits.bytes().all(|byte| byte.is_ascii_digit()) {
                digits.parse().ok()
            } else {
                None
            }
        };
        let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
        let (hour, minute, second) = (number(9..11)?, number(11..13)?, number(13..15)?);
        let milli = number(16..19)?;
        let sequence = match stem.get(20..)? {
            "" => 0,
            suffix => suffix.strip_prefix('-')?.parse().ok()?,
        };
        let days = days_from_civil(
            i64::try_from(year).ok()?,
            i64::try_from(month).ok()?,
            i64::try_from(day).ok()?,
        );
        let millis = u64::try_from(days).ok()? * MILLIS_PER_DAY
            + ((hour * 60 + minute) * 60 + second) * 1000
            + milli;
        let name = Self::new(millis, sequence);
        // Re-formatting rejects separators, out-of-range fields, and non-canonical suffixes.
        (name.id()?.0 == stem).then_some(name)
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian `(year, month, day)` of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_known_day_counts() {
        let known = [
            ((1970, 1, 1), 0),
            ((1969, 12, 31), -1),
            ((2000, 1, 1), 10_957),
            ((2000, 2, 29), 11_016),
            ((2000, 3, 1), 11_017),
            ((2024, 2, 29), 19_782),
            ((2026, 10, 31), 20_757),
            ((2038, 1, 19), 24_855),
            ((2100, 3, 1), 47_541),
            ((9999, 12, 31), 2_932_896),
        ];
        for ((year, month, day), days) in known {
            assert_eq!(
                days_from_civil(year, month, day),
                days,
                "{year}-{month}-{day}"
            );
            assert_eq!(civil_from_days(days), (year, month, day), "{days}");
        }
    }

    #[test]
    fn civil_dates_round_trip_across_four_centuries() {
        let start = days_from_civil(1970, 1, 1);
        let mut previous = civil_from_days(start - 1);
        for days in start..start + 146_097 {
            let date = civil_from_days(days);
            assert_eq!(days_from_civil(date.0, date.1, date.2), days);
            assert!(date > previous, "{date:?} after {previous:?}");
            previous = date;
        }
    }

    #[test]
    fn snapshot_names_format_in_utc_to_the_millisecond() {
        // 2026-10-31T17:15:03.042Z
        let millis = 1_793_466_903_042;
        assert_eq!(
            SnapshotName::new(millis, 0).id().unwrap().as_str(),
            "20261031T171503.042Z"
        );
        assert_eq!(
            SnapshotName::new(millis, 12).id().unwrap().as_str(),
            "20261031T171503.042Z-12"
        );
        assert_eq!(
            SnapshotName::new(0, 0).id().unwrap().as_str(),
            "19700101T000000.000Z"
        );
    }

    #[test]
    fn snapshot_names_round_trip() {
        for (millis, sequence) in [(0, 0), (1_793_466_903_042, 0), (1_709_164_799_999, 3)] {
            let name = SnapshotName::new(millis, sequence);
            assert_eq!(SnapshotName::parse(name.id().unwrap().as_str()), Some(name));
        }
    }

    #[test]
    fn foreign_stems_are_not_snapshot_names() {
        for stem in [
            "",
            "notes",
            "2026-10-31",
            "20261031T171503.042",
            "20261031T171503.042Z-",
            "20261031T171503.042Z-0",
            "20261031T171503.042Z-01",
            "20261031T171503.042Z-x",
            "20261031T171503.042Zx",
            "20261031 171503.042Z",
            "20260230T000000.000Z",
            "20261031T240000.000Z",
            "20261031T176000.000Z",
            "+0261031T171503.042Z",
        ] {
            assert_eq!(SnapshotName::parse(stem), None, "{stem:?}");
        }
    }
}
