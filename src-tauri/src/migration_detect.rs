//! Read-only probe for data the pre-rename brand left behind (T-MIG-DETECT).
//!
//! # What this module is allowed to do
//!
//! Look, and report. Nothing here creates, copies, moves or deletes anything,
//! and nothing here can fail: every probe that hits an error logs it and
//! answers "nothing found", because a detector that can return an error is a
//! detector that can stop the app from starting. The user's data is still on
//! disk either way; refusing to launch would not make it any more reachable.
//!
//! The consequence is deliberate and worth naming: a probe that silently
//! answers "absent" when it really means "I could not tell" costs the user a
//! *prompt*, never data. Every legacy root stays exactly where it is
//! (FORBID-05), so a missed detection is recoverable on the next start.
//!
//! # Every probe is gated on the rename having landed
//!
//! A file at `~/.ssh/<`[`brand::LEGACY::ssh_known_hosts_file`]`>` existing is not
//! evidence of legacy data on a build that still writes that exact name — it is
//! the live store. So each probe first asks whether [`brand::LEGACY`] and
//! [`brand::canonical`] even differ for the identifier it is about, and reports
//! "not present" when they do not. For an identifier Wave 5 has not flipped yet,
//! that makes its probe empty, which is the correct answer: nothing has been
//! renamed for it.
//!
//! [`brand::LEGACY::ssh_known_hosts_file`]: crate::brand::BrandCanonical::ssh_known_hosts_file
//!
//! # M-15 is the one root detection does not trigger
//!
//! The app-managed SSH host-key store migrates unconditionally at startup
//! ([`crate::ssh::known_hosts_migration::run_at_startup`]) because the window it
//! closes — every previously trusted host reading as unknown, and `accept-new`
//! re-trusting whatever answers — opens the moment the app runs, whether or not
//! the user ever opens the merge banner. This module only *reads* the outcome
//! that pass recorded and reports it. It never calls the migration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::brand;
use crate::commands::IpcResult;
use crate::credentials;
use crate::ssh::credential_store;
use crate::ssh::known_hosts_migration::{self, KnownHostsMigration, StartupOutcome};
use crate::webview_storage_handoff::{self, HostPlatform};
use crate::{legacy_appdata, macos_permissions};

/// The `tauri-plugin-store` file the renderer persists projects into.
///
/// Not reachable as a constant: the name lives in a renderer module
/// (`src/renderer/lib/tauri-persistence-api.ts`) that this crate cannot import.
/// Pinned instead by
/// [`tests::the_persistence_store_file_matches_the_renderer_source`], so a
/// rename there is a red test here rather than a keychain enumeration that
/// silently finds no projects.
pub const PERSISTENCE_STORE_FILE: &str = "termul-data.json";

/// Key under which the renderer writes [`PersistedProjectData`-shaped] project
/// state. Mirrors `PersistenceKeys.projects` in
/// `src/shared/types/persistence.types.ts`.
const PERSISTED_PROJECTS_KEY: &str = "projects";

/// Every legacy root the detector knows how to look for.
///
/// The wire strings are the contract the renderer's `LegacySignalKind` union is
/// written against; [`tests::the_wire_shape_is_exactly_what_the_renderer_reads`]
/// pins them so a Rust-side identifier rename cannot change the JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LegacySignalKind {
    /// M-01, M-02 — the pre-rename `app_data_dir` trees, prod and dev.
    AppDataDir,
    /// M-06 — `~/Documents/<pre-rename display name>`.
    DocumentsWorkspace,
    /// M-07 — the standalone server's state root.
    StandaloneStateRoot,
    /// M-09, M-10 — the two pre-rename desktop keychain services.
    KeychainService,
    /// M-05 — WebView storage / renderer `localStorage` under the old prefix.
    LocalStorage,
    /// M-08, M-12 — `<repo>/<pre-rename workspace dir>` in the user's own repos.
    RepoWorkspaceDir,
    /// M-15 — the app-managed SSH host-key store. Reported, never triggered.
    SshKnownHosts,
}

impl LegacySignalKind {
    /// The human label the banner renders. Host-side so the two surfaces cannot
    /// drift into naming the same root differently.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AppDataDir => "Application data",
            Self::DocumentsWorkspace => "Session workspaces",
            Self::StandaloneStateRoot => "Standalone server state",
            Self::KeychainService => "Keychain credentials",
            Self::LocalStorage => "WebView storage",
            Self::RepoWorkspaceDir => "Repository workspace directories",
            Self::SshKnownHosts => "SSH host-key store",
        }
    }
}

/// One probed root. `present: false` means the detector looked and found
/// nothing — including "looked, hit an error, and is reporting the safe answer".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDataSignal {
    pub kind: LegacySignalKind,
    pub label: String,
    /// Absolute path, or `null` for roots that have none (the keychain).
    pub path: Option<String>,
    pub present: bool,
}

impl LegacyDataSignal {
    fn absent(kind: LegacySignalKind) -> Self {
        Self {
            kind,
            label: kind.label().to_string(),
            path: None,
            present: false,
        }
    }

    fn found(kind: LegacySignalKind, path: Option<&Path>) -> Self {
        Self {
            kind,
            label: kind.label().to_string(),
            path: path.map(|value| value.display().to_string()),
            present: true,
        }
    }
}

/// Outcome of the unconditional startup migration of the SSH host-key store.
///
/// Internally tagged on `state` to match the renderer's discriminated union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SshKnownHostsStatus {
    /// The store is in place under the current name.
    Migrated,
    /// There was nothing to carry.
    Skipped,
    /// The current name is already the name on disk — no rename to carry across.
    NotApplicable,
    /// The pass failed. The fail-closed interlock is set for this process.
    Failed { reason: String },
}

