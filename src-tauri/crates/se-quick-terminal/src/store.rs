use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use se_foundation::durable_fs::{DirectoryPermissions, DurableFileSystem};

use crate::record::{QuickTerminalRecord, QUICK_TERMINAL_SCHEMA_VERSION};
use crate::QuickTerminalId;

/// One durable JSON file per quick terminal (`<dir>/<id>.json`).
///
/// The owning process is the only writer. Files that fail to parse are left
/// in place and skipped, so a hand-edited or future-version record is never
/// destroyed by an older build.
pub struct QuickTerminalStore {
    dir: PathBuf,
    fs: DurableFileSystem,
    records: RwLock<HashMap<QuickTerminalId, QuickTerminalRecord>>,
}

impl QuickTerminalStore {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        let fs = DurableFileSystem::new();
        fs.create_dir_durable(&dir, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(io::Error::other)?;
        let mut records = HashMap::new();
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(stem) = record_stem(&path) else {
                continue;
            };
            match load_record(&path, stem) {
                Ok(record) => {
                    records.insert(record.id, record);
                }
                Err(reason) => log::warn!(
                    target: "se_manager::quick_terminal",
                    "operation=store_load stable_code=RECORD_SKIPPED file={stem} reason={reason}"
                ),
            }
        }
        log::info!(
            target: "se_manager::quick_terminal",
            "operation=store_open stable_code=READY records={}",
            records.len()
        );
        Ok(Self {
            dir,
            fs,
            records: RwLock::new(records),
        })
    }

    /// Newest activity first.
    pub fn list(&self) -> Vec<QuickTerminalRecord> {
        let mut records: Vec<_> = self.records.read().values().cloned().collect();
        records.sort_by(|left, right| right.updated_at_utc.cmp(&left.updated_at_utc));
        records
    }

    pub fn get(&self, id: QuickTerminalId) -> Option<QuickTerminalRecord> {
        self.records.read().get(&id).cloned()
    }

    /// Persist `record`, replacing any record with the same id.
    pub fn put(&self, record: QuickTerminalRecord) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        self.fs
            .replace_bytes(&self.path_for(record.id), &bytes)
            .map_err(io::Error::other)?;
        self.records.write().insert(record.id, record);
        Ok(())
    }

    /// Remove the record. Returns whether one existed.
    pub fn remove(&self, id: QuickTerminalId) -> io::Result<bool> {
        let path = self.path_for(id);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(&self.dir)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(self.records.write().remove(&id).is_some())
    }

    fn path_for(&self, id: QuickTerminalId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

fn record_stem(path: &Path) -> Option<&str> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
        return None;
    }
    path.file_stem().and_then(|stem| stem.to_str())
}

fn load_record(path: &Path, stem: &str) -> Result<QuickTerminalRecord, String> {
    let bytes = fs::read(path).map_err(|error| error.kind().to_string())?;
    let record: QuickTerminalRecord =
        serde_json::from_slice(&bytes).map_err(|_| "INVALID_JSON".to_string())?;
    if record.schema_version != QUICK_TERMINAL_SCHEMA_VERSION {
        return Err(format!("SCHEMA_{}", record.schema_version));
    }
    if record.id.to_string() != stem {
        return Err("ID_MISMATCH".to_string());
    }
    Ok(record)
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Windows has no portable directory fsync; the removal is already durable
/// once `DeleteFile` returns.
#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{QuickTerminalOrigin, QuickTerminalTarget};

    /// Durable directory creation refuses symlinked components (macOS `/var`).
    fn tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        (dir, path)
    }

    fn record(id: QuickTerminalId, updated: &str) -> QuickTerminalRecord {
        QuickTerminalRecord {
            schema_version: QUICK_TERMINAL_SCHEMA_VERSION,
            id,
            title: None,
            target: QuickTerminalTarget::Workspace,
            cwd: "/tmp/qt".to_string(),
            created_at_utc: "2026-09-29T00:00:00.000Z".to_string(),
            updated_at_utc: updated.to_string(),
            terminal_id: None,
            origin: QuickTerminalOrigin::Created,
        }
    }

    #[test]
    fn records_survive_a_reopen_newest_first() {
        let (_guard, dir) = tempdir();
        let older = QuickTerminalId::new_v4();
        let newer = QuickTerminalId::new_v4();
        {
            let store = QuickTerminalStore::open(&dir).unwrap();
            store
                .put(record(older, "2026-09-29T01:00:00.000Z"))
                .unwrap();
            store
                .put(record(newer, "2026-09-29T02:00:00.000Z"))
                .unwrap();
        }

        let reopened = QuickTerminalStore::open(&dir).unwrap();
        let ids: Vec<_> = reopened
            .list()
            .into_iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(ids, vec![newer, older]);
    }

    #[test]
    fn removal_is_persistent() {
        let (_guard, dir) = tempdir();
        let id = QuickTerminalId::new_v4();
        let store = QuickTerminalStore::open(&dir).unwrap();
        store.put(record(id, "2026-09-29T01:00:00.000Z")).unwrap();

        assert!(store.remove(id).unwrap());
        assert!(!store.remove(id).unwrap());
        assert!(QuickTerminalStore::open(&dir).unwrap().list().is_empty());
    }

    #[test]
    fn unreadable_records_are_skipped_and_kept_on_disk() {
        let (_guard, dir) = tempdir();
        let good = QuickTerminalId::new_v4();
        let store = QuickTerminalStore::open(&dir).unwrap();
        store.put(record(good, "2026-09-29T01:00:00.000Z")).unwrap();
        let corrupt = dir.join(format!("{}.json", QuickTerminalId::new_v4()));
        fs::write(&corrupt, b"{not json").unwrap();
        // A record whose file name does not match its id is not trusted either.
        let renamed = dir.join(format!("{}.json", QuickTerminalId::new_v4()));
        fs::write(
            &renamed,
            serde_json::to_vec(&record(
                QuickTerminalId::new_v4(),
                "2026-09-29T01:00:00.000Z",
            ))
            .unwrap(),
        )
        .unwrap();

        let reopened = QuickTerminalStore::open(&dir).unwrap();
        assert_eq!(reopened.list().len(), 1);
        assert!(corrupt.exists(), "a record we cannot read is never deleted");
        assert!(renamed.exists());
    }
}
