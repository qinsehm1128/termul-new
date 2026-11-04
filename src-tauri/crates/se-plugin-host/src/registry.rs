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
    // Retained after uninstall so old configuration forms cannot match a reinstallation.
    // Default supports registries written before revision allocation was profile-wide.
    #[serde(default)]
    pub(crate) config_revision_high_water: u64,
    pub(crate) plugins: Vec<RegistryEntry>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            config_revision_high_water: 0,
            plugins: Vec::new(),
        }
    }
}

impl Registry {
    pub(crate) fn allocate_config_revision(&mut self) -> Result<u64> {
        let last = self
            .plugins
            .iter()
            .map(|entry| entry.config_revision)
            .max()
            .unwrap_or(0)
            .max(self.config_revision_high_water);
        if last >= se_plugin_protocol::MAX_SAFE_INTEGER {
            return Err(io_error("configuration revision exhausted"));
        }
        self.config_revision_high_water = last + 1;
        Ok(last + 1)
    }

    pub(crate) fn retain_config_revisions(&mut self) {
        self.config_revision_high_water = self
            .plugins
            .iter()
            .map(|entry| entry.config_revision)
            .max()
            .unwrap_or(0)
            .max(self.config_revision_high_water);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistryEntry {
    pub(crate) id: String,
    pub(crate) manifest: Manifest,
    pub(crate) source_path: PathBuf,
    pub(crate) artifact_path: PathBuf,
    /// Hash of the selected native library, never the enclosing archive.
    pub(crate) sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) package: Option<PackageMetadata>,
    pub(crate) desired_enabled: bool,
    pub(crate) config_revision: u64,
    pub(crate) config_overrides: ConfigValues,
    #[serde(deserialize_with = "required_option")]
    pub(crate) quarantine: Option<Quarantine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PackageMetadata {
    pub(crate) digest: String,
    /// Canonical published `<plugins>/packages/<id>/<version>/<digest>` directory.
    pub(crate) root: PathBuf,
    pub(crate) target: String,
    /// Normalized package-relative `native/<target>/<library>` path.
    pub(crate) native_path: String,
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
    #[cfg(test)]
    fault: Arc<TestCrashInjector>,
}

#[cfg(test)]
#[derive(Default)]
struct TestCrashInjector(Mutex<Option<se_foundation::durable_fs::CrashPoint>>);

#[cfg(test)]
impl se_foundation::durable_fs::CrashInjector for TestCrashInjector {
    fn should_interrupt(&self, point: se_foundation::durable_fs::CrashPoint) -> bool {
        let mut armed = self.0.lock().unwrap();
        if *armed == Some(point) {
            *armed = None;
            true
        } else {
            false
        }
    }
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
        #[cfg(test)]
        let fault = Arc::new(TestCrashInjector::default());
        #[cfg(test)]
        let fs = DurableFileSystem::with_crash_injector(fault.clone());
        Ok(Self {
            root,
            _lock: lock,
            fs,
            #[cfg(test)]
            fault,
        })
    }

    #[cfg(test)]
    pub(crate) fn interrupt_next_write(&self, point: se_foundation::durable_fs::CrashPoint) {
        *self.fault.0.lock().unwrap() = Some(point);
    }

    pub(crate) fn read(&self) -> Result<Registry> {
        let Some(bytes) = read_optional(&self.root.join("registry.json"))? else {
            return Ok(Registry::default());
        };
        let mut registry: Registry = serde_json::from_slice(&bytes)
            .map_err(|_| corrupt("registry JSON is malformed or has an unsupported shape"))?;
        for entry in &mut registry.plugins {
            if let Some(quarantine) = &mut entry.quarantine {
                let diagnostic =
                    normalize_diagnostic(PluginError::new(&quarantine.code, &quarantine.message));
                quarantine.code = diagnostic.code;
                quarantine.message = diagnostic.message;
            }
        }
        self.validate(&registry)?;
        registry.retain_config_revisions();
        Ok(registry)
    }

    pub(crate) fn write(&self, registry: &Registry) -> Result<()> {
        self.validate(registry)?;
        // Never replace an existing corrupt registry, even if a caller supplies an empty one.
        self.read()?;
        let mut registry = registry.clone();
        registry.retain_config_revisions();
        persist(&self.fs, &self.root.join("registry.json"), &registry)
    }

    fn validate(&self, registry: &Registry) -> Result<()> {
        if registry.schema_version != SCHEMA_VERSION {
            return Err(corrupt("unsupported registry schemaVersion"));
        }
        if registry.config_revision_high_water > se_plugin_protocol::MAX_SAFE_INTEGER {
            return Err(corrupt("configuration revision high-water mark is invalid"));
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
            if let Some(package) = &entry.package {
                let expected_root = self
                    .root
                    .join("packages")
                    .join(&entry.id)
                    .join(&entry.manifest.version)
                    .join(&package.digest);
                if !valid_hash(&package.digest)
                    || !absolute_normal_path(&package.root)
                    || package.root != expected_root
                    || !se_plugin_protocol::is_package_path(&package.native_path)
                    || !package
                        .native_path
                        .starts_with(&format!("native/{}/", package.target))
                    || !(1..=128).contains(&package.target.len())
                    || package.target.split('-').count() < 3
                    || !package
                        .target
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
                    || entry.artifact_path != package.root.join(&package.native_path)
                {
                    return Err(corrupt(
                        "registry package identity or native path is invalid",
                    ));
                }
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
pub(crate) fn io_error(error: impl std::fmt::Display) -> PluginError {
    PluginError::new(PLUGIN_IO, error.to_string())
}

pub(crate) fn reject_non_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(corrupt("plugin metadata path is not a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

pub(crate) fn owner_only_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    let _ = path;
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)?;
    let _ = path;
    Ok(())
}
pub(crate) fn owner_only_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    }
    let _ = path;
    Ok(())
}
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
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

pub(crate) fn normalize_diagnostic(mut error: PluginError) -> PluginError {
    const MAX: usize = 8192;
    let suffix = error.code.strip_prefix("PLUGIN_").unwrap_or("");
    if error.code.len() > MAX
        || suffix.is_empty()
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
    {
        error.code = se_plugin_protocol::PLUGIN_CALLBACK_ERROR.into();
    }
    // JS string limits count UTF-16 code units; never split a Rust char or surrogate pair.
    let mut units = 0;
    let keep = error.message.char_indices().find_map(|(index, ch)| {
        units += ch.len_utf16();
        (units > MAX).then_some(index)
    });
    if let Some(keep) = keep {
        error.message.truncate(keep);
    }
    if error.message.is_empty() {
        error.message = "plugin diagnostic unavailable".into();
    }
    error
}

fn persist(fs: &DurableFileSystem, path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| io_error("plugin metadata serialization failed"))?;
    finish_persist(path, fs.replace_bytes(path, &bytes))
}

// Publication is the commit point for live state, including journal tokens. Returning an
// error after publication would tell callers to roll back even though disk has new bytes.
// Keep the new state and surface uncertain crash durability as a structured log warning.
pub(crate) fn finish_persist(
    path: &Path,
    result: se_foundation::durable_fs::Result<se_foundation::durable_fs::DurableWriteOutcome>,
) -> Result<()> {
    use se_foundation::durable_fs::{CrashPoint, NamespaceState};
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            match error.durable_outcome() {
                Some(outcome) if outcome.namespace_state == NamespaceState::NewComplete => {
                    log::warn!("code={PLUGIN_IO} durability=uncertain published=true path={} error={error}", path.display());
                    return Ok(());
                }
                _ => return Err(io_error(error)),
            }
        }
    };
    if outcome.namespace_state != NamespaceState::NewComplete {
        return Err(io_error("plugin metadata write was not published"));
    }
    if outcome.crash_point != CrashPoint::AfterNamespaceSync {
        log::warn!(
            "code={PLUGIN_IO} durability=uncertain published=true path={}",
            path.display()
        );
    }
    Ok(())
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
    fn published_namespace_sync_error_is_committed_but_prepublication_error_is_not() {
        use se_foundation::durable_fs::{CrashPoint, DurableFsError, NamespaceState};
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let path = store.root.join("registry.json");
        store.write(&Registry::default()).unwrap();
        let fault = Arc::new(TestCrashInjector::default());
        let fs = DurableFileSystem::with_crash_injector(fault.clone());
        for point in [CrashPoint::AfterTempSync, CrashPoint::AfterReplace] {
            *fault.0.lock().unwrap() = Some(point);
            let bytes = br#"{"schemaVersion":1,"configRevisionHighWater":42,"plugins":[]}"#;
            let outcome = fs.replace_bytes(&path, bytes).unwrap();
            // Use the real injector's namespace outcome, wrapped exactly as durable_fs
            // wraps an actual parent-directory fsync error after replacement.
            let result = finish_persist(
                &path,
                Err(DurableFsError::Io {
                    operation: "replace_bytes",
                    step: "namespace_sync",
                    path: path.clone(),
                    source: io::Error::other("injected sync failure"),
                    outcome: Some(outcome),
                }),
            );
            if outcome.namespace_state == NamespaceState::NewComplete {
                result.unwrap();
                assert_eq!(store.read().unwrap().config_revision_high_water, 42);
            } else {
                assert_eq!(result.unwrap_err().code, PLUGIN_IO);
                assert_eq!(store.read().unwrap().config_revision_high_water, 0);
            }
        }
    }