/// Payload of `detect_legacy_brand_data`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDataDetection {
    pub has_legacy_data: bool,
    pub signals: Vec<LegacyDataSignal>,
    /// Result of the STARTUP pass — already done, never part of the prompt.
    pub ssh_known_hosts: SshKnownHostsStatus,
    /// macOS-only M-13 privacy-grant notice; `null` on every other platform.
    pub tcc_notice: Option<String>,
}

/// The paths a probe needs, resolved by the caller.
///
/// Passed in rather than resolved inside each probe so the whole detector is
/// exercisable against a temporary tree, and so the resolution happens once on
/// the thread that owns the brand seam (FORBID-07).
#[derive(Debug, Clone)]
pub struct LegacyRoots {
    /// Tauri's `app_data_dir()` for the *current* identifier.
    pub app_data_dir: PathBuf,
    /// `app_local_data_dir()`. Only Windows keeps WebView storage outside
    /// `app_data_dir`, but the field is unconditional so the Windows arm is
    /// reachable from a test on any host.
    pub app_local_data_dir: Option<PathBuf>,
    /// `<documents>/<current display name>`.
    pub workspace_base: PathBuf,
    /// Parent of the standalone server's state root, when this host has one.
    pub state_root_parent: Option<PathBuf>,
    /// Roots of the repositories the user has opened as projects.
    pub project_roots: Vec<PathBuf>,
    /// The user's `~/.ssh`, when it is locatable.
    pub ssh_dir: Option<PathBuf>,
    /// Which of the three WebView-storage answers applies.
    pub platform: HostPlatform,
}

impl LegacyRoots {
    /// Resolve from a live Tauri handle.
    ///
    /// `Err` only when `app_data_dir()` itself is unavailable, which is the one
    /// path with nothing to anchor the other roots to.
    pub fn resolve(handle: &tauri::AppHandle) -> Result<Self, String> {
        let app_data_dir = handle
            .path()
            .app_data_dir()
            .map_err(|error| format!("app data directory is unavailable: {error}"))?;
        let workspace_base = std::env::var("SE_CONVERSATION_WORKSPACE_ROOT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                handle
                    .path()
                    .document_dir()
                    .ok()
                    .map(|path| path.join(brand::canonical().display_name))
            })
            .unwrap_or_else(|| app_data_dir.join(brand::canonical().display_name));
        Ok(Self {
            project_roots: persisted_project_roots(&app_data_dir),
            app_local_data_dir: handle.path().app_local_data_dir().ok(),
            workspace_base,
            state_root_parent: crate::web::config::state_root_parent(),
            ssh_dir: ssh_dir(),
            platform: HostPlatform::host(),
            app_data_dir,
        })
    }

    /// A detection over roots that are all absent — the honest answer when the
    /// caller could not even resolve `app_data_dir`.
    fn nothing_found() -> LegacyDataDetection {
        LegacyDataDetection {
            has_legacy_data: false,
            signals: ALL_KINDS
                .iter()
                .copied()
                .map(LegacyDataSignal::absent)
                .collect(),
            ssh_known_hosts: ssh_known_hosts_status(
                known_hosts_migration::startup_outcome().as_ref(),
            ),
            tcc_notice: macos_permissions::tcc_grants_reset_notice().map(str::to_string),
        }
    }
}

/// Every kind, in the order the banner lists them.
pub const ALL_KINDS: [LegacySignalKind; 7] = [
    LegacySignalKind::AppDataDir,
    LegacySignalKind::DocumentsWorkspace,
    LegacySignalKind::StandaloneStateRoot,
    LegacySignalKind::KeychainService,
    LegacySignalKind::LocalStorage,
    LegacySignalKind::RepoWorkspaceDir,
    LegacySignalKind::SshKnownHosts,
];

/// The user's `~/.ssh`, resolved the same way the startup migration does.
fn ssh_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".ssh"))
}

/// Probe every root. Never fails, never writes.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
#[must_use]
pub fn detect_legacy_data(roots: &LegacyRoots) -> LegacyDataDetection {
    let signals = vec![
        probe_app_data_dir(roots),
        probe_documents_workspace(roots),
        probe_standalone_state_root(roots),
        probe_keychain_service(roots),
        probe_local_storage(roots),
        probe_repo_workspace_dir(roots),
        probe_ssh_known_hosts(roots),
    ];
    LegacyDataDetection {
        has_legacy_data: signals.iter().any(|signal| signal.present),
        signals,
        // Read, not run. See the module doc.
        ssh_known_hosts: ssh_known_hosts_status(known_hosts_migration::startup_outcome().as_ref()),
        tcc_notice: macos_permissions::tcc_grants_reset_notice().map(str::to_string),
    }
}

/// Map the recorded startup outcome onto the wire status.
///
/// Pure and separate from the process-wide record so it is testable without
/// touching a global that sibling tests share.
#[must_use]
pub fn ssh_known_hosts_status(outcome: Option<&StartupOutcome>) -> SshKnownHostsStatus {
    match outcome {
        // `AlreadyMigrated` is "the store is under the current name", reached by
        // an earlier run rather than this one. From a user's point of view that
        // is migrated, and calling it "skipped" would read as work still owed.
        Some(StartupOutcome::Completed(
            KnownHostsMigration::Copied { .. } | KnownHostsMigration::AlreadyMigrated,
        )) => SshKnownHostsStatus::Migrated,
        Some(StartupOutcome::Completed(KnownHostsMigration::NotNeeded)) => {
            SshKnownHostsStatus::Skipped
        }
        Some(StartupOutcome::Completed(KnownHostsMigration::NotApplicable)) => {
            SshKnownHostsStatus::NotApplicable
        }
        Some(StartupOutcome::Failed { reason }) => SshKnownHostsStatus::Failed {
            reason: reason.clone(),
        },
        // The pass never ran in this process — every composition that is not the
        // desktop app. There is no rename to report across.
        None => SshKnownHostsStatus::NotApplicable,
    }
}

