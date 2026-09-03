//! User-initiated merge of the pre-rename brand roots (T-MIG-RUN).
//!
//! # What this is, and what it deliberately is not
//!
//! One pass over the legacy roots, under the existing cross-process host
//! migration lock, producing a receipt the banner can render. It is *not* a
//! `copy_dir` over one big root: the fourteen roots in the migration plan have
//! genuinely different strategies, and three of them are "do nothing, on
//! purpose". Collapsing them would turn a decision into an accident.
//!
//! Every copy here is [`legacy_appdata::carry_forward`], which is copy-only,
//! never overwrites a destination, never deletes a source and skips symlinks
//! rather than following them out of the tree. FORBID-05 therefore holds by
//! construction rather than by a second implementation agreeing to behave, and
//! the second pass is a no-op for the same reason — a destination that already
//! exists wins, so the user can click the button as many times as they like.
//!
//! # A failing root does not take the others down
//!
//! Each root produces its own row. A row that failed says so, with the reason,
//! and the pass keeps going. The alternative — abort on the first failure —
//! would let one unwritable directory strand every other root behind it, and
//! the user would have no way to tell which.
//!
//! # M-15 is read here, never run here
//!
//! The SSH host-key store migrates unconditionally at startup
//! ([`crate::ssh::known_hosts_migration::run_at_startup`]). This orchestrator
//! carries its recorded outcome into the receipt and calls nothing. Making that
//! root wait for a click would mean the window it exists to close — every
//! trusted host reading as unknown, `accept-new` re-trusting whatever answers —
//! stays open for as long as the user ignores a banner.
//!
//! # Which M-number lands where
//!
//! The receipt is keyed by [`LegacySignalKind`], one row per kind, because that
//! is what the banner looks up (`receipt.roots.find(kind === signal.kind)`).
//! Three plan roots have no kind of their own and so cannot be rows: M-03 (the
//! cache, deliberately abandoned and left to rebuild), M-13 (macOS privacy
//! grants, which no API can move) and M-14 (the frp proxy name, registered on
//! the user's own remote server). They are recorded in the journal instead, so
//! the ledger is complete even though the banner has nowhere to draw them.
//! M-04 (log file name) carries no historical data forward by design.

use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::brand;
use crate::commands::IpcResult;
use crate::conversation::durable_fs::{DirectoryPermissions, DurableFileSystem};
use crate::conversation::migration::lock::HostMigrationLock;
use crate::conversation::migration::{MigrationError, MigrationErrorCode, Result};
use crate::credentials;
use crate::legacy_appdata::{self, CarryForwardReport};
use crate::migration_detect::{self, LegacyRoots, LegacySignalKind, SshKnownHostsStatus};
use crate::ssh::known_hosts_migration;
use crate::webview_storage_handoff::{self, WebViewStorageStrategy};

/// Directory holding this migration's journal, beside the conversation
/// migration's own. Kept apart because the two are different migrations with
/// different identities; they share the lock, not the ledger.
pub const BRAND_MIGRATION_DIR: &str = "brand-migration";
pub const BRAND_MIGRATION_JOURNAL_FILE: &str = "journal-v1.json";
pub const BRAND_MIGRATION_JOURNAL_SCHEMA_VERSION: u32 = 1;

/// Per-root outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrandMigrationRootStatus {
    /// This pass carried something across.
    Migrated,
    /// There was nothing left to carry, or this root is read in place by design.
    Skipped,
    /// This root does not exist on this host, or no rename applies to it.
    NotApplicable,
    /// This root could not be carried. `reason` says why.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandMigrationRootReceipt {
    pub kind: LegacySignalKind,
    pub label: String,
    pub status: BrandMigrationRootStatus,
    pub reason: Option<String>,
}

impl BrandMigrationRootReceipt {
    fn new(
        kind: LegacySignalKind,
        status: BrandMigrationRootStatus,
        reason: Option<String>,
    ) -> Self {
        Self {
            kind,
            label: kind.label().to_string(),
            status,
            reason,
        }
    }

    fn migrated(kind: LegacySignalKind, reason: impl Into<String>) -> Self {
        Self::new(
            kind,
            BrandMigrationRootStatus::Migrated,
            Some(reason.into()),
        )
    }

