//! Log files that roll over each UTC day, and the sweep that deletes old ones.
//!
//! Every process writes one active file (`se-manager.log`,
//! `se-manager-terminal-core.log`, ...) whose name never changes, so "save a
//! copy of the log" and the startup banner keep pointing at it. When the day
//! changes, or the file outgrows its size cap, it is renamed to
//! `<stem>.<YYYY-MM-DD>.log` (`.<YYYY-MM-DD>.<n>.log` for a second roll the same
//! day) and a fresh active file is opened. Dates are UTC, matching the
//! timestamps on every line.
//!
//! Deleting is left to [`prune_log_dir`], run by the GUI with the user's
//! retention setting. It only ever removes archives: an active file may be held
//! open by a running process, and unlinking it would lose everything written
//! after.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, Utc};

/// The active log file of one process. See the module docs.
pub struct DailyLogFile {
    path: PathBuf,
    max_size: u64,
    file: File,
    /// UTC day the active file holds lines for.
    day: NaiveDate,
    size: u64,
}

impl DailyLogFile {
    /// Open `path` for appending. A file left over from an earlier day is
    /// archived first, so the active file only ever holds one day.
    pub fn open(path: impl Into<PathBuf>, max_size: u64) -> io::Result<Self> {
        Self::open_at(path.into(), max_size, Utc::now())
    }

    fn open_at(path: PathBuf, max_size: u64, now: DateTime<Utc>) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let day = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .map(|modified| DateTime::<Utc>::from(modified).date_naive())
            .unwrap_or_else(|_| now.date_naive());
        let file = open_append(&path)?;
        let size = file.metadata()?.len();
        let mut log = Self {
            path,
            max_size,
            file,
            day,
            size,
        };
        log.roll_if_due(now.date_naive(), 0)?;
        Ok(log)
    }

    /// Write `bytes` as of `now`, rolling over first when the day changed or
    /// they would push the file past its size cap.
    pub fn write_at(&mut self, bytes: &[u8], now: DateTime<Utc>) -> io::Result<()> {
        self.roll_if_due(now.date_naive(), bytes.len() as u64)?;
        self.file.write_all(bytes)?;
        self.size += bytes.len() as u64;
        Ok(())
    }

    fn roll_if_due(&mut self, today: NaiveDate, incoming: u64) -> io::Result<()> {
        let new_day = today != self.day;
        let too_big = self.size > 0 && self.size + incoming > self.max_size;
        if !new_day && !too_big {
            return Ok(());
        }
        if self.size > 0 {
            self.file.flush()?;
            std::fs::rename(&self.path, free_archive_path(&self.path, self.day))?;
            self.file = open_append(&self.path)?;
            self.size = 0;
        }
        self.day = today;
        Ok(())
    }
}