// ---------------------------------------------------------------------------
// The seven probes
// ---------------------------------------------------------------------------

/// M-01, M-02 — both pre-rename identifier trees, prod and dev.
fn probe_app_data_dir(roots: &LegacyRoots) -> LegacyDataSignal {
    match legacy_appdata::legacy_appdata_roots(&roots.app_data_dir).first() {
        Some(found) => LegacyDataSignal::found(LegacySignalKind::AppDataDir, Some(found)),
        None => LegacyDataSignal::absent(LegacySignalKind::AppDataDir),
    }
}

/// M-06 — `~/Documents/<pre-rename display name>`.
fn probe_documents_workspace(roots: &LegacyRoots) -> LegacyDataSignal {
    match crate::conversation::bootstrap::legacy_workspace_base(&roots.workspace_base) {
        Some(found) => LegacyDataSignal::found(LegacySignalKind::DocumentsWorkspace, Some(&found)),
        None => LegacyDataSignal::absent(LegacySignalKind::DocumentsWorkspace),
    }
}

/// M-07 — the standalone server's state root under its pre-rename name.
fn probe_standalone_state_root(roots: &LegacyRoots) -> LegacyDataSignal {
    let kind = LegacySignalKind::StandaloneStateRoot;
    let Some(parent) = roots.state_root_parent.as_deref() else {
        return LegacyDataSignal::absent(kind);
    };
    match crate::web::config::legacy_state_root_pair(parent) {
        Some((legacy, _)) if legacy.is_dir() => LegacyDataSignal::found(kind, Some(&legacy)),
        _ => LegacyDataSignal::absent(kind),
    }
}

/// M-09, M-10 — a pre-rename keychain service still holding a readable entry.
///
/// The probe reads; it never writes and never carries anything forward. Reading
/// a credential is also the one probe that can fail for a reason the user could
/// act on (a locked keychain), so the failure is logged with its cause and
/// answered as "absent" rather than propagated.
fn probe_keychain_service(roots: &LegacyRoots) -> LegacyDataSignal {
    let kind = LegacySignalKind::KeychainService;
    let backend = credentials::backend();
    for (scope, key) in legacy_credential_keys(&roots.app_data_dir) {
        let (legacy_service, canonical_service) = scope.services();
        if legacy_service == canonical_service {
            continue;
        }
        match backend.get(legacy_service, &key) {
            // The path stays `None`: a keychain has no path, and the key names
            // a project and an environment variable the banner has no business
            // printing.
            Ok(Some(_)) => return LegacyDataSignal::found(kind, None),
            Ok(None) => {}
            Err(error) => log::warn!(
                "[brand-migration] could not read the pre-rename keychain service: {error}"
            ),
        }
    }
    LegacyDataSignal::absent(kind)
}

/// M-05 — WebView storage / renderer `localStorage` under the old prefix.
///
/// Rust cannot read `localStorage` directly, so the probe looks at the two
/// artefacts that stand in for it, in the order they can exist:
///
/// 1. Windows keeps the WebView2 store in `%LOCALAPPDATA%\{id}\EBWebView`,
///    outside `app_data_dir` — a pre-rename copy of that directory is direct
///    evidence.
/// 2. On every platform, an unconsumed handoff file holding app-owned keys is
///    what the macOS replay leg is waiting for. It is looked for under the
///    current root (where M-02 will have carried it) and under the pre-rename
///    roots (where it was written).
fn probe_local_storage(roots: &LegacyRoots) -> LegacyDataSignal {
    let kind = LegacySignalKind::LocalStorage;
    if roots.platform == HostPlatform::Windows {
        if let Some(local) = roots.app_local_data_dir.as_deref() {
            if let Some(legacy) = legacy_appdata::matching_legacy_root(local) {
                let store = legacy.join(webview_storage_handoff::WEBVIEW_DATA_DIR);
                if store.is_dir() {
                    return LegacyDataSignal::found(kind, Some(&store));
                }
            }
        }
    }
    let bases = std::iter::once(roots.app_data_dir.clone())
        .chain(legacy_appdata::legacy_appdata_roots(&roots.app_data_dir));
    for base in bases {
        let Some(handoff) = webview_storage_handoff::pending(&base) else {
            continue;
        };
        if !handoff.entries.is_empty() {
            return LegacyDataSignal::found(
                kind,
                Some(&webview_storage_handoff::handoff_path(&base)),
            );
        }
    }
    LegacyDataSignal::absent(kind)
}

/// M-08, M-12 — `<repo>/<pre-rename workspace dir>` in the user's own repos.
fn probe_repo_workspace_dir(roots: &LegacyRoots) -> LegacyDataSignal {
    let kind = LegacySignalKind::RepoWorkspaceDir;
    let legacy_name = brand::LEGACY.workspace_dir;
    if legacy_name == brand::canonical().workspace_dir {
        return LegacyDataSignal::absent(kind);
    }
    match roots
        .project_roots
        .iter()
        .map(|root| root.join(legacy_name))
        .find(|candidate| candidate.is_dir())
    {
        Some(found) => LegacyDataSignal::found(kind, Some(&found)),
        None => LegacyDataSignal::absent(kind),
    }
}