    #[test]
    fn journal_adopts_published_begin_end_and_clean_exit() {
        use se_foundation::durable_fs::CrashPoint;
        let temp = Scratch::new();
        let (journal, _) = Journal::open_at(temp.0.clone(), boot(), 100).unwrap();
        let mut journal = Arc::try_unwrap(journal).ok().unwrap();
        let fault = Arc::new(TestCrashInjector::default());
        journal.fs = DurableFileSystem::with_crash_injector(fault.clone());
        *fault.0.lock().unwrap() = Some(CrashPoint::AfterReplace);
        let token = call(&journal, Some("test.plugin"));
        assert_eq!(journal.state().unwrap().calls.len(), 1);
        let disk: RuntimeRecord =
            serde_json::from_slice(&fs::read(&journal.path).unwrap()).unwrap();
        assert_eq!(disk.in_flight.len(), 1);
        *fault.0.lock().unwrap() = Some(CrashPoint::AfterReplace);
        journal.end(&token).unwrap();
        assert!(journal.state().unwrap().calls.is_empty());
        let disk: RuntimeRecord =
            serde_json::from_slice(&fs::read(&journal.path).unwrap()).unwrap();
        assert!(disk.in_flight.is_empty());
        *fault.0.lock().unwrap() = Some(CrashPoint::AfterReplace);
        journal.clean_exit().unwrap();
        assert!(journal.state().unwrap().record.clean_exit);
        let disk: RuntimeRecord =
            serde_json::from_slice(&fs::read(&journal.path).unwrap()).unwrap();
        assert!(disk.clean_exit);
    }

