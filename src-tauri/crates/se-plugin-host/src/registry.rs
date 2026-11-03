//! Profile-owned durable metadata. No native call may begin before `Journal::begin` succeeds.

use fs2::FileExt;
use se_foundation::durable_fs::{DirectoryPermissions, DurableFileSystem};
use se_plugin_protocol::{
    is_plugin_id, validate_manifest, ConfigValues, Manifest, PluginError, PLUGIN_BUSY, PLUGIN_IO,
    PLUGIN_STORE_CORRUPT, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

type Result<T> = std::result::Result<T, PluginError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Registry {
    pub(crate) schema_version: u32,
    pub(crate) plugins: Vec<RegistryEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistryEntry {
    pub(crate) id: String,
    pub(crate) manifest: Manifest,
    pub(crate) source_path: PathBuf,
    pub(crate) artifact_path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) desired_enabled: bool,
    pub(crate) config_revision: u64,
    pub(crate) config_overrides: ConfigValues,
    #[serde(deserialize_with = "required_option")]
    pub(crate) quarantine: Option<Quarantine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Quarantine {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) at: u64,
}

fn required_option<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub(crate) struct Store {
    /// The canonical `<profile>/plugins` directory, not the profile directory.
    pub(crate) root: PathBuf,
    _lock: File,
    fs: DurableFileSystem,
}

impl Store {
    /// Acquire `<profile>/plugins/host.lock` for this Store's entire lifetime.
    /// `root` is the plugins directory, not the profile directory.
    pub(crate) fn open(root: PathBuf) -> Result<Self> {
        let fs = DurableFileSystem::new();
        fs.create_dir_durable(&root, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(io_error)?;
        owner_only_directory(&root)?;
        let root = root.canonicalize().map_err(io_error)?;
        let lock_path = root.join("host.lock");
        reject_non_regular(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(&lock_path).map_err(io_error)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                PluginError::new(PLUGIN_BUSY, "another Plugin Core owns this profile")
            } else {
                io_error(error)
            }
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            lock.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(io_error)?;
        }
        lock.sync_all().map_err(io_error)?;
        Ok(Self {
            root,
            _lock: lock,
            fs,
        })
    }

    pub(crate) fn read(&self) -> Result<Registry> {
        let Some(bytes) = read_optional(&self.root.join("registry.json"))? else {
            return Ok(Registry::default());
        };
        let registry: Registry = serde_json::from_slice(&bytes)
            .map_err(|_| corrupt("registry JSON is malformed or has an unsupported shape"))?;
        self.validate(&registry)?;
        Ok(registry)
    }

    pub(crate) fn write(&self, registry: &Registry) -> Result<()> {
        self.validate(registry)?;
        // Never replace an existing corrupt registry, even if a caller supplies an empty one.
        self.read()?;
        persist(&self.fs, &self.root.join("registry.json"), registry)
    }

    fn validate(&self, registry: &Registry) -> Result<()> {
        if registry.schema_version != SCHEMA_VERSION {
            return Err(corrupt("unsupported registry schemaVersion"));
        }
        let mut ids = BTreeSet::new();
        for entry in &registry.plugins {
            if entry.id != entry.manifest.id || !ids.insert(&entry.id) {
                return Err(corrupt(
                    "registry contains duplicate or mismatched plugin identities",
                ));
            }
            validate_manifest(&entry.manifest)
                .map_err(|_| corrupt("registry manifest is invalid"))?;
            if !absolute_normal_path(&entry.source_path)
                || !absolute_normal_path(&entry.artifact_path)
                || !entry.artifact_path.starts_with(self.root.join("packages"))
                || !valid_hash(&entry.sha256)
            {
                return Err(corrupt("registry contains invalid paths or SHA-256"));
            }
            crate::config::snapshot(entry)
                .map_err(|_| corrupt("registry configuration is invalid"))?;
            if entry
                .quarantine
                .as_ref()
                .is_some_and(|q| q.code.is_empty() || q.message.is_empty() || q.at == 0)
            {
                return Err(corrupt("registry quarantine is invalid"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Recovery {
    pub(crate) safe_mode: bool,
    pub(crate) quarantine_all: bool,
    pub(crate) plugin_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InFlight {
    #[serde(deserialize_with = "required_option")]
    plugin_id: Option<String>,
    generation: String,
    sha256: String,
    callback: String,
    started_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeRecord {
    schema_version: u32,
    boot_id: String,
    clean_exit: bool,
    recent_unclean_starts: Vec<u64>,
    in_flight: Vec<InFlight>,
}

#[derive(Clone)]
struct JournalState {
    record: RuntimeRecord,
    // Tokens are host-private; on-disk records intentionally use the approved schema only.
    calls: BTreeMap<String, InFlight>,
}

pub(crate) struct Journal {
    path: PathBuf,
    fs: DurableFileSystem,
    state: Mutex<JournalState>,
}

impl Journal {
    /// `root` is Store::root, and the Store lock must remain held while this journal is used.
    pub(crate) fn open(root: PathBuf, boot_id: String) -> Result<(Arc<Self>, Recovery)> {
        Self::open_at(root, boot_id, now()?)
    }

    fn open_at(root: PathBuf, boot_id: String, at: u64) -> Result<(Arc<Self>, Recovery)> {
        if !valid_uuid(&boot_id) {
            return Err(io_error("invalid journal boot ID"));
        }
        let fs = DurableFileSystem::new();
        fs.create_dir_durable(&root, DirectoryPermissions::PrivateOwnerOnly)
            .map_err(io_error)?;
        owner_only_directory(&root)?;
        let path = root.join("runtime.json");
        let previous: Option<RuntimeRecord> = read_optional(&path)?
            .map(|bytes| {
                serde_json::from_slice(&bytes)
                    .map_err(|_| corrupt("runtime journal JSON is malformed"))
            })
            .transpose()?;
        let mut recovery = Recovery::default();
        let mut starts = Vec::new();
        if let Some(previous) = previous {
            validate_runtime(&previous)?;
            if !previous.clean_exit {
                starts = previous
                    .recent_unclean_starts
                    .into_iter()
                    .filter(|time| at.saturating_sub(*time) <= 60)
                    .collect();
                starts.push(at);
                recovery.safe_mode = starts.len() >= 3;
                recovery.quarantine_all = previous.in_flight.is_empty();
                for call in previous.in_flight {
                    match call.plugin_id {
                        Some(id) => {
                            recovery.plugin_ids.insert(id);
                        }
                        None => recovery.quarantine_all = true,
                    }
                }
            }
        }
        // Only the sliding window and threshold matter; keep the record bounded in crash loops.
        if starts.len() > 3 {
            starts.drain(..starts.len() - 3);
        }
        let record = RuntimeRecord {
            schema_version: SCHEMA_VERSION,
            boot_id,
            clean_exit: false,
            recent_unclean_starts: starts,
            in_flight: Vec::new(),
        };
        persist(&fs, &path, &record)?;
        Ok((
            Arc::new(Self {
                path,
                fs,
                state: Mutex::new(JournalState {
                    record,
                    calls: BTreeMap::new(),
                }),
            }),
            recovery,
        ))
    }

    pub(crate) fn begin(
        &self,
        plugin_id: Option<String>,
        generation: String,
        sha256: String,
        callback: String,
    ) -> Result<String> {
        let call = InFlight {
            plugin_id,
            generation,
            sha256,
            callback,
            started_at: now()?,
        };
        validate_call(&call)?;
        let mut state = self.state()?;
        if state.record.clean_exit {
            return Err(io_error("journal is already closed"));
        }
        let token = Uuid::new_v4().to_string();
        let mut next = state.clone();
        next.calls.insert(token.clone(), call);
        next.record.in_flight = next.calls.values().cloned().collect();
        persist(&self.fs, &self.path, &next.record)?;
        *state = next;
        Ok(token)
    }

    pub(crate) fn end(&self, token: &str) -> Result<()> {
        let mut state = self.state()?;
        if !state.calls.contains_key(token) {
            return Err(io_error("unknown journal callback token"));
        }
        let mut next = state.clone();
        next.calls.remove(token);
        next.record.in_flight = next.calls.values().cloned().collect();
        persist(&self.fs, &self.path, &next.record)?;
        *state = next;
        Ok(())
    }

    pub(crate) fn clean_exit(&self) -> Result<()> {
        let mut state = self.state()?;
        if !state.calls.is_empty() {
            return Err(io_error(
                "cannot mark clean exit with foreign calls in flight",
            ));
        }
        let mut next = state.clone();
        next.record.clean_exit = true;
        next.record.recent_unclean_starts.clear();
        persist(&self.fs, &self.path, &next.record)?;
        *state = next;
        Ok(())
    }

    fn state(&self) -> Result<MutexGuard<'_, JournalState>> {
        self.state
            .lock()
            .map_err(|_| io_error("runtime journal mutex poisoned"))
    }
}

fn validate_runtime(record: &RuntimeRecord) -> Result<()> {
    if record.schema_version != SCHEMA_VERSION
        || !valid_uuid(&record.boot_id)
        || (record.clean_exit && !record.in_flight.is_empty())
        || record.recent_unclean_starts.contains(&0)
    {
        return Err(corrupt("runtime journal semantic validation failed"));
    }
    for call in &record.in_flight {
        validate_call(call)?;
    }
    Ok(())
}

fn validate_call(call: &InFlight) -> Result<()> {
    if call
        .plugin_id
        .as_deref()
        .is_some_and(|id| !is_plugin_id(id))
        || !valid_uuid(&call.generation)
        || !valid_hash(&call.sha256)
        || call.callback.is_empty()
        || call.callback.len() > 128
        || call.started_at == 0
    {
        return Err(corrupt("runtime journal callback is invalid"));
    }
    Ok(())
}

fn absolute_normal_path(path: &Path) -> bool {
    path.is_absolute()
        && path.file_name().is_some()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(io_error)
}
fn corrupt(message: &str) -> PluginError {
    PluginError::new(PLUGIN_STORE_CORRUPT, message)
}
fn io_error(error: impl std::fmt::Display) -> PluginError {
    PluginError::new(PLUGIN_IO, error.to_string())
}

fn reject_non_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(corrupt("plugin metadata path is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn owner_only_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    let _ = path;
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    reject_non_regular(path)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    // Bound allocations on damaged metadata without disclosing any configuration bytes.
    const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(corrupt("plugin metadata exceeds size limit"));
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("se-plugin-store-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn boot() -> String {
        Uuid::new_v4().to_string()
    }
    fn call(journal: &Journal, id: Option<&str>) -> String {
        journal
            .begin(
                id.map(str::to_owned),
                boot(),
                "a".repeat(64),
                "command".into(),
            )
            .unwrap()
    }

    #[test]
    fn lock_excludes_writers_and_releases_when_store_drops() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        assert!(store.read().unwrap().plugins.is_empty());
        assert!(matches!(Store::open(temp.0.clone()), Err(error) if error.code == PLUGIN_BUSY));
        store.write(&Registry::default()).unwrap();
        assert_eq!(store.read().unwrap().schema_version, SCHEMA_VERSION);
        drop(store);
        Store::open(temp.0.clone()).unwrap();
    }

    #[test]
    fn corrupt_registry_bytes_are_not_overwritten() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let path = store.root.join("registry.json");
        for bytes in [
            b"{broken".as_slice(),
            br#"{"schemaVersion":2,"plugins":[]}"#,
            br#"{"schemaVersion":1}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(store.read().unwrap_err().code, PLUGIN_STORE_CORRUPT);
            assert_eq!(
                store.write(&Registry::default()).unwrap_err().code,
                PLUGIN_STORE_CORRUPT
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    fn valid_entry(store: &Store) -> RegistryEntry {
        RegistryEntry {
            id: "test.plugin".into(),
            manifest: serde_json::from_value(serde_json::json!({
                "schemaVersion": 1, "id": "test.plugin", "name": "Test", "version": "1.0.0",
                "abi": {"min": 1, "max": 1},
                "contributes": {"commands": [], "views": [], "configuration": {"title": "Settings", "properties": {}}},
                "services": {"provides": [], "requires": []}
            })).unwrap(),
            source_path: store.root.join("original.dylib"),
            artifact_path: store.root.join("packages/test.plugin/generation/artifact.dylib"),
            sha256: "a".repeat(64), desired_enabled: true,
            config_revision: 1, config_overrides: ConfigValues::new(), quarantine: None,
        }
    }

    #[test]
    fn semantic_corruption_is_rejected_and_preserved() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let valid = valid_entry(&store);
        let base = Registry {
            plugins: vec![valid.clone()],
            ..Registry::default()
        };
        store.write(&base).unwrap();
        assert!(store.read().unwrap().plugins[0].desired_enabled);
        let mut variants = Vec::new();
        let mut duplicate = base.clone();
        duplicate.plugins.push(valid.clone());
        variants.push(duplicate);
        let mut mismatch = base.clone();
        mismatch.plugins[0].id = "other.plugin".into();
        variants.push(mismatch);
        let mut revision = base.clone();
        revision.plugins[0].config_revision = 0;
        variants.push(revision);
        let mut path = base.clone();
        path.plugins[0].artifact_path = store.root.join("../escape.dylib");
        variants.push(path);
        let mut hash = base.clone();
        hash.plugins[0].sha256 = "bad".into();
        variants.push(hash);
        let mut config = base.clone();
        config.plugins[0].config_overrides.insert(
            "unknown".into(),
            se_plugin_protocol::ConfigValue::Boolean(true),
        );
        variants.push(config);
        for invalid in variants {
            let bytes = serde_json::to_vec(&invalid).unwrap();
            fs::write(store.root.join("registry.json"), &bytes).unwrap();
            assert_eq!(store.read().unwrap_err().code, PLUGIN_STORE_CORRUPT);
            assert_eq!(fs::read(store.root.join("registry.json")).unwrap(), bytes);
        }
    }

    #[test]
    fn completed_calls_do_not_replace_other_inflight_calls() {
        let temp = Scratch::new();
        let (journal, recovery) = Journal::open_at(temp.0.clone(), boot(), 100).unwrap();
        assert!(!recovery.quarantine_all);
        let first = call(&journal, Some("first.plugin"));
        let _second = call(&journal, Some("second.plugin"));
        let disk: RuntimeRecord =
            serde_json::from_slice(&fs::read(&journal.path).unwrap()).unwrap();
        assert_eq!(disk.in_flight.len(), 2);
        assert!(!disk.clean_exit);
        assert!(journal.clean_exit().is_err());
        journal.end(&first).unwrap();
        assert!(journal.end(&first).is_err());
        drop(journal);
        let (_, recovery) = Journal::open_at(temp.0.clone(), boot(), 101).unwrap();
        assert_eq!(
            recovery.plugin_ids,
            [String::from("second.plugin")].into_iter().collect()
        );
        assert!(!recovery.quarantine_all);
    }

    #[test]
    fn unattributed_crashes_and_repeated_idle_crashes_are_conservative() {
        let temp = Scratch::new();
        let (journal, _) = Journal::open_at(temp.0.clone(), boot(), 100).unwrap();
        call(&journal, None);
        drop(journal);
        let (journal, recovery) = Journal::open_at(temp.0.clone(), boot(), 110).unwrap();
        assert!(recovery.quarantine_all);
        assert!(!recovery.safe_mode);
        drop(journal);
        let (journal, recovery) = Journal::open_at(temp.0.clone(), boot(), 120).unwrap();
        assert!(!recovery.safe_mode);
        drop(journal);
        let (journal, recovery) = Journal::open_at(temp.0.clone(), boot(), 130).unwrap();
        assert!(recovery.safe_mode);
        journal.clean_exit().unwrap();
        assert!(journal
            .begin(None, boot(), "a".repeat(64), "late".into())
            .is_err());
        drop(journal);
        let (_, recovery) = Journal::open_at(temp.0.clone(), boot(), 131).unwrap();
        assert!(!recovery.safe_mode);
        assert!(!recovery.quarantine_all);
    }

    #[test]
    fn old_crashes_age_out_of_safe_mode_window() {
        let temp = Scratch::new();
        for time in [100, 110, 200, 201] {
            let (journal, recovery) = Journal::open_at(temp.0.clone(), boot(), time).unwrap();
            assert!(!recovery.safe_mode);
            drop(journal);
        }
    }

    #[test]
    fn corrupt_runtime_is_not_replaced() {
        let temp = Scratch::new();
        let path = temp.0.join("runtime.json");
        let bytes = b"broken-runtime";
        fs::write(&path, bytes).unwrap();
        assert!(
            matches!(Journal::open_at(temp.0.clone(), boot(), 100), Err(error) if error.code == PLUGIN_STORE_CORRUPT)
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_metadata_is_private_and_symlinks_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        store.write(&Registry::default()).unwrap();
        for (path, mode) in [
            (store.root.clone(), 0o700),
            (store.root.join("host.lock"), 0o600),
            (store.root.join("registry.json"), 0o600),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
        let outside = temp.0.join("outside");
        fs::write(&outside, b"untouched").unwrap();
        fs::remove_file(store.root.join("registry.json")).unwrap();
        symlink(&outside, store.root.join("registry.json")).unwrap();
        assert_eq!(store.read().unwrap_err().code, PLUGIN_STORE_CORRUPT);
        assert!(store.write(&Registry::default()).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
    }
}

fn persist(fs: &DurableFileSystem, path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| io_error("plugin metadata serialization failed"))?;
    let outcome = fs.replace_bytes(path, &bytes).map_err(io_error)?;
    if outcome.namespace_state != se_foundation::durable_fs::NamespaceState::NewComplete
        || outcome.crash_point != se_foundation::durable_fs::CrashPoint::AfterNamespaceSync
    {
        return Err(io_error(
            "plugin metadata write did not reach a durable commit",
        ));
    }
    Ok(())
}