/// M-15 — the pre-rename host-key store. Reported only; never carried here.
fn probe_ssh_known_hosts(roots: &LegacyRoots) -> LegacyDataSignal {
    let kind = LegacySignalKind::SshKnownHosts;
    let legacy_name = brand::LEGACY.ssh_known_hosts_file;
    if legacy_name == brand::canonical().ssh_known_hosts_file {
        return LegacyDataSignal::absent(kind);
    }
    let Some(directory) = roots.ssh_dir.as_deref() else {
        return LegacyDataSignal::absent(kind);
    };
    let legacy = directory.join(legacy_name);
    if legacy.is_file() {
        LegacyDataSignal::found(kind, Some(&legacy))
    } else {
        LegacyDataSignal::absent(kind)
    }
}

// ---------------------------------------------------------------------------
// Keychain key enumeration, shared with the merge orchestrator
// ---------------------------------------------------------------------------

/// Which service pair a key belongs to. A keychain is keyed by
/// `(service, key)`, so the pair has to travel with the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialScope {
    /// `brand::*.keychain_service` — project environment-variable secrets (M-09).
    General,
    /// `brand::*.keychain_ssh_service` — SSH passwords and passphrases (M-10).
    Ssh,
}

impl CredentialScope {
    /// `(pre-rename service, current service)`.
    ///
    /// Reads the brand seam, so it must be called on the thread that owns it
    /// (FORBID-07).
    #[must_use]
    pub fn services(self) -> (&'static str, &'static str) {
        match self {
            Self::General => (
                brand::LEGACY.keychain_service,
                brand::canonical().keychain_service,
            ),
            Self::Ssh => (
                brand::LEGACY.keychain_ssh_service,
                brand::canonical().keychain_ssh_service,
            ),
        }
    }
}

/// Every keychain key a pre-rename install may have written, derived from the
/// state it wrote alongside them.
///
/// There is no way to list a keychain service's entries through the `keyring`
/// crate, so the key set has to be reconstructed: projects × their secret
/// environment variables, and SSH profile ids × the two credential suffixes.
/// A key that cannot be reconstructed is a credential the merge cannot carry —
/// which is why the two on-disk shapes this reads are pinned by parity tests
/// rather than trusted to stay put.
#[must_use]
pub fn legacy_credential_keys(app_data_dir: &Path) -> Vec<(CredentialScope, String)> {
    let mut keys = Vec::new();
    for project in persisted_projects(app_data_dir) {
        let Some(id) = project.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(vars) = project.get("envVars").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for variable in vars {
            if variable
                .get("isSecret")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                continue;
            }
            let Some(name) = variable.get("key").and_then(serde_json::Value::as_str) else {
                continue;
            };
            keys.push((CredentialScope::General, format!("project:{id}:env:{name}")));
        }
    }
    for profile in ssh_profile_ids(app_data_dir) {
        keys.push((
            CredentialScope::Ssh,
            format!("{profile}-{}", credential_store::PASSWORD_SUFFIX),
        ));
        keys.push((
            CredentialScope::Ssh,
            format!("{profile}-{}", credential_store::PASSPHRASE_SUFFIX),
        ));
    }
    keys
}

/// One `tauri-plugin-store` file, as a plain JSON object. `None` for every
/// reason a store can be unreadable — all of them mean the same thing here:
/// nothing to enumerate.
fn read_store(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!(
                "[brand-migration] could not read {}: {error}",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        Ok(_) => None,
        Err(error) => {
            log::warn!(
                "[brand-migration] {} does not parse as a store: {error}",
                path.display()
            );
            None
        }
    }
}

fn persisted_projects(app_data_dir: &Path) -> Vec<serde_json::Value> {
    read_store(&app_data_dir.join(PERSISTENCE_STORE_FILE))
        .and_then(|store| store.get(PERSISTED_PROJECTS_KEY).cloned())
        .and_then(|value| value.get(PERSISTED_PROJECTS_KEY).cloned())
        .and_then(|value| match value {
            serde_json::Value::Array(items) => Some(items),
            _ => None,
        })
        .unwrap_or_default()
}