    fn skipped(kind: LegacySignalKind, reason: impl Into<String>) -> Self {
        Self::new(kind, BrandMigrationRootStatus::Skipped, Some(reason.into()))
    }

    fn not_applicable(kind: LegacySignalKind, reason: impl Into<String>) -> Self {
        Self::new(
            kind,
            BrandMigrationRootStatus::NotApplicable,
            Some(reason.into()),
        )
    }

    fn failed(kind: LegacySignalKind, reason: impl Into<String>) -> Self {
        Self::new(kind, BrandMigrationRootStatus::Failed, Some(reason.into()))
    }
}

/// Payload of `run_brand_migration`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandMigrationReceipt {
    pub roots: Vec<BrandMigrationRootReceipt>,
}

impl BrandMigrationReceipt {
    /// The row for `kind`, for callers that want to assert on one root.
    #[must_use]
    pub fn root(&self, kind: LegacySignalKind) -> Option<&BrandMigrationRootReceipt> {
        self.roots.iter().find(|root| root.kind == kind)
    }
}

/// A plan root that has no [`LegacySignalKind`] and so cannot be a receipt row.
/// Recorded in the journal so the ledger is complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandMigrationNoticeV1 {
    /// The plan's identifier for the root, e.g. `M-03`.
    pub id: String,
    pub status: BrandMigrationRootStatus,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandMigrationRunV1 {
    pub run_id: Uuid,
    pub started_at_utc: DateTime<Utc>,
    pub roots: Vec<BrandMigrationRootReceipt>,
    pub notices: Vec<BrandMigrationNoticeV1>,
}

/// Append-only ledger of every merge attempt.
///
/// Deliberately not [`crate::conversation::migration::journal::MigrationJournalV1`]:
/// that type validates `migration_id == "conversation-layout-v2"`,
/// `source_layout == "legacy_v1"` and `target_layout == "conversation_v2"`, and
/// keys its steps by a sha256 operation key. Reusing it would mean writing
/// three identity fields that are false, which is a worse kind of "reuse" than
/// a small honest record. The *durability* machinery and the cross-process lock
/// are reused verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandMigrationJournalV1 {
    pub schema_version: u32,
    pub migration_id: String,
    pub runs: Vec<BrandMigrationRunV1>,
}

impl BrandMigrationJournalV1 {
    fn empty() -> Self {
        Self {
            schema_version: BRAND_MIGRATION_JOURNAL_SCHEMA_VERSION,
            migration_id: "brand-rename".to_string(),
            runs: Vec::new(),
        }
    }
}

/// Run the merge under the existing cross-process host migration lock.
///
/// `Err` only for the two conditions that make the whole pass meaningless: the
/// lock is held by another process, or it cannot be taken at all. Every
/// *per-root* failure is a row in the returned receipt, never an `Err` — the
/// user needs to see which root failed, and losing six good rows to report one
/// bad one helps nobody.
///
/// Reads the brand seam on the calling thread (FORBID-07). Nothing here is
/// handed to `spawn_blocking`.
pub fn run_migration(roots: &LegacyRoots) -> Result<BrandMigrationReceipt> {
    fs::create_dir_all(&roots.app_data_dir).map_err(|error| {
        MigrationError::new(
            MigrationErrorCode::MigrationLockInvalid,
            "prepare_brand_migration",
            format!("application-data root is unavailable: {error}"),
        )
    })?;
    // The same lock the conversation migration takes. Sharing it is the point:
    // both passes walk the same host state root, and two of them at once would
    // interleave copies into a tree neither of them owns exclusively.
    let lock = HostMigrationLock::new(&roots.app_data_dir)?;
    let guard = lock.acquire()?;

    let receipt = BrandMigrationReceipt {
        roots: vec![
            // First, and the order is load-bearing rather than cosmetic. The
            // keychain has no enumerable key list, so the two rows below it
            // reconstruct their keys from `termul-data.json` and
            // `ssh-profiles.json` — files that live *inside* the
            // application-data root. On a machine where only the pre-rename
            // root exists, running the keychain row first would read an empty
            // current root, find no keys, and report "nothing to carry" over a
            // keychain full of the user's secrets.
            migrate_app_data_dir(roots),
            migrate_documents_workspace(roots),
            migrate_standalone_state_root(roots),
            migrate_keychain_services(roots),
            migrate_local_storage(roots),
            migrate_repo_workspace_dirs(roots),
            // READ, not run. See the module doc.
            ssh_known_hosts_row(),
        ],
    };

    record_run(&roots.app_data_dir, &receipt);
    drop(guard);
    Ok(receipt)
}