impl Write for DailyLogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_at(buf, Utc::now())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// `<stem>.<day>.log`, or `<stem>.<day>.<n>.log` when that is taken.
fn free_archive_path(active: &Path, day: NaiveDate) -> PathBuf {
    let stem = active
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let day = day.format("%Y-%m-%d");
    let first = active.with_file_name(format!("{stem}.{day}.log"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| active.with_file_name(format!("{stem}.{day}.{n}.log")))
        .find(|candidate| !candidate.exists())
        .expect("an unused archive name")
}

/// Delete log archives in `dir` older than `keep_days` days, today included:
/// with 7, everything from six days ago onward stays. Returns what was removed.
///
/// Archives are recognised by the date in their name. Files from before daily
/// rolling (`*.old.log`) carry no date and are judged by modification time.
/// Active files and anything that is not a `.log` file are never touched.
pub fn prune_log_dir(dir: &Path, keep_days: u32, today: NaiveDate) -> Vec<PathBuf> {
    let Some(oldest_kept) =
        today.checked_sub_signed(Duration::days(i64::from(keep_days.max(1)) - 1))
    else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(day) = archive_day(&path) else {
            continue;
        };
        if day < oldest_kept && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// The day an archive holds, or `None` for anything that is not an archive.
fn archive_day(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let base = name.strip_suffix(".log")?;
    if base.ends_with(".old") {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        return Some(DateTime::<Utc>::from(modified).date_naive());
    }
    // `<stem>.<day>` or `<stem>.<day>.<n>`.
    let mut parts = base.rsplit('.');
    let last = parts.next()?;
    let day_part = if last.chars().all(|c| c.is_ascii_digit()) && last.len() < 4 {
        parts.next()?
    } else {
        last
    };
    NaiveDate::parse_from_str(day_part, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: &str, time: &str) -> DateTime<Utc> {
        format!("{day}T{time}Z").parse().unwrap()
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_new_day_archives_yesterday_under_its_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se-manager.log");
        let mut log =
            DailyLogFile::open_at(path.clone(), 1024, at("2026-10-14", "23:59:00")).unwrap();

        log.write_at(b"monday\n", at("2026-10-14", "23:59:30"))
            .unwrap();
        log.write_at(b"tuesday\n", at("2026-10-15", "00:00:01"))
            .unwrap();

        assert_eq!(
            names(dir.path()),
            ["se-manager.2026-10-14.log", "se-manager.log"]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("se-manager.2026-10-14.log")).unwrap(),
            "monday\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tuesday\n");
    }

    #[test]
    fn the_size_cap_rolls_within_a_day_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("core.log");
        let now = at("2026-10-15", "10:00:00");
        let mut log = DailyLogFile::open_at(path.clone(), 10, now).unwrap();

        log.write_at(b"12345678\n", now).unwrap();
        log.write_at(b"abcdefgh\n", now).unwrap();
        log.write_at(b"ABCDEFGH\n", now).unwrap();

        assert_eq!(
            names(dir.path()),
            ["core.2026-10-15.2.log", "core.2026-10-15.log", "core.log"]
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "ABCDEFGH\n");
    }

    #[test]
    fn a_file_left_from_an_earlier_day_is_archived_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se-manager.log");
        std::fs::write(&path, "old run\n").unwrap();

        // The file was just written, so its day is today in real time; open it
        // "tomorrow" to see it archived under that day.
        let tomorrow = Utc::now() + Duration::days(1);
        let written = Utc::now().date_naive().format("%Y-%m-%d").to_string();
        let mut log = DailyLogFile::open_at(path.clone(), 1024, tomorrow).unwrap();
        log.write_at(b"new run\n", tomorrow).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join(format!("se-manager.{written}.log"))).unwrap(),
            "old run\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new run\n");
    }

    #[test]
    fn an_empty_active_file_is_not_archived() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("se-manager.log");
        let mut log =
            DailyLogFile::open_at(path.clone(), 1024, at("2026-10-14", "12:00:00")).unwrap();

        log.write_at(b"x\n", at("2026-10-15", "12:00:00")).unwrap();

        assert_eq!(names(dir.path()), ["se-manager.log"]);
    }

    #[test]
    fn pruning_keeps_the_last_n_days_and_never_touches_active_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "se-manager.log",
            "se-manager-terminal-core.log",
            "se-manager.2026-10-15.log",
            "se-manager.2026-10-09.log",
            "se-manager.2026-10-09.2.log",
            "se-manager.2026-10-08.log",
            "se-manager-acp-core.2026-10-01.3.log",
            "notes.txt",
            "se-manager.not-a-date.log",
        ] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }

        let removed = prune_log_dir(dir.path(), 7, day("2026-10-15"));

        assert_eq!(removed.len(), 2);
        assert_eq!(
            names(dir.path()),
            [
                "notes.txt",
                "se-manager-terminal-core.log",
                "se-manager.2026-10-09.2.log",
                "se-manager.2026-10-09.log",
                "se-manager.2026-10-15.log",
                "se-manager.log",
                "se-manager.not-a-date.log",
            ]
        );
    }

    #[test]
    fn pruning_with_one_day_keeps_only_today() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.2026-10-15.log", "a.2026-10-14.log"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }

        prune_log_dir(dir.path(), 1, day("2026-10-15"));

        assert_eq!(names(dir.path()), ["a.2026-10-15.log"]);
    }

    #[test]
    fn pre_rolling_archives_are_judged_by_age() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("se-manager-acp-core.old.log");
        std::fs::write(&old, "x").unwrap();
        let today = Utc::now().date_naive();

        assert!(prune_log_dir(dir.path(), 7, today).is_empty());
        let removed = prune_log_dir(dir.path(), 7, today + Duration::days(7));

        assert_eq!(removed, vec![old]);
    }
}