    #[test]
    fn legacy_registry_revisions_seed_persistent_high_water_and_exhaust_safely() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let mut entry = valid_entry(&store);
        entry.config_revision = 99;
        let bytes = serde_json::json!({"schemaVersion": 1, "plugins": [entry]});
        fs::write(
            store.root.join("registry.json"),
            serde_json::to_vec(&bytes).unwrap(),
        )
        .unwrap();
        let mut registry = store.read().unwrap();
        assert_eq!(registry.allocate_config_revision().unwrap(), 100);
        registry.plugins.clear();
        store.write(&registry).unwrap();
        let mut registry = store.read().unwrap();
        assert_eq!(registry.allocate_config_revision().unwrap(), 101);
        registry.config_revision_high_water = se_plugin_protocol::MAX_SAFE_INTEGER;
        assert_eq!(
            registry.allocate_config_revision().unwrap_err().code,
            PLUGIN_IO
        );
    }

    #[test]
    fn malformed_quarantine_diagnostics_are_normalized_on_read() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let mut entry = valid_entry(&store);
        entry.quarantine = Some(Quarantine {
            code: "bad-code".into(),
            message: "😀".repeat(9000),
            at: 1,
        });
        let bytes = serde_json::json!({"schemaVersion": 1, "plugins": [entry]});
        fs::write(
            store.root.join("registry.json"),
            serde_json::to_vec(&bytes).unwrap(),
        )
        .unwrap();
        let registry = store.read().unwrap();
        let q = registry.plugins[0].quarantine.as_ref().unwrap();
        assert_eq!(q.code, se_plugin_protocol::PLUGIN_CALLBACK_ERROR);
        assert_eq!(q.message.encode_utf16().count(), 8192);
        assert_eq!(q.message.chars().count(), 4096);
        // Reading repairs the in-memory diagnostic, not the original evidence on disk.
        assert_eq!(
            fs::read(store.root.join("registry.json")).unwrap(),
            serde_json::to_vec(&bytes).unwrap()
        );
    }

    #[test]
    fn diagnostic_normalization_keeps_valid_errors_and_bounds_bad_codes() {
        let valid = PluginError::new(PLUGIN_IO, "disk unavailable");
        assert_eq!(normalize_diagnostic(valid.clone()), valid);
        for code in [
            "PLUGIN_".to_owned(),
            "PLUGIN_bad".into(),
            format!("PLUGIN_{}", "A".repeat(8192)),
        ] {
            assert_eq!(
                normalize_diagnostic(PluginError::new(code, "reason")).code,
                se_plugin_protocol::PLUGIN_CALLBACK_ERROR
            );
        }
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

    #[test]
    fn package_metadata_binds_digest_root_target_and_native_path() {
        let temp = Scratch::new();
        let store = Store::open(temp.0.clone()).unwrap();
        let mut entry = valid_entry(&store);
        let target = crate::archive::target().to_owned();
        let digest = "b".repeat(64);
        let root = store
            .root
            .join("packages")
            .join(&entry.id)
            .join(&entry.manifest.version)
            .join(&digest);
        let native_path = format!("native/{target}/plugin.{}", crate::loader::extension());
        entry.artifact_path = root.join(&native_path);
        entry.package = Some(PackageMetadata {
            digest,
            root,
            target,
            native_path,
        });
        let mut registry = Registry::default();
        registry.plugins.push(entry);
        store.validate(&registry).unwrap();
        let roundtrip: Registry =
            serde_json::from_slice(&serde_json::to_vec(&registry).unwrap()).unwrap();
        assert_eq!(roundtrip.plugins[0].package, registry.plugins[0].package);
        assert_ne!(
            registry.plugins[0].sha256,
            registry.plugins[0].package.as_ref().unwrap().digest
        );
        let mut wrong_digest = registry.clone();
        wrong_digest.plugins[0].package.as_mut().unwrap().digest = "c".repeat(64);
        assert!(store.validate(&wrong_digest).is_err());
        let mut escaped = registry.clone();
        escaped.plugins[0].package.as_mut().unwrap().native_path = "../outside".into();
        assert!(store.validate(&escaped).is_err());
        let mut wrong_native = registry.clone();
        wrong_native.plugins[0].artifact_path = store.root.join("packages/elsewhere/plugin.dylib");
        assert!(store.validate(&wrong_native).is_err());
        let mut wrong_target = registry;
        wrong_target.plugins[0].package.as_mut().unwrap().target = "other-vendor-target".into();
        assert!(store.validate(&wrong_target).is_err());
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
            sha256: "a".repeat(64), package: None, desired_enabled: true,
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