// ---------------------------------------------------------------------------
// The roots
// ---------------------------------------------------------------------------

/// M-01, M-02 — the pre-rename `app_data_dir` tree for *this* install channel.
///
/// prod carries prod and dev carries dev; [`legacy_appdata::matching_legacy_root`]
/// owns that pairing, so a dev experiment can never be merged into a release
/// install by this call.
fn migrate_app_data_dir(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::AppDataDir;
    let Some(source) = legacy_appdata::matching_legacy_root(&roots.app_data_dir) else {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "no pre-rename application-data root exists for this install channel",
        );
    };
    match legacy_appdata::carry_forward(&source, &roots.app_data_dir) {
        Ok(report) => carry_row(kind, &source, &report),
        Err(error) => BrandMigrationRootReceipt::failed(
            kind,
            format!("{} could not be carried forward: {error}", source.display()),
        ),
    }
}

/// M-06 — `~/Documents/<pre-rename display name>`.
///
/// Registered read-only and never copied. That directory is full of the user's
/// own project files; the merge declares it so old sessions stay readable and
/// leaves every byte of it exactly where the user put it.
fn migrate_documents_workspace(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::DocumentsWorkspace;
    match crate::conversation::bootstrap::legacy_workspace_base(&roots.workspace_base) {
        Some(base) => BrandMigrationRootReceipt::skipped(
            kind,
            format!(
                "{} is registered read-only and stays where it is — the merge never moves \
                 the user's own project files",
                base.display()
            ),
        ),
        None => BrandMigrationRootReceipt::not_applicable(
            kind,
            "no pre-rename session-workspace root exists",
        ),
    }
}

/// M-07 — the standalone server's state root.
///
/// Named from `state_dir`, not from the bundle identifier, so the application
/// data carry-forward never reaches it and it needs its own pass.
fn migrate_standalone_state_root(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::StandaloneStateRoot;
    let Some(parent) = roots.state_root_parent.as_deref() else {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "this host has no standalone state root",
        );
    };
    let Some((legacy, canonical)) = crate::web::config::legacy_state_root_pair(parent) else {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "the state-root name is unchanged, so there is nothing to carry across",
        );
    };
    if !legacy.is_dir() {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "no pre-rename standalone state root exists",
        );
    }
    match legacy_appdata::carry_forward(&legacy, &canonical) {
        Ok(report) => carry_row(kind, &legacy, &report),
        Err(error) => BrandMigrationRootReceipt::failed(
            kind,
            format!("{} could not be carried forward: {error}", legacy.display()),
        ),
    }
}