/// The absolute paths of the repositories the user has opened as projects.
#[must_use]
pub fn persisted_project_roots(app_data_dir: &Path) -> Vec<PathBuf> {
    persisted_projects(app_data_dir)
        .iter()
        .filter_map(|project| project.get("path").and_then(serde_json::Value::as_str))
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

fn ssh_profile_ids(app_data_dir: &Path) -> Vec<String> {
    let Some(store) = read_store(&app_data_dir.join(crate::ssh::profile_manager::STORE_FILE))
    else {
        return Vec::new();
    };
    let Some(serde_json::Value::Array(profiles)) =
        store.get(crate::ssh::profile_manager::STORE_KEY)
    else {
        return Vec::new();
    };
    profiles
        .iter()
        .filter_map(|profile| profile.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

/// Read-only probe for legacy brand data.
///
/// Always reports success. A detector that can hand the renderer an error is a
/// detector that can paint a failure over a workspace for something the user
/// never asked for and cannot act on; the failure is logged instead and the
/// answer degrades to "nothing found", which costs a prompt and never data.
#[tauri::command]
pub fn detect_legacy_brand_data(app: tauri::AppHandle) -> IpcResult<LegacyDataDetection> {
    match LegacyRoots::resolve(&app) {
        Ok(roots) => IpcResult::success(detect_legacy_data(&roots)),
        Err(error) => {
            log::warn!(
                "[brand-migration] legacy-data detection could not resolve its roots: {error}. \
                 Reporting no legacy data; every legacy root is still on disk."
            );
            IpcResult::success(LegacyRoots::nothing_found())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brand::BrandCanonical;
    use crate::credentials::{CredentialBackend, CredentialError};
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::{Arc, Mutex};

    const POST_RENAME_KNOWN_HOSTS: &str = "known_hosts_se-manager";

    fn renamed() -> BrandCanonical {
        BrandCanonical {
            bundle_id: "com.se-manager.app",
            bundle_id_dev: "com.se-manager.app.dev",
            display_name: "Se Manager",
            workspace_dir: ".se-manager",
            state_dir: "se-manager",
            state_dir_windows: "SeManager",
            keychain_service: "com.se.manager",
            keychain_ssh_service: "se-ssh",
            ssh_known_hosts_file: POST_RENAME_KNOWN_HOSTS,
            storage_prefix: "se-store:",
            storage_key_prefix: "se:",
            ..brand::DEFAULT_CANONICAL
        }
    }

    /// A `(service, key)`-keyed store — what a real OS keychain is.
    #[derive(Default)]
    struct MapBackend {
        entries: Mutex<BTreeMap<(String, String), String>>,
        fail: bool,
    }

    impl MapBackend {
        fn seeded(pairs: &[(&str, &str, &str)]) -> Arc<Self> {
            let backend = Self::default();
            {
                let mut entries = backend.entries.lock().unwrap();
                for (service, key, value) in pairs {
                    entries.insert(
                        ((*service).to_string(), (*key).to_string()),
                        (*value).to_string(),
                    );
                }
            }
            Arc::new(backend)
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                entries: Mutex::default(),
                fail: true,
            })
        }
    }

    impl CredentialBackend for MapBackend {
        fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
            if self.fail {
                return Err(CredentialError::Backend("the keychain is locked".into()));
            }
            Ok(self
                .entries
                .lock()
                .unwrap()
                .get(&(service.to_string(), key.to_string()))
                .cloned())
        }

        fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
            if self.fail {
                return Err(CredentialError::Backend("the keychain is locked".into()));
            }
            self.entries
                .lock()
                .unwrap()
                .insert((service.to_string(), key.to_string()), value.to_string());
            Ok(())
        }

        fn delete(&self, service: &str, key: &str) -> Result<(), CredentialError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(service.to_string(), key.to_string()));
            Ok(())
        }
    }

    /// A tree with nothing legacy in it, under the *post*-rename brand.
    struct Fixture {
        _temp: tempfile::TempDir,
        base: PathBuf,
        roots: LegacyRoots,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let base = temp.path().canonicalize().expect("canonicalize");
            let app_data_dir = base.join("appdata").join(renamed().bundle_id);
            fs::create_dir_all(&app_data_dir).expect("app data dir");
            fs::create_dir_all(base.join("documents").join(renamed().display_name))
                .expect("workspace base");
            fs::create_dir_all(base.join("state")).expect("state parent");
            fs::create_dir_all(base.join("ssh")).expect("ssh dir");
            fs::create_dir_all(base.join("repo")).expect("repo");
            let roots = LegacyRoots {
                app_data_dir,
                app_local_data_dir: Some(base.join("localappdata").join(renamed().bundle_id)),
                workspace_base: base.join("documents").join(renamed().display_name),
                state_root_parent: Some(base.join("state")),
                project_roots: vec![base.join("repo")],
                ssh_dir: Some(base.join("ssh")),
                platform: HostPlatform::MacOs,
            };
            Self {
                _temp: temp,
                base,
                roots,
            }
        }

        fn signal(detection: &LegacyDataDetection, kind: LegacySignalKind) -> &LegacyDataSignal {
            detection
                .signals
                .iter()
                .find(|signal| signal.kind == kind)
                .unwrap_or_else(|| panic!("no signal for {kind:?}"))
        }
    }

    // -- Wire shape ---------------------------------------------------------

    /// The renderer's types are written against these exact strings. A Rust-side
    /// identifier rename must be a red test here, not a silently different
    /// payload that makes the banner render nothing.
    #[test]
    fn the_wire_shape_is_exactly_what_the_renderer_reads() {
        let detection = LegacyDataDetection {
            has_legacy_data: true,
            signals: ALL_KINDS
                .iter()
                .copied()
                .map(|kind| LegacyDataSignal {
                    kind,
                    label: kind.label().to_string(),
                    path: Some("/legacy".to_string()),
                    present: true,
                })
                .collect(),
            ssh_known_hosts: SshKnownHostsStatus::Failed {
                reason: "disk full".to_string(),
            },
            tcc_notice: Some("notice".to_string()),
        };
        let json = serde_json::to_value(&detection).expect("serialize");

        assert_eq!(json["hasLegacyData"], serde_json::json!(true));
        assert_eq!(json["tccNotice"], serde_json::json!("notice"));
        assert_eq!(
            json["sshKnownHosts"],
            serde_json::json!({"state": "failed", "reason": "disk full"})
        );
        let kinds: Vec<&str> = json["signals"]
            .as_array()
            .expect("signals array")
            .iter()
            .map(|signal| signal["kind"].as_str().expect("kind string"))
            .collect();
        assert_eq!(
            kinds,
            [
                "appDataDir",
                "documentsWorkspace",
                "standaloneStateRoot",
                "keychainService",
                "localStorage",
                "repoWorkspaceDir",
                "sshKnownHosts",
            ]
        );
        assert_eq!(json["signals"][0]["present"], serde_json::json!(true));
        assert_eq!(json["signals"][0]["path"], serde_json::json!("/legacy"));

        // Round-trips, so the payload is a shape and not a one-way format.
        let decoded: LegacyDataDetection = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, detection);
    }

    #[test]
    fn the_three_non_failure_ssh_states_are_bare_tags() {
        for (status, expected) in [
            (SshKnownHostsStatus::Migrated, "migrated"),
            (SshKnownHostsStatus::Skipped, "skipped"),
            (SshKnownHostsStatus::NotApplicable, "notApplicable"),
        ] {
            assert_eq!(
                serde_json::to_value(&status).expect("serialize"),
                serde_json::json!({ "state": expected })
            );
        }
    }

    /// The keychain enumeration reconstructs a key shape that lives in a
    /// renderer file this crate cannot import. If that literal moves, the merge
    /// stops finding any project secret and reports "nothing to carry" — a
    /// silent failure. This makes it loud.
    #[test]
    fn the_persistence_store_file_matches_the_renderer_source() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/renderer/lib/tauri-persistence-api.ts");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        assert!(
            source.contains(&format!("const STORE_FILE = '{PERSISTENCE_STORE_FILE}'")),
            "{} no longer declares STORE_FILE = '{PERSISTENCE_STORE_FILE}'",
            path.display()
        );
    }

    /// Both directions of the same contract. The renderer *writes* the secret
    /// under a key it builds from a template literal; this crate has to
    /// reconstruct that exact key to carry it. So the template is lifted out of
    /// the renderer source, instantiated, and compared against what production
    /// actually enumerates — a change on either side is red.
    #[test]
    fn the_project_secret_key_shape_matches_the_renderer_source() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/renderer/hooks/use-projects-persistence.ts");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let template = source
            .lines()
            .find_map(|line| line.trim().strip_prefix("return `")?.strip_suffix("`"))
            .unwrap_or_else(|| {
                panic!(
                    "{} no longer returns a template literal from \
                     getSecureStorageKey; the enumeration cannot be pinned to it",
                    path.display()
                )
            });
        let expected = template
            .replace("${projectId}", "p1")
            .replace("${envKey}", "TOKEN");

        let temp = tempfile::tempdir().expect("tempdir");
        seed_project_secret(temp.path());
        let keys = legacy_credential_keys(temp.path());

        assert_eq!(
            keys.first(),
            Some(&(CredentialScope::General, expected.clone())),
            "the renderer writes project secrets under {expected:?}; the merge \
             enumerates something else and would carry nothing"
        );
    }

    // -- Seven signals, present and absent ---------------------------------

    #[test]
    fn app_data_dir_is_detected_when_a_pre_rename_identifier_tree_exists() {
        let fixture = Fixture::new();
        let legacy = fixture.base.join("appdata").join(brand::LEGACY.bundle_id);
        fs::create_dir_all(&legacy).expect("legacy root");
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::AppDataDir);
        assert!(signal.present);
        assert_eq!(
            signal.path.as_deref(),
            Some(legacy.display().to_string()).as_deref()
        );
        assert!(detection.has_legacy_data);
    }

    #[test]
    fn app_data_dir_is_absent_without_a_pre_rename_identifier_tree() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        assert!(!Fixture::signal(&detection, LegacySignalKind::AppDataDir).present);
        assert!(!detection.has_legacy_data);
    }

    #[test]
    fn documents_workspace_is_detected_when_the_pre_rename_root_exists() {
        let fixture = Fixture::new();
        let legacy = fixture
            .base
            .join("documents")
            .join(brand::LEGACY.display_name);
        fs::create_dir_all(&legacy).expect("legacy workspace base");
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::DocumentsWorkspace);
        assert!(signal.present);
        assert_eq!(signal.path, Some(legacy.display().to_string()));
    }

    #[test]
    fn documents_workspace_is_absent_without_the_pre_rename_root() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::DocumentsWorkspace
            )
            .present
        );
    }

    #[test]
    fn standalone_state_root_is_detected_when_the_pre_rename_root_exists() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        let (legacy, _) = crate::web::config::legacy_state_root_pair(
            fixture.roots.state_root_parent.as_deref().expect("parent"),
        )
        .expect("a rename has landed");
        fs::create_dir_all(&legacy).expect("legacy state root");

        let signal = {
            let detection = detect_legacy_data(&fixture.roots);
            Fixture::signal(&detection, LegacySignalKind::StandaloneStateRoot).clone()
        };
        assert!(signal.present);
        assert_eq!(signal.path, Some(legacy.display().to_string()));
    }

    #[test]
    fn standalone_state_root_is_absent_without_the_pre_rename_root() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::StandaloneStateRoot
            )
            .present
        );
    }

    /// Seeds a project with one secret env var, so the enumeration has a key.
    fn seed_project_secret(app_data_dir: &Path) -> String {
        let store = serde_json::json!({
            "projects": {
                "projects": [{
                    "id": "p1",
                    "name": "demo",
                    "path": "/tmp/demo",
                    "envVars": [
                        {"key": "TOKEN", "value": "[REDACTED]", "isSecret": true},
                        {"key": "PLAIN", "value": "visible", "isSecret": false}
                    ]
                }],
                "activeProjectId": "p1",
                "updatedAt": "2026-01-01T00:00:00.000Z"
            }
        });
        fs::write(
            app_data_dir.join(PERSISTENCE_STORE_FILE),
            serde_json::to_vec(&store).expect("serialize store"),
        )
        .expect("write store");
        "project:p1:env:TOKEN".to_string()
    }

    #[test]
    fn keychain_service_is_detected_when_a_pre_rename_entry_is_readable() {
        let fixture = Fixture::new();
        let key = seed_project_secret(&fixture.roots.app_data_dir);
        let _brand = brand::override_canonical(renamed());
        let _backend = credentials::override_backend(MapBackend::seeded(&[(
            brand::LEGACY.keychain_service,
            &key,
            "s3cret",
        )]));

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::KeychainService);
        assert!(signal.present);
        assert_eq!(
            signal.path, None,
            "a keychain has no path, and the key names a project and an env var"
        );
    }

    #[test]
    fn keychain_service_is_absent_when_the_pre_rename_service_holds_nothing() {
        let fixture = Fixture::new();
        let key = seed_project_secret(&fixture.roots.app_data_dir);
        let _brand = brand::override_canonical(renamed());
        // Present under the CURRENT service only: already carried, nothing owed.
        let _backend = credentials::override_backend(MapBackend::seeded(&[(
            renamed().keychain_service,
            &key,
            "s3cret",
        )]));

        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::KeychainService
            )
            .present
        );
    }

    #[test]
    fn local_storage_is_detected_from_an_unconsumed_handoff() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        webview_storage_handoff::capture(
            &fixture.roots.app_data_dir,
            brand::LEGACY.bundle_id,
            [(
                format!("{}theme", brand::LEGACY.storage_key_prefix),
                "dark".to_string(),
            )],
            &renamed(),
        )
        .expect("capture");

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::LocalStorage);
        assert!(signal.present);
        assert_eq!(
            signal.path,
            Some(
                webview_storage_handoff::handoff_path(&fixture.roots.app_data_dir)
                    .display()
                    .to_string()
            )
        );
    }

    #[test]
    fn local_storage_is_absent_without_a_handoff_or_a_pre_rename_webview_store() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::LocalStorage
            )
            .present
        );
    }

    /// The Windows arm is reachable on any host because the platform is a field
    /// rather than a `cfg`. Without this the branch would only ever be compiled,
    /// never executed, on the machine this suite runs on.
    #[test]
    fn local_storage_is_detected_from_a_pre_rename_webview2_store_on_windows() {
        let mut fixture = Fixture::new();
        fixture.roots.platform = HostPlatform::Windows;
        let store = fixture
            .base
            .join("localappdata")
            .join(brand::LEGACY.bundle_id)
            .join(webview_storage_handoff::WEBVIEW_DATA_DIR);
        fs::create_dir_all(&store).expect("legacy EBWebView");
        fs::create_dir_all(fixture.base.join("localappdata").join(renamed().bundle_id))
            .expect("canonical local root");
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::LocalStorage);
        assert!(signal.present, "the pre-rename WebView2 store was not seen");
        assert_eq!(signal.path, Some(store.display().to_string()));
    }

    #[test]
    fn repo_workspace_dir_is_detected_in_an_opened_project() {
        let fixture = Fixture::new();
        let legacy = fixture.base.join("repo").join(brand::LEGACY.workspace_dir);
        fs::create_dir_all(&legacy).expect("legacy workspace dir");
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::RepoWorkspaceDir);
        assert!(signal.present);
        assert_eq!(signal.path, Some(legacy.display().to_string()));
    }

    #[test]
    fn repo_workspace_dir_is_absent_when_no_opened_project_has_one() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.base.join("repo").join(renamed().workspace_dir))
            .expect("current workspace dir only");
        let _brand = brand::override_canonical(renamed());
        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::RepoWorkspaceDir
            )
            .present
        );
    }

    #[test]
    fn ssh_known_hosts_is_detected_when_the_pre_rename_store_exists() {
        let fixture = Fixture::new();
        let legacy = fixture
            .base
            .join("ssh")
            .join(brand::LEGACY.ssh_known_hosts_file);
        fs::write(&legacy, b"host ssh-ed25519 AAAA\n").expect("legacy store");
        let _brand = brand::override_canonical(renamed());

        let detection = detect_legacy_data(&fixture.roots);
        let signal = Fixture::signal(&detection, LegacySignalKind::SshKnownHosts);
        assert!(signal.present);
        assert_eq!(signal.path, Some(legacy.display().to_string()));
    }

    #[test]
    fn ssh_known_hosts_is_absent_without_a_pre_rename_store() {
        let fixture = Fixture::new();
        let _brand = brand::override_canonical(renamed());
        assert!(
            !Fixture::signal(
                &detect_legacy_data(&fixture.roots),
                LegacySignalKind::SshKnownHosts
            )
            .present
        );
    }

    /// Every probe has to answer "absent" while the two brands are still the
    /// same values, or a user whose live data sits under exactly those names is
    /// told they have legacy data to merge — which would be their live data.
    ///
    /// This needed no injection until Wave 5 began flipping `DEFAULT_CANONICAL`,
    /// because `canonical == LEGACY` then held for every identifier by itself.
    /// Injecting `LEGACY` wholesale reproduces that build exactly, which is the
    /// condition the gates are about; asserting it against today's half-flipped
    /// shipped values would be asserting something else.
    #[test]
    fn nothing_is_legacy_before_the_rename_lands() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.base.join("repo").join(brand::LEGACY.workspace_dir))
            .expect("workspace dir");
        fs::write(
            fixture
                .base
                .join("ssh")
                .join(brand::LEGACY.ssh_known_hosts_file),
            b"host ssh-ed25519 AAAA\n",
        )
        .expect("store");
        let _brand = brand::override_canonical(brand::LEGACY);
        let detection = detect_legacy_data(&fixture.roots);
        assert!(
            !detection.has_legacy_data,
            "reported legacy data on an un-renamed build: {:?}",
            detection
                .signals
                .iter()
                .filter(|signal| signal.present)
                .collect::<Vec<_>>()
        );
    }

    // -- Failure never escapes ---------------------------------------------

    /// A probe that can return an error is a probe that can stop the app from
    /// starting. The keychain is the one that really can fail, so it is the one
    /// driven here: a backend that errors on every read must produce a complete
    /// detection with that signal absent, not a panic and not an `Err`.
    #[test]
    fn a_failing_probe_degrades_to_absent_instead_of_failing() {
        let fixture = Fixture::new();
        seed_project_secret(&fixture.roots.app_data_dir);
        let legacy = fixture.base.join("appdata").join(brand::LEGACY.bundle_id);
        fs::create_dir_all(&legacy).expect("legacy root");
        let _brand = brand::override_canonical(renamed());
        let _backend = credentials::override_backend(MapBackend::failing());

        let detection = detect_legacy_data(&fixture.roots);
        assert_eq!(detection.signals.len(), ALL_KINDS.len());
        assert!(
            !Fixture::signal(&detection, LegacySignalKind::KeychainService).present,
            "a keychain that cannot be read must read as absent, not as present"
        );
        assert!(
            Fixture::signal(&detection, LegacySignalKind::AppDataDir).present,
            "one failing probe must not take the others down with it"
        );
        assert!(detection.has_legacy_data);
    }

    // -- M-15 is not detection-triggered -----------------------------------

    /// Two legs, and both have to hold:
    ///
    /// 1. The startup pass copies the store with detection never being called.
    /// 2. Detection over a directory holding a pre-rename store writes nothing.
    ///
    /// Leg 2 is what fails if anyone "unifies" the roots by having the detector
    /// trigger this migration — which would hand a security root's timing to a
    /// user's click.
    #[test]
    fn the_ssh_store_copy_is_not_triggered_by_detection() {
        let fixture = Fixture::new();
        let started_by = fixture.base.join("startup-ssh");
        fs::create_dir_all(&started_by).expect("startup ssh dir");
        fs::write(
            started_by.join(brand::LEGACY.ssh_known_hosts_file),
            b"host ssh-ed25519 AAAA\n",
        )
        .expect("plant");
        let _brand = brand::override_canonical(renamed());

        // Leg 1 — startup, with no detection anywhere in sight.
        known_hosts_migration::migrate_app_known_hosts_in(&started_by).expect("startup pass");
        assert!(
            started_by.join(POST_RENAME_KNOWN_HOSTS).is_file(),
            "the startup pass did not carry the store"
        );

        // Leg 2 — detection, over a directory the startup pass never touched.
        let probed = fixture.roots.ssh_dir.clone().expect("ssh dir");
        fs::write(
            probed.join(brand::LEGACY.ssh_known_hosts_file),
            b"host ssh-ed25519 AAAA\n",
        )
        .expect("plant");
        let detection = detect_legacy_data(&fixture.roots);

        assert!(
            Fixture::signal(&detection, LegacySignalKind::SshKnownHosts).present,
            "detection must still report the store it found"
        );
        assert!(
            !probed.join(POST_RENAME_KNOWN_HOSTS).exists(),
            "detection carried the host-key store forward; that copy belongs to \
             startup, not to a probe"
        );
    }

    // -- The startup outcome is read, not recomputed ------------------------

    #[test]
    fn the_startup_outcome_maps_onto_the_wire_status() {
        assert_eq!(
            ssh_known_hosts_status(Some(&StartupOutcome::Completed(
                KnownHostsMigration::Copied { bytes: 12 }
            ))),
            SshKnownHostsStatus::Migrated
        );
        assert_eq!(
            ssh_known_hosts_status(Some(&StartupOutcome::Completed(
                KnownHostsMigration::AlreadyMigrated
            ))),
            SshKnownHostsStatus::Migrated
        );
        assert_eq!(
            ssh_known_hosts_status(Some(&StartupOutcome::Completed(
                KnownHostsMigration::NotNeeded
            ))),
            SshKnownHostsStatus::Skipped
        );
        assert_eq!(
            ssh_known_hosts_status(Some(&StartupOutcome::Completed(
                KnownHostsMigration::NotApplicable
            ))),
            SshKnownHostsStatus::NotApplicable
        );
        assert_eq!(
            ssh_known_hosts_status(Some(&StartupOutcome::Failed {
                reason: "cannot write /Users/x/.ssh/known_hosts_se-manager".to_string()
            })),
            SshKnownHostsStatus::Failed {
                reason: "cannot write /Users/x/.ssh/known_hosts_se-manager".to_string()
            }
        );
        assert_eq!(
            ssh_known_hosts_status(None),
            SshKnownHostsStatus::NotApplicable
        );
    }

    #[test]
    fn the_credential_enumeration_covers_project_secrets_and_ssh_profiles() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_data_dir = temp.path();
        seed_project_secret(app_data_dir);
        fs::write(
            app_data_dir.join(crate::ssh::profile_manager::STORE_FILE),
            serde_json::to_vec(&serde_json::json!({
                "profiles": [{"id": "prof-1", "name": "box"}]
            }))
            .expect("serialize"),
        )
        .expect("write ssh store");

        let keys = legacy_credential_keys(app_data_dir);
        assert_eq!(
            keys,
            vec![
                (CredentialScope::General, "project:p1:env:TOKEN".to_string()),
                (CredentialScope::Ssh, "prof-1-password".to_string()),
                (CredentialScope::Ssh, "prof-1-passphrase".to_string()),
            ],
            "a non-secret env var must not produce a key, and both SSH suffixes must"
        );
    }

    #[test]
    fn the_credential_enumeration_is_empty_when_no_state_was_persisted() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(legacy_credential_keys(temp.path()).is_empty());
    }
}