/// M-09, M-10 — the two pre-rename keychain services.
///
/// A keychain cannot be listed through `keyring`, so the key set is
/// reconstructed from the state written alongside the secrets: projects × their
/// secret environment variables, and SSH profile ids × the two credential
/// suffixes. Each key is copied from the pre-rename service into the current
/// one; the pre-rename entry is left in place (FORBID-05), which is also what
/// makes a second pass find every key already present and copy nothing.
///
/// No secret, key name or backend message reaches the receipt: the reason
/// carries counts only. The banner is on screen, and an environment-variable
/// name paired with a project id is more than it needs.
fn migrate_keychain_services(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::KeychainService;
    let keys = migration_detect::legacy_credential_keys(&roots.app_data_dir);
    if keys.is_empty() {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "no stored credentials were found to carry",
        );
    }

    let backend = credentials::backend();
    let mut copied = 0usize;
    let mut already_present = 0usize;
    let mut failed = 0usize;
    let mut applicable = 0usize;

    for (scope, key) in &keys {
        let (legacy_service, canonical_service) = scope.services();
        if legacy_service == canonical_service {
            continue;
        }
        applicable += 1;
        match carry_credential(&*backend, legacy_service, canonical_service, key) {
            Ok(CredentialCarry::Copied) => copied += 1,
            Ok(CredentialCarry::AlreadyPresent) => already_present += 1,
            Ok(CredentialCarry::NotStored) => {}
            Err(error) => {
                failed += 1;
                log::warn!("[brand-migration] a credential could not be carried: {error}");
            }
        }
    }

    if applicable == 0 {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "the keychain service names are unchanged, so there is nothing to carry across",
        );
    }
    if failed > 0 {
        return BrandMigrationRootReceipt::failed(
            kind,
            format!(
                "{failed} of {applicable} credentials could not be carried to the current \
                 keychain service; every one of them is still readable under the previous \
                 service"
            ),
        );
    }
    if copied > 0 {
        return BrandMigrationRootReceipt::migrated(
            kind,
            format!(
                "{copied} credential(s) carried to the current keychain service; the previous \
                 entries were left in place"
            ),
        );
    }
    BrandMigrationRootReceipt::skipped(
        kind,
        format!(
            "nothing left to carry: {already_present} credential(s) are already under the \
             current keychain service"
        ),
    )
}

enum CredentialCarry {
    Copied,
    AlreadyPresent,
    NotStored,
}

fn carry_credential(
    backend: &dyn credentials::CredentialBackend,
    legacy_service: &str,
    canonical_service: &str,
    key: &str,
) -> std::result::Result<CredentialCarry, credentials::CredentialError> {
    // The current service wins. It may hold a value the user changed after the
    // rename, and overwriting that would be the same data loss in the other
    // direction.
    if backend.get(canonical_service, key)?.is_some() {
        return Ok(CredentialCarry::AlreadyPresent);
    }
    let Some(value) = backend.get(legacy_service, key)? else {
        return Ok(CredentialCarry::NotStored);
    };
    backend.set(canonical_service, key, &value)?;
    Ok(CredentialCarry::Copied)
}

/// M-05 — WebView storage, which has three different answers.
///
/// The platform is a parameter rather than a `cfg` so all three arms are
/// reachable from a test on any host; the orchestrator passes
/// [`HostPlatform::host`].
fn migrate_local_storage(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::LocalStorage;
    match webview_storage_handoff::strategy_for(roots.platform) {
        // Linux: the store is inside `app_data_dir`, so the M-02 row already
        // moved it. A second copier here would only race the first.
        WebViewStorageStrategy::RidesAlongWithAppData => BrandMigrationRootReceipt::not_applicable(
            kind,
            "the WebView store is inside the application-data root and was carried with it",
        ),
        // Windows: WebView2 keeps `EBWebView` under %LOCALAPPDATA%, outside the
        // Roaming application-data root, so it needs this one extra copy.
        WebViewStorageStrategy::CopyLocalWebViewDir => {
            let Some(local) = roots.app_local_data_dir.as_deref() else {
                return BrandMigrationRootReceipt::not_applicable(
                    kind,
                    "the local application-data root is unavailable",
                );
            };
            match webview_storage_handoff::carry_forward_local_webview_data(local) {
                Ok(report) if report == CarryForwardReport::default() => {
                    BrandMigrationRootReceipt::not_applicable(
                        kind,
                        "no pre-rename WebView2 store exists",
                    )
                }
                Ok(report) => carry_row(
                    kind,
                    &local.join(webview_storage_handoff::WEBVIEW_DATA_DIR),
                    &report,
                ),
                Err(error) => BrandMigrationRootReceipt::failed(
                    kind,
                    format!("the WebView2 store could not be carried forward: {error}"),
                ),
            }
        }
        // macOS: WKWebView storage is partitioned by bundle identifier and is
        // not a directory that can be moved. The app's own keys travel in the
        // handoff file, which rides along with the application-data root; the
        // renderer replays them. There is no copy for this pass to make.
        WebViewStorageStrategy::ReplayAppOwnedKeys => {
            match webview_storage_handoff::pending(&roots.app_data_dir) {
                Some(handoff) => BrandMigrationRootReceipt::skipped(
                    kind,
                    format!(
                        "{} app-owned key(s) are waiting in the storage handoff; the app \
                         replays them itself — WKWebView storage cannot be copied",
                        handoff.entries.len()
                    ),
                ),
                None => BrandMigrationRootReceipt::not_applicable(
                    kind,
                    "WKWebView storage is partitioned by bundle identifier and cannot be \
                     copied; no app-owned keys are waiting to be replayed",
                ),
            }
        }
    }
}

/// M-08, M-12 — the per-repository workspace directory in the user's own repos.
///
/// Read in two places, written in one, and never relocated. The directory sits
/// inside a repository the user owns and may have committed; moving it would be
/// a change to their working tree that they did not ask for. The managed skill
/// M-12 claims is deactivated at manifest level on the next provisioning, with
/// the old file left byte-identical on disk.
fn migrate_repo_workspace_dirs(roots: &LegacyRoots) -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::RepoWorkspaceDir;
    let legacy_name = brand::LEGACY.workspace_dir;
    if legacy_name == brand::canonical().workspace_dir {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "the workspace directory name is unchanged, so there is nothing to read across",
        );
    }
    let found = roots
        .project_roots
        .iter()
        .filter(|root| root.join(legacy_name).is_dir())
        .count();
    if found == 0 {
        return BrandMigrationRootReceipt::not_applicable(
            kind,
            "no opened repository holds a pre-rename workspace directory",
        );
    }
    BrandMigrationRootReceipt::skipped(
        kind,
        format!(
            "{found} repository workspace director(y/ies) are read in place and never \
             relocated; a managed skill claimed under the previous marker is deactivated in \
             the manifest on next provisioning, and its file is left unchanged"
        ),
    )
}

/// M-15 — read from the startup pass, never re-run.
///
/// There is deliberately no call to `known_hosts_migration::migrate_*` anywhere
/// in this module. Handing a security root's migration to a user's click would
/// leave the fail-open window that root exists to close open for exactly as
/// long as the banner is ignored.
fn ssh_known_hosts_row() -> BrandMigrationRootReceipt {
    let kind = LegacySignalKind::SshKnownHosts;
    let status =
        migration_detect::ssh_known_hosts_status(known_hosts_migration::startup_outcome().as_ref());
    match status {
        SshKnownHostsStatus::Migrated => BrandMigrationRootReceipt::migrated(
            kind,
            "carried unconditionally during startup; the merge never re-runs it",
        ),
        SshKnownHostsStatus::Skipped => BrandMigrationRootReceipt::skipped(
            kind,
            "the startup pass found no pre-rename host-key store to carry",
        ),
        SshKnownHostsStatus::NotApplicable => BrandMigrationRootReceipt::not_applicable(
            kind,
            "the host-key store name is unchanged, so the startup pass had nothing to carry",
        ),
        SshKnownHostsStatus::Failed { reason } => BrandMigrationRootReceipt::failed(
            kind,
            format!(
                "the startup pass failed: {reason}. Unknown hosts are refused this session \
                 rather than trusted on first use."
            ),
        ),
    }
}

/// Turn a carry-forward report into a row.
fn carry_row(
    kind: LegacySignalKind,
    source: &Path,
    report: &CarryForwardReport,
) -> BrandMigrationRootReceipt {
    if report.is_noop() {
        return BrandMigrationRootReceipt::skipped(
            kind,
            format!(
                "nothing left to carry from {}: {} path(s) were already present",
                source.display(),
                report.already_present
            ),
        );
    }
    BrandMigrationRootReceipt::migrated(
        kind,
        format!(
            "{} file(s) carried from {}; {} already present, {} link(s) skipped. The source \
             was left intact.",
            report.copied,
            source.display(),
            report.already_present,
            report.skipped_links
        ),
    )
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

/// The roots that have no [`LegacySignalKind`] and so cannot be receipt rows,
/// with the decision each one records.
///
/// Static rather than probed: every one of them is a *decision* about a root,
/// and there is nothing on disk that could change the answer. Each text is the
/// constant its own module already declares — the three modules that made the
/// decision own the wording, so a decision cannot be restated here in terms
/// that drift from it.
fn notices() -> Vec<BrandMigrationNoticeV1> {
    vec![
        BrandMigrationNoticeV1 {
            id: "M-03".to_string(),
            status: BrandMigrationRootStatus::NotApplicable,
            detail: crate::agent_registry::CACHE_NOT_MIGRATED_NOTICE.to_string(),
        },
        BrandMigrationNoticeV1 {
            id: "M-13".to_string(),
            status: BrandMigrationRootStatus::NotApplicable,
            detail: crate::macos_permissions::tcc_grants_reset_notice()
                .unwrap_or("this platform has no per-application privacy grants to reset")
                .to_string(),
        },
        BrandMigrationNoticeV1 {
            id: "M-14".to_string(),
            status: BrandMigrationRootStatus::NotApplicable,
            detail: crate::remote::tunnel::frp::FRP_PROXY_RENAME_NOTICE.to_string(),
        },
    ]
}

/// Append this pass to the journal. Best effort: a ledger that cannot be
/// written is not a reason to discard a merge that already succeeded, so the
/// failure is logged and the receipt still goes back to the user.
fn record_run(app_data_dir: &Path, receipt: &BrandMigrationReceipt) {
    if let Err(error) = write_journal(app_data_dir, receipt) {
        log::error!("[brand-migration] could not record the merge in the journal: {error}");
    }
}

fn write_journal(
    app_data_dir: &Path,
    receipt: &BrandMigrationReceipt,
) -> std::result::Result<(), String> {
    let directory = app_data_dir.join(BRAND_MIGRATION_DIR);
    let durable = DurableFileSystem::new();
    durable
        .create_dir_durable(&directory, DirectoryPermissions::PrivateOwnerOnly)
        .map_err(|error| error.to_string())?;
    let path = directory.join(BRAND_MIGRATION_JOURNAL_FILE);
    let mut journal = read_journal(&path).unwrap_or_else(BrandMigrationJournalV1::empty);
    journal.runs.push(BrandMigrationRunV1 {
        run_id: Uuid::new_v4(),
        started_at_utc: Utc::now(),
        roots: receipt.roots.clone(),
        notices: notices(),
    });
    let bytes = serde_json::to_vec_pretty(&journal).map_err(|error| error.to_string())?;
    durable
        .replace_bytes(&path, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The journal on disk, or `None` when there is nothing usable there. A journal
/// this build cannot parse is left alone rather than half-understood; the new
/// run still gets recorded, into a fresh ledger.
#[must_use]
pub fn read_journal(path: &Path) -> Option<BrandMigrationJournalV1> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice::<BrandMigrationJournalV1>(&bytes) {
        Ok(journal) if journal.schema_version == BRAND_MIGRATION_JOURNAL_SCHEMA_VERSION => {
            Some(journal)
        }
        Ok(journal) => {
            log::warn!(
                "[brand-migration] journal {} is schema {} but this build writes {}; starting a \
                 fresh ledger",
                path.display(),
                journal.schema_version,
                BRAND_MIGRATION_JOURNAL_SCHEMA_VERSION
            );
            None
        }
        Err(error) => {
            log::warn!(
                "[brand-migration] journal {} does not parse: {error}",
                path.display()
            );
            None
        }
    }
}

/// Where the journal lives under a given application-data root.
#[must_use]
pub fn journal_path(app_data_dir: &Path) -> std::path::PathBuf {
    app_data_dir
        .join(BRAND_MIGRATION_DIR)
        .join(BRAND_MIGRATION_JOURNAL_FILE)
}

// ---------------------------------------------------------------------------
// IPC
// ---------------------------------------------------------------------------

/// Run the user-initiated merge.
///
/// Unlike detection, this one *does* report failure: the user pressed a button
/// and is owed an answer. A per-root failure still comes back inside a
/// successful envelope, as a row — the error envelope is reserved for the pass
/// never having run at all.
#[tauri::command]
pub fn run_brand_migration(app: tauri::AppHandle) -> IpcResult<BrandMigrationReceipt> {
    let roots = match LegacyRoots::resolve(&app) {
        Ok(roots) => roots,
        Err(error) => return IpcResult::error(error, "BRAND_MIGRATION_FAILED"),
    };
    match run_migration(&roots) {
        Ok(receipt) => IpcResult::success(receipt),
        Err(error) if error.code == MigrationErrorCode::MigrationInProgress => IpcResult::error(
            "another process is already migrating this installation".to_string(),
            "BRAND_MIGRATION_IN_PROGRESS",
        ),
        Err(error) => IpcResult::error(error.to_string(), "BRAND_MIGRATION_FAILED"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration_detect::ALL_KINDS;
    use crate::webview_storage_handoff::HostPlatform;

    /// The renderer's `BrandMigrationRootStatus` union is written against these
    /// four strings, and `BrandMigrationRootReceipt.kind` against the same seven
    /// kinds the detector emits.
    #[test]
    fn the_wire_shape_is_exactly_what_the_renderer_reads() {
        let receipt = BrandMigrationReceipt {
            roots: vec![
                BrandMigrationRootReceipt::new(
                    LegacySignalKind::AppDataDir,
                    BrandMigrationRootStatus::Migrated,
                    None,
                ),
                BrandMigrationRootReceipt::new(
                    LegacySignalKind::DocumentsWorkspace,
                    BrandMigrationRootStatus::Skipped,
                    None,
                ),
                BrandMigrationRootReceipt::new(
                    LegacySignalKind::StandaloneStateRoot,
                    BrandMigrationRootStatus::NotApplicable,
                    None,
                ),
                BrandMigrationRootReceipt::new(
                    LegacySignalKind::SshKnownHosts,
                    BrandMigrationRootStatus::Failed,
                    Some("disk full".to_string()),
                ),
            ],
        };
        let json = serde_json::to_value(&receipt).expect("serialize");
        let rows = json["roots"].as_array().expect("roots array");
        let statuses: Vec<&str> = rows
            .iter()
            .map(|row| row["status"].as_str().expect("status string"))
            .collect();
        assert_eq!(statuses, ["migrated", "skipped", "notApplicable", "failed"]);
        assert_eq!(rows[0]["kind"], serde_json::json!("appDataDir"));
        assert_eq!(rows[0]["label"], serde_json::json!("Application data"));
        assert_eq!(rows[0]["reason"], serde_json::Value::Null);
        assert_eq!(rows[3]["kind"], serde_json::json!("sshKnownHosts"));
        assert_eq!(rows[3]["reason"], serde_json::json!("disk full"));

        let decoded: BrandMigrationReceipt = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded, receipt);
    }

    /// The banner resolves a row with `roots.find(kind === signal.kind)`, so a
    /// duplicate kind would shadow another root's status and a missing one would
    /// leave a root reading "pending" forever.
    #[test]
    fn every_kind_has_exactly_one_row_and_the_journal_carries_the_rest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let app_data_dir = temp.path().canonicalize().expect("canonicalize");
        let roots = LegacyRoots {
            app_data_dir: app_data_dir.clone(),
            app_local_data_dir: None,
            workspace_base: app_data_dir.join("workspace"),
            state_root_parent: None,
            project_roots: Vec::new(),
            ssh_dir: None,
            platform: HostPlatform::MacOs,
        };

        let receipt = run_migration(&roots).expect("the merge runs");
        let mut kinds: Vec<LegacySignalKind> = receipt.roots.iter().map(|root| root.kind).collect();
        kinds.sort_unstable();
        let mut expected = ALL_KINDS.to_vec();
        expected.sort_unstable();
        assert_eq!(kinds, expected);

        let journal = read_journal(&journal_path(&app_data_dir)).expect("journal written");
        let run = journal.runs.last().expect("one run recorded");
        let ids: Vec<&str> = run
            .notices
            .iter()
            .map(|notice| notice.id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["M-03", "M-13", "M-14"],
            "the three roots with no receipt row of their own must still be in the ledger"
        );
        assert!(run
            .notices
            .iter()
            .all(|notice| notice.status == BrandMigrationRootStatus::NotApplicable));
    }
}
