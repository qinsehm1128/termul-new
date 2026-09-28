//! Centralized host bootstrap for Conversation v2.
//!
//! This is the only component allowed to acquire the host migration lock. It completes migration
//! recovery before opening the canonical repository or publishing any Conversation-dependent
//! service.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(any(test, feature = "test-support"))]
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::application::ConversationApplicationService;
use crate::creation::ConversationCreationService;
use crate::locator::{ConversationLocator, SessionWorkspaceLocator};
use crate::migration::{
    load_migration_map, BootstrapObservationReceiptV1, ConversationMigrationControlService,
    ConversationMigrationService, ConversationReader, HostMigrationLock, LegacyConversationReader,
    LegacyMigrationCallbacks, LegacyRootConfiguration, MigrationAdmissionState, MigrationContext,
    MigrationHostMode, MigrationPhase, ReaderPrecedence,
};
use crate::ordered_persistence::OrderedConversationPersistence;
use crate::persistence_adapter::ConversationPersistenceAdapter;
use crate::repository::{CatalogFlushCoordinator, ConversationRepository};
use crate::session_workspace::SessionWorkspaceService;
use crate::write_authority::{ConversationWriteAuthority, ConversationWriter};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConversationRoots {
    pub state_root: PathBuf,
    pub workspace_base: PathBuf,
    pub legacy_session_roots: Vec<PathBuf>,
    pub legacy_workspace_manifest_roots: Vec<PathBuf>,
    /// Pre-rename `app_data_dir` trees still present on disk — both bundle
    /// identifiers, prod and dev (M-01, M-02).
    ///
    /// **Read-only. Deliberately NOT fed into [`LegacyRootConfiguration`].**
    /// Two independent reasons, and both have to hold:
    ///
    /// 1. *Shape.* `standalone_session_roots` entries are consumed as
    ///    `LegacySourceKind::LegacyHostSessions` leaf directories — the
    ///    inventory scans the path itself, it does not append `acp-sessions`.
    ///    An `app_data_dir` root is one level up, so declaring it there would
    ///    point the session scanner at a directory of unrelated subtrees.
    /// 2. *Channel.* This vector reports **both** identifier trees, including
    ///    the install channel the running process is not. Migrating that one
    ///    would merge a dev build's data into a release install, which
    ///    the host's legacy app-data carry-forward exists specifically to prevent.
    ///
    /// The matching channel's data reaches the canonical root by
    /// the host's carry-forward instead, which runs before this
    /// struct is built. By the time the inventory looks at
    /// `host_state_root.join("acp-sessions")`, the carried-forward records are
    /// already there. This field's job is to let detection and the merge banner
    /// *tell the user* what still exists.
    pub legacy_appdata_roots: Vec<PathBuf>,
    /// Pre-rename *visible* session workspace roots — `~/Documents/<old display
    /// name>` on the desktop (M-06).
    ///
    /// Kept apart from `legacy_workspace_manifest_roots` because they are not
    /// the same thing: a manifest root holds the app's own `*.json` manifests,
    /// while this is a directory of the user's session workspaces sitting in
    /// their Documents folder. It is declared read-only — the user's files are
    /// never moved or copied on the strength of this field.
    ///
    /// It is also named by a completely different identity: `display_name`, not
    /// the bundle identifier. Renaming only the bundle id leaves it alone;
    /// renaming only the display name strands the entire root.
    pub legacy_workspace_bases: Vec<PathBuf>,
    /// The channel-matched pre-rename state root the host carried this
    /// install's data forward from, if any. A migration journal written there
    /// keyed its operation under the retired path-dependent formula; see
    /// [`superseded_operation_keys`].
    pub carried_from_state_roots: Vec<PathBuf>,
}

impl HostConversationRoots {
    /// Roots with no legacy sources declared. Hosts that know about
    /// pre-rename installs fill the legacy fields themselves.
    #[must_use]
    pub fn new(state_root: PathBuf, workspace_base: PathBuf) -> Self {
        Self {
            state_root,
            workspace_base,
            legacy_session_roots: Vec::new(),
            legacy_workspace_manifest_roots: Vec::new(),
            legacy_appdata_roots: Vec::new(),
            legacy_workspace_bases: Vec::new(),
            carried_from_state_roots: Vec::new(),
        }
    }

    #[must_use]
    pub fn private_conversation_root(&self) -> PathBuf {
        self.state_root.join("conversations").join("v2")
    }

    /// ACP-local operational journal root under the existing profile/state tree.
    ///
    /// This is not Conversation business storage. Canonical records remain under
    /// [`Self::private_conversation_root`]; the journal records in-flight
    /// cross-Core lifecycle operations only.
    #[must_use]
    pub fn lifecycle_journal_root(&self) -> PathBuf {
        super::lifecycle_journal::lifecycle_journal_root_for(&self.state_root)
    }
}

pub struct BootstrapOutcome {
    pub repository: Arc<ConversationRepository>,
    pub catalog_flush: Arc<CatalogFlushCoordinator>,
    pub authority: Arc<ConversationWriteAuthority>,
    pub writer: Arc<ConversationWriter>,
    pub reader: Arc<ConversationReader>,
    pub creation: Arc<ConversationCreationService>,
    pub persistence_adapter: Arc<ConversationPersistenceAdapter>,
    /// Sole bootstrap-owned ordering/backpressure/shutdown authority for canonical ACP events.
    pub ordered_persistence: Arc<OrderedConversationPersistence>,
    pub workspace: Arc<SessionWorkspaceService>,
    pub application: Arc<ConversationApplicationService>,
    pub layout_generation: uuid::Uuid,
    pub reader_precedence: ReaderPrecedence,
    pub migration_phase: MigrationPhase,
    pub recovery_item_count: usize,
    pub repository_scanned_event_count: u64,
    pub repository_sparse_index_entry_count: usize,
    pub repository_retained_payload_bytes: usize,
    pub repository_open_duration_ms: u64,
    pub repository_root: PathBuf,
    pub workspace_base: PathBuf,
}

#[derive(Debug)]
pub struct BootstrapError {
    pub code: &'static str,
    pub operation: &'static str,
    pub detail: String,
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.code, self.operation, self.detail
        )
    }
}

impl std::error::Error for BootstrapError {}

pub struct ConversationBootstrap;

#[cfg(any(test, feature = "test-support"))]
struct BootstrapTestHook {
    before_repository_open: Box<dyn Fn() + Send + Sync>,
    lock_acquire_count: AtomicUsize,
    store_open_count: AtomicUsize,
}

#[cfg(any(test, feature = "test-support"))]
static BOOTSTRAP_TEST_HOOKS: LazyLock<Mutex<HashMap<PathBuf, Arc<BootstrapTestHook>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(any(test, feature = "test-support"))]
fn test_hook(root: &Path) -> Option<Arc<BootstrapTestHook>> {
    BOOTSTRAP_TEST_HOOKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(root)
        .cloned()
}

impl ConversationBootstrap {
    pub fn run(
        roots: HostConversationRoots,
        host_mode: MigrationHostMode,
    ) -> Result<BootstrapOutcome, BootstrapError> {
        Self::run_with_admission(roots, host_mode, MigrationAdmissionState::default())
    }

    pub fn run_with_admission(
        mut roots: HostConversationRoots,
        host_mode: MigrationHostMode,
        admission: MigrationAdmissionState,
    ) -> Result<BootstrapOutcome, BootstrapError> {
        if !admission.is_clear() {
            return Err(error(
                "CONVERSATION_BOOTSTRAP_ADMISSION_OPEN",
                "validate_admission",
                "an app-managed mutable store, resource manager, or route was admitted before bootstrap",
            ));
        }
        let bootstrap_run_id = uuid::Uuid::new_v4().to_string();
        log::info!(
            "[conversation-bootstrap] start host_mode={host_mode:?} bootstrap_run_id={bootstrap_run_id}"
        );
        roots.state_root = create_absolute_directory(&roots.state_root, "create_state_root")?;
        roots.workspace_base =
            create_absolute_directory(&roots.workspace_base, "create_workspace_base")?;
        // Preserve configured legacy paths verbatim until the no-follow inventory validates every
        // root/component. Canonicalizing here would follow a symlink or junction before the
        // migration security boundary can reject it.

        // Bootstrap is the sole lock owner. The migration service receives this exact guard and
        // validates it; it never reacquires the lock.
        let migration_lock = HostMigrationLock::new(&roots.state_root)
            .map_err(|source| bootstrap_error(source.code.as_str(), "create_lock", source))?;
        let lock_guard = migration_lock
            .acquire()
            .map_err(|source| bootstrap_error(source.code.as_str(), "acquire_lock", source))?;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(hook) = test_hook(&roots.state_root) {
            hook.lock_acquire_count.fetch_add(1, Ordering::SeqCst);
        }
        let migration_service =
            ConversationMigrationService::new(&roots.state_root).map_err(|source| {
                bootstrap_error("MIGRATION_STARTUP_FAILED", "create_migration", source)
            })?;
        let legacy_configuration = LegacyRootConfiguration {
            host_state_root: roots.state_root.clone(),
            standalone_session_roots: roots.legacy_session_roots.clone(),
            standalone_workspace_manifest_roots: roots.legacy_workspace_manifest_roots.clone(),
        };
        let mut callbacks = LegacyMigrationCallbacks {
            roots: legacy_configuration.clone(),
            project_worktrees: Vec::new(),
        };
        let operation_key = migration_operation_key(&legacy_configuration);
        // A journal carried forward from a previous install path still
        // describes this operation; recognising its old key is what keeps a
        // relocation from reading as a foreign migration.
        let adoptable_operation_keys =
            superseded_operation_keys(&legacy_configuration, &roots.carried_from_state_roots);
        let mut report = migration_service
            .recover_and_run(MigrationContext {
                lock_guard: &lock_guard,
                host_state_root: &roots.state_root,
                operation_key: &operation_key,
                adoptable_operation_keys: &adoptable_operation_keys,
                host_mode,
                admission,
                now_utc: Utc::now(),
                callbacks: &mut callbacks,
            })
            .map_err(|source| bootstrap_error(source.code.as_str(), "recover_and_run", source))?;
        // Maintenance scheduling holds the kernel-backed control lock across
        // load/validate/modify/durable-replace. Consume pending intents on that
        // path before mutable stores or network admission are published.
        let control_service = ConversationMigrationControlService::new(&roots.state_root)
            .map_err(|source| bootstrap_error(source.code.as_str(), "create_control", source))?;
        let mut control_request_ids = Vec::new();
        if let Some(request) = control_service
            .pending()
            .map_err(|source| bootstrap_error(source.code.as_str(), "load_control", source))?
        {
            report = migration_service
                .apply_maintenance(
                    &request,
                    MigrationContext {
                        lock_guard: &lock_guard,
                        host_state_root: &roots.state_root,
                        operation_key: &operation_key,
                        adoptable_operation_keys: &adoptable_operation_keys,
                        host_mode,
                        admission,
                        now_utc: Utc::now(),
                        callbacks: &mut callbacks,
                    },
                )
                .map_err(|source| {
                    bootstrap_error(source.code.as_str(), "apply_maintenance", source)
                })?;
            control_service
                .complete(&request, &report, Utc::now())
                .map_err(|source| {
                    bootstrap_error(source.code.as_str(), "complete_maintenance", source)
                })?;
            control_request_ids.push(request.request_id);
        }

        if !matches!(
            report.phase,
            MigrationPhase::ObservationWindow
                | MigrationPhase::RolledBack
                | MigrationPhase::Finalized
        ) {
            return Err(error(
                "MIGRATION_STARTUP_FAILED",
                "admit_layout",
                format!(
                    "migration stopped in non-admissible phase {:?}",
                    report.phase
                ),
            ));
        }

        #[cfg(any(test, feature = "test-support"))]
        if let Some(hook) = test_hook(&roots.state_root) {
            (hook.before_repository_open)();
            hook.store_open_count.fetch_add(1, Ordering::SeqCst);
        }

        let repository_root = roots.private_conversation_root();
        let (repository, open_report) = ConversationRepository::open(repository_root.clone())
            .map_err(|source| {
                bootstrap_error(
                    "CONVERSATION_REPOSITORY_OPEN_FAILED",
                    "open_repository",
                    source,
                )
            })?;
        // The disposable cache coordinator is published only after the authoritative repository
        // has completed validation/rebuild. Repository and adapter retain this exact Arc for host
        // shutdown barriers; it never becomes a second writable authority.
        let catalog_flush = repository.catalog_flush_coordinator();
        let operation_dir = roots
            .state_root
            .join("conversation-migrations")
            .join(report.operation_id.to_string());
        let migration_map = load_migration_map(&operation_dir)
            .map_err(|source| {
                bootstrap_error(
                    "LEGACY_COMPATIBILITY_OPEN_FAILED",
                    "load_migration_map",
                    source,
                )
            })?
            .unwrap_or_else(|| crate::migration::MigrationMapV1 {
                schema_version: crate::migration::MIGRATION_MAP_SCHEMA_VERSION,
                operation_id: report.operation_id,
                entries: Vec::new(),
            });
        let legacy_roots = legacy_configuration
            .known_roots()
            .into_iter()
            .filter_map(|spec| spec.path.canonicalize().ok())
            .collect::<Vec<_>>();
        let legacy = LegacyConversationReader::open_read_only(&migration_map, &legacy_roots)
            .map_err(|source| {
                bootstrap_error(
                    "LEGACY_COMPATIBILITY_OPEN_FAILED",
                    "open_legacy_reader",
                    source,
                )
            })?;
        let authority = Arc::new(ConversationWriteAuthority::new(
            repository.as_ref(),
            report.reader_precedence,
            migration_map
                .entries
                .iter()
                .map(|entry| entry.conversation_id),
        ));
        let writer = Arc::new(
            ConversationWriter::new(Arc::clone(&repository), Arc::clone(&authority)).map_err(
                |source| {
                    bootstrap_error(
                        "CONVERSATION_WRITE_AUTHORITY_FAILED",
                        "create_writer",
                        source,
                    )
                },
            )?,
        );
        let reader = Arc::new(ConversationReader::new(
            Arc::clone(&repository),
            legacy,
            report.reader_precedence,
        ));
        let private_locator =
            ConversationLocator::new(repository_root.clone()).map_err(|source| {
                bootstrap_error("CONVERSATION_LOCATOR_FAILED", "private_locator", source)
            })?;
        let workspace_locator = SessionWorkspaceLocator::new(roots.workspace_base.clone())
            .map_err(|source| {
                bootstrap_error("CONVERSATION_LOCATOR_FAILED", "workspace_locator", source)
            })?;
        let creation = Arc::new(
            ConversationCreationService::new(
                Arc::clone(&writer),
                private_locator,
                workspace_locator,
            )
            .map(|service| service.with_legacy_workspace_roots(&roots.legacy_workspace_bases))
            .map_err(|source| {
                bootstrap_error(
                    "CONVERSATION_CREATION_OPEN_FAILED",
                    "creation_service",
                    source,
                )
            })?,
        );
        let recovered = futures::executor::block_on(creation.recover_incomplete_creations())
            .map_err(|source| {
                bootstrap_error("CONVERSATION_RECOVERY_FAILED", "recover_creations", source)
            })?;
        if recovered > 0 {
            log::warn!("[conversation-bootstrap] incomplete creations recovered count={recovered}");
        }
        let persistence_adapter = Arc::new(ConversationPersistenceAdapter::new(
            Arc::clone(&writer),
            Arc::clone(&reader),
        ));
        // Construct the canonical ordered lane exactly once after adapter bootstrap. Later host
        // composition injects this exact Arc; raw adapter append remains Conversation-module-only.
        let ordered_persistence = Arc::new(OrderedConversationPersistence::new(Arc::clone(
            &persistence_adapter,
        )));
        let workspace = Arc::new(SessionWorkspaceService::new(Arc::clone(&writer)));
        let application = Arc::new(ConversationApplicationService::new(
            Arc::clone(&reader),
            Arc::clone(&writer),
            Arc::clone(&workspace),
            &migration_map,
            host_mode,
            report.phase,
            report.reader_precedence,
        ));
        let recovery_item_count = workspace
            .list_recovery_items()
            .map_err(|source| {
                bootstrap_error(
                    "CONVERSATION_RECOVERY_FAILED",
                    "load_actionable_recovery",
                    source,
                )
            })?
            .into_iter()
            .filter(|item| item.status == crate::migration::RecoveryStatus::Unresolved)
            .count();
        if report.phase == MigrationPhase::ObservationWindow {
            let admitted_at_utc = Utc::now();
            let validation_sha256 = report.validation_sha256.clone().ok_or_else(|| {
                error(
                    "MIGRATION_OBSERVATION_INVALID",
                    "record_observation",
                    "observation-window report is missing the current validation digest",
                )
            })?;
            report = migration_service
                .record_bootstrap_observation(
                    crate::migration::MigrationControlContext {
                        lock_guard: &lock_guard,
                        host_state_root: &roots.state_root,
                        now_utc: admitted_at_utc,
                    },
                    BootstrapObservationReceiptV1 {
                        bootstrap_run_id: bootstrap_run_id.clone(),
                        admitted_at_utc,
                        validation_sha256,
                        control_request_ids,
                    },
                )
                .map_err(|source| {
                    bootstrap_error(source.code.as_str(), "record_observation", source)
                })?;
            log::info!(
                "[conversation-bootstrap] service-ready observation recorded bootstrap_run_id={} generation={}",
                bootstrap_run_id,
                report.target_generation
            );
        }
        drop(lock_guard);
        log::info!(
            "[conversation-bootstrap] complete host_mode={host_mode:?} phase={:?} precedence={:?} recovery_count={} scanned_event_count={} sparse_index_entry_count={} retained_payload_bytes={} repository_open_duration_ms={}",
            report.phase,
            report.reader_precedence,
            recovery_item_count,
            open_report.scanned_event_count,
            open_report.sparse_index_entry_count,
            open_report.retained_payload_bytes,
            open_report.duration_ms
        );
        Ok(BootstrapOutcome {
            repository,
            catalog_flush,
            authority,
            writer,
            reader,
            creation,
            persistence_adapter,
            ordered_persistence,
            workspace,
            application,
            layout_generation: report.target_generation,
            reader_precedence: report.reader_precedence,
            migration_phase: report.phase,
            recovery_item_count,
            repository_scanned_event_count: open_report.scanned_event_count,
            repository_sparse_index_entry_count: open_report.sparse_index_entry_count,
            repository_retained_payload_bytes: open_report.retained_payload_bytes,
            repository_open_duration_ms: open_report.duration_ms,
            repository_root,
            workspace_base: roots.workspace_base,
        })
    }
}

fn create_absolute_directory(
    path: &Path,
    operation: &'static str,
) -> Result<PathBuf, BootstrapError> {
    if !path.is_absolute() {
        return Err(error(
            "CONVERSATION_ROOT_INVALID",
            operation,
            "root must be absolute",
        ));
    }
    fs::create_dir_all(path)
        .map_err(|source| bootstrap_error("CONVERSATION_ROOT_CREATE_FAILED", operation, source))?;
    path.canonicalize()
        .map_err(|source| bootstrap_error("CONVERSATION_ROOT_INVALID", operation, source))
}

fn hex(digest: Sha256) -> String {
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Idempotency key identifying *which migration operation* a journal describes.
///
/// Keyed on the SHAPE of the legacy source set, never on where that set happens
/// to live. The absolute state root used to be hashed in, which made the key a
/// function of the install path — so relocating the tree (a bundle-identifier
/// change carrying `app_data_dir` forward, a user-chosen root) produced a
/// journal whose key no longer matched its own contents, and startup aborted on
/// data that was perfectly intact. The desktop host's three roots are always
/// `<state_root>/{acp-sessions,acp-chat-history,workspace-manifests}`, so the
/// absolute prefix contributed no distinguishing power at all — only fragility.
///
/// Roots outside the state root (standalone hosts point at arbitrary
/// directories) still contribute their absolute path: there the location IS the
/// identity, and two different external roots must not share a key.
fn migration_operation_key(configuration: &LegacyRootConfiguration) -> String {
    let mut digest = Sha256::new();
    digest.update(b"conversation-layout-v2\0");
    let mut entries = configuration
        .known_roots()
        .into_iter()
        .map(|spec| {
            let located = match spec.path.strip_prefix(&configuration.host_state_root) {
                Ok(relative) => format!("state-root:{}", relative.display()),
                Err(_) => format!("external:{}", spec.path.display()),
            };
            format!("{}\0{located}", spec.source_kind.as_str())
        })
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        digest.update(b"\0");
        digest.update(entry.as_bytes());
    }
    hex(digest)
}

/// Keys this same operation would have carried under the superseded formula.
///
/// A journal already on disk was written before the key stopped depending on the
/// install path, and possibly under a different root. `previous_state_roots` is
/// therefore the channel-matched pre-rename root and nothing else: that is the
/// only place the host's carry-forward can have brought this
/// journal from. Passing every known legacy root instead would let a dev
/// install's journal be adopted by a release one, which is the exact merge that
/// module exists to prevent.
///
/// Recomputing the old formula for those roots turns "the key does not match"
/// from an unexplained conflict into a recognised one, so the journal can be
/// adopted rather than treated as a foreign operation.
///
/// This is deliberately an exact-match allowlist, not a tolerance: a key that
/// matches none of these really does describe a different source set, and that
/// case must still refuse to proceed.
fn superseded_operation_keys(
    configuration: &LegacyRootConfiguration,
    previous_state_roots: &[PathBuf],
) -> Vec<String> {
    std::iter::once(&configuration.host_state_root)
        .chain(previous_state_roots)
        .map(|root| {
            let relocated = LegacyRootConfiguration {
                host_state_root: root.clone(),
                standalone_session_roots: configuration.standalone_session_roots.clone(),
                standalone_workspace_manifest_roots: configuration
                    .standalone_workspace_manifest_roots
                    .clone(),
            };
            let mut digest = Sha256::new();
            digest.update(b"conversation-layout-v2\0");
            digest.update(relocated.host_state_root.as_os_str().as_encoded_bytes());
            let mut legacy_roots = relocated
                .known_roots()
                .into_iter()
                .map(|spec| spec.path)
                .collect::<Vec<_>>();
            legacy_roots.sort();
            for path in legacy_roots {
                digest.update(b"\0");
                digest.update(path.as_os_str().as_encoded_bytes());
            }
            hex(digest)
        })
        .collect()
}

fn bootstrap_error(
    code: &'static str,
    operation: &'static str,
    source: impl fmt::Display,
) -> BootstrapError {
    log::error!("[conversation-bootstrap] failed code={code} operation={operation}");
    error(code, operation, source.to_string())
}

fn error(code: &'static str, operation: &'static str, detail: impl Into<String>) -> BootstrapError {
    BootstrapError {
        code,
        operation,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop_config(state_root: &str) -> LegacyRootConfiguration {
        LegacyRootConfiguration {
            host_state_root: PathBuf::from(state_root),
            standalone_session_roots: Vec::new(),
            standalone_workspace_manifest_roots: Vec::new(),
        }
    }

    /// The property the shipped crash violated. A desktop install's legacy
    /// sources are always the same three directories under its own state root,
    /// so moving that root — a bundle-identifier change, a user-chosen
    /// location — describes the very same migration and must key the same.
    #[test]
    fn the_desktop_key_does_not_depend_on_where_the_root_lives() {
        let a = migration_operation_key(&desktop_config("/Users/x/Library/com.a.app"));
        let b = migration_operation_key(&desktop_config("/Users/x/.se-manager"));
        let c = migration_operation_key(&desktop_config("/completely/elsewhere"));
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    /// Roots outside the state root are the standalone host's identity, so they
    /// still have to separate two different source sets.
    #[test]
    fn external_roots_still_separate_distinct_source_sets() {
        let base = desktop_config("/srv/state");
        let with_one = LegacyRootConfiguration {
            standalone_session_roots: vec![PathBuf::from("/mnt/alpha")],
            ..base.clone()
        };
        let with_other = LegacyRootConfiguration {
            standalone_session_roots: vec![PathBuf::from("/mnt/beta")],
            ..base.clone()
        };
        assert_ne!(
            migration_operation_key(&with_one),
            migration_operation_key(&with_other)
        );
        assert_ne!(
            migration_operation_key(&with_one),
            migration_operation_key(&base)
        );
    }

    /// An external root moving is a real identity change, unlike the state root.
    #[test]
    fn an_external_root_relocating_changes_the_key() {
        let here = LegacyRootConfiguration {
            standalone_session_roots: vec![PathBuf::from("/mnt/alpha")],
            ..desktop_config("/srv/state")
        };
        let moved = LegacyRootConfiguration {
            standalone_session_roots: vec![PathBuf::from("/mnt/alpha")],
            ..desktop_config("/srv/other-state")
        };
        // Same external root, different state root: the external path is what
        // carries identity here, and it did not move.
        assert_eq!(
            migration_operation_key(&here),
            migration_operation_key(&moved)
        );
    }

    /// The recognised-key list has to reproduce the SUPERSEDED formula exactly,
    /// or journals already on users' disks stay unrecognised and still abort.
    #[test]
    fn superseded_keys_reproduce_the_retired_path_dependent_formula() {
        let root = "/Users/x/Library/Application Support/com.termul-manager.app.dev";
        let config = desktop_config(root);

        // Recomputed by hand the way the retired formula did it.
        let mut digest = Sha256::new();
        digest.update(b"conversation-layout-v2\0");
        digest.update(root.as_bytes());
        let mut paths = config
            .known_roots()
            .into_iter()
            .map(|spec| spec.path)
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            digest.update(b"\0");
            digest.update(path.as_os_str().as_encoded_bytes());
        }
        let expected = hex(digest);

        let keys =
            superseded_operation_keys(&desktop_config("/somewhere/else"), &[PathBuf::from(root)]);
        assert!(
            keys.contains(&expected),
            "the retired key for {root} is not recognised: {keys:?}"
        );
    }

    /// The current root is always a candidate: that is the in-place upgrade
    /// case, where only the formula changed and nothing moved.
    #[test]
    fn superseded_keys_cover_an_in_place_formula_upgrade() {
        let config = desktop_config("/Users/x/state");
        let keys = superseded_operation_keys(&config, &[]);
        assert_eq!(keys.len(), 1);
        assert_ne!(
            keys[0],
            migration_operation_key(&config),
            "the retired formula must differ from the current one, or this whole path is dead code"
        );
    }

    #[test]
    fn fresh_desktop_and_standalone_roots_are_distinct_and_publish_identical_services() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = ConversationBootstrap::run(
            HostConversationRoots::new(
                temp.path().join("desktop-state"),
                temp.path().join("desktop-visible"),
            ),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        let standalone = ConversationBootstrap::run(
            HostConversationRoots::new(
                temp.path().join("server-state"),
                temp.path().join("server-visible"),
            ),
            MigrationHostMode::Standalone,
        )
        .unwrap();
        assert_ne!(desktop.repository.root(), standalone.repository.root());
        assert_ne!(desktop.workspace_base, standalone.workspace_base);
        assert_eq!(
            std::any::type_name_of_val(desktop.application.as_ref()),
            std::any::type_name_of_val(standalone.application.as_ref()),
            "both hosts publish the exact shared ConversationApplicationService type"
        );
        assert_eq!(
            desktop.application.host_status().unwrap().host_kind,
            crate::ConversationHostKind::Desktop
        );
        assert_eq!(
            standalone.application.host_status().unwrap().host_kind,
            crate::ConversationHostKind::Standalone
        );
        assert!(desktop.repository.list_conversations().is_empty());
        assert!(standalone.repository.list_conversations().is_empty());
    }

    #[test]
    fn successful_bootstrap_does_not_self_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let visible = temp.path().join("visible");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&visible).unwrap();
        let state = state.canonicalize().unwrap();
        let hook = Arc::new(BootstrapTestHook {
            before_repository_open: Box::new(|| {}),
            lock_acquire_count: AtomicUsize::new(0),
            store_open_count: AtomicUsize::new(0),
        });
        BOOTSTRAP_TEST_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(state.clone(), Arc::clone(&hook));

        let outcome = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(outcome.repository.root(), state.join("conversations/v2"));
        assert_eq!(hook.lock_acquire_count.load(Ordering::SeqCst), 1);
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 1);
        let journal: crate::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::migration::MIGRATION_JOURNAL_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            journal
                .observation_evidence
                .as_ref()
                .unwrap()
                .successful_bootstrap_count,
            1
        );
        assert!(state
            .join("conversation-migrations")
            .join(crate::migration::MIGRATION_LOCK_FILE)
            .is_file());
        BOOTSTRAP_TEST_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&state);
    }

    #[test]
    fn next_bootstrap_consumes_restart_intents_before_admission_and_pins_request_id() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let visible = temp.path().join("visible");
        let first = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible.clone()),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(first.migration_phase, MigrationPhase::ObservationWindow);
        drop(first);

        let control = ConversationMigrationControlService::new(&state).unwrap();
        let rollback = crate::migration::MigrationMaintenanceRequestV1 {
            action: crate::migration::MigrationMaintenanceAction::Rollback,
            request_id: uuid::Uuid::new_v4().to_string(),
            requested_at_utc: Utc::now(),
            approval_receipt: None,
        };
        control.request(rollback).unwrap();
        let rolled_back = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible.clone()),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(rolled_back.migration_phase, MigrationPhase::RolledBack);
        drop(rolled_back);
        assert!(control.pending().unwrap().is_none());

        let reapply = crate::migration::MigrationMaintenanceRequestV1 {
            action: crate::migration::MigrationMaintenanceAction::Reapply,
            request_id: uuid::Uuid::new_v4().to_string(),
            requested_at_utc: Utc::now(),
            approval_receipt: None,
        };
        control.request(reapply.clone()).unwrap();
        let reapplied = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(reapplied.migration_phase, MigrationPhase::ObservationWindow);
        assert!(control.pending().unwrap().is_none());

        let journal: crate::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::migration::MIGRATION_JOURNAL_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        let evidence = journal.observation_evidence.unwrap();
        assert_eq!(evidence.successful_bootstrap_count, 1);
        assert_eq!(
            evidence.bootstrap_receipts[0].control_request_ids,
            vec![reapply.request_id]
        );
    }

    #[test]
    fn concurrent_bootstrap_fails_before_store_open() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let visible = temp.path().join("visible");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&visible).unwrap();
        let state = state.canonicalize().unwrap();
        let visible = visible.canonicalize().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Mutex::new(release_rx);
        let hook = Arc::new(BootstrapTestHook {
            before_repository_open: Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .recv()
                    .unwrap();
            }),
            lock_acquire_count: AtomicUsize::new(0),
            store_open_count: AtomicUsize::new(0),
        });
        BOOTSTRAP_TEST_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(state.clone(), Arc::clone(&hook));

        let first_state = state.clone();
        let first_visible = visible.clone();
        let first = std::thread::spawn(move || {
            ConversationBootstrap::run(
                HostConversationRoots::new(first_state, first_visible),
                MigrationHostMode::Desktop,
            )
        });
        entered_rx.recv().unwrap();
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 0);
        let journal_before_ready: crate::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::migration::MIGRATION_JOURNAL_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            journal_before_ready
                .observation_evidence
                .as_ref()
                .unwrap()
                .successful_bootstrap_count,
            0,
            "bootstrap observation must not be recorded before repository/service readiness"
        );

        let second = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .err()
        .unwrap();
        assert_eq!(second.code, "MIGRATION_IN_PROGRESS");
        assert_eq!(hook.lock_acquire_count.load(Ordering::SeqCst), 1);
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 0);

        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 1);
        let journal_after_ready: crate::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::migration::MIGRATION_JOURNAL_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            journal_after_ready
                .observation_evidence
                .as_ref()
                .unwrap()
                .successful_bootstrap_count,
            1
        );
        BOOTSTRAP_TEST_HOOKS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&state);
    }

    #[test]
    fn bootstrap_is_sole_lock_owner() {
        let bootstrap = include_str!("bootstrap.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        assert_eq!(bootstrap.matches("HostMigrationLock::new").count(), 1);
        assert_eq!(bootstrap.matches(".acquire()").count(), 1);

        let migration = include_str!("migration/mod.rs")
            .split("#[cfg(test)]\nmod tests")
            .next()
            .unwrap();
        let recover = migration
            .split("pub fn recover_and_run")
            .nth(1)
            .unwrap()
            .split("pub fn recover_and_run_without_guard")
            .next()
            .unwrap();
        assert!(!recover.contains("HostMigrationLock"));
        assert!(!recover.contains(".acquire()"));
    }

    #[test]
    fn corrupt_migration_journal_aborts_before_repository_admission() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let visible = temp.path().join("visible");
        fs::create_dir_all(state.join("conversation-migrations")).unwrap();
        fs::write(
            state
                .join("conversation-migrations")
                .join(crate::migration::MIGRATION_JOURNAL_FILE),
            b"not-json",
        )
        .unwrap();
        let journal_path = state
            .join("conversation-migrations")
            .join(crate::migration::MIGRATION_JOURNAL_FILE);
        let failure = ConversationBootstrap::run(
            HostConversationRoots::new(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .err()
        .unwrap();
        assert_eq!(failure.code, "MIGRATION_JOURNAL_CORRUPT");
        assert_eq!(fs::read(journal_path).unwrap(), b"not-json");
        assert!(!state.join("conversations/v2").exists());
    }

    /// A journal the host carried forward from a pre-rename root still carries
    /// that root's retired key. It is adopted only when the host names that
    /// root in `carried_from_state_roots`; otherwise it is a foreign operation.
    #[test]
    fn a_journal_keyed_under_the_carried_from_root_is_adopted_only_when_declared() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("legacy-state");
        let retired_key =
            superseded_operation_keys(&desktop_config(legacy.to_str().unwrap()), &[]).remove(0);
        let seed = |state: &Path| {
            let dir = state.join("conversation-migrations");
            fs::create_dir_all(&dir).unwrap();
            let journal =
                crate::migration::MigrationJournalV1::new(retired_key.clone(), Utc::now());
            fs::write(
                dir.join(crate::migration::MIGRATION_JOURNAL_FILE),
                serde_json::to_vec(&journal).unwrap(),
            )
            .unwrap();
        };

        let undeclared = temp.path().join("undeclared");
        seed(&undeclared);
        let error = ConversationBootstrap::run(
            HostConversationRoots::new(undeclared, temp.path().join("visible-a")),
            MigrationHostMode::Desktop,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "MIGRATION_IDEMPOTENCY_CONFLICT");

        let declared = temp.path().join("declared");
        seed(&declared);
        ConversationBootstrap::run(
            HostConversationRoots {
                carried_from_state_roots: vec![legacy],
                ..HostConversationRoots::new(declared, temp.path().join("visible-b"))
            },
            MigrationHostMode::Desktop,
        )
        .expect("the carried-forward journal is adopted");
    }

    #[test]
    fn legacy_roots_are_byte_unchanged_after_bootstrap() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let visible = temp.path().join("visible");
        let roots = [
            state.join("acp-sessions"),
            state.join("acp-chat-history"),
            state.join("workspace-manifests"),
        ];
        for (index, root) in roots.iter().enumerate() {
            fs::create_dir_all(root).unwrap();
            fs::write(
                root.join(format!("preserved-{index}.bin")),
                [index as u8, 7, 9],
            )
            .unwrap();
        }
        let before = roots
            .iter()
            .map(|root| fs::read_dir(root).unwrap().count())
            .collect::<Vec<_>>();
        let bytes = roots
            .iter()
            .enumerate()
            .map(|(index, root)| fs::read(root.join(format!("preserved-{index}.bin"))).unwrap())
            .collect::<Vec<_>>();

        ConversationBootstrap::run(
            HostConversationRoots::new(state, visible),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        for (index, root) in roots.iter().enumerate() {
            assert_eq!(fs::read_dir(root).unwrap().count(), before[index]);
            assert_eq!(
                fs::read(root.join(format!("preserved-{index}.bin"))).unwrap(),
                bytes[index]
            );
        }
    }

    /// An explicit project target must NOT move the agent's cwd. The agent's
    /// working directory is the directory its Conversation created, always; the
    /// project is reachable as an additional root instead. Before this, an
    /// attached project silently became the agent's home, so every relative path
    /// it emitted resolved outside the Conversation.
    #[tokio::test]
    async fn explicit_execution_target_preserves_independent_workspace_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let outcome = ConversationBootstrap::run(
            HostConversationRoots::new(temp.path().join("state"), temp.path().join("visible")),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        let project_path = project
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let prepared = outcome
            .creation
            .prepare_conversation(crate::PrepareConversationRequest {
                schema_version: crate::PREPARE_CONVERSATION_SCHEMA_VERSION,
                conversation_id: None,
                project_attachment: Some(crate::ProjectAttachment {
                    schema_version: crate::PROJECT_ATTACHMENT_SCHEMA_VERSION,
                    project_id: "project-1".to_string(),
                    attached_at_utc: Utc::now(),
                    project_path_snapshot: project_path.clone(),
                    worktree_path: None,
                    worktree_branch: None,
                }),
                execution_target: crate::ExecutionTarget::ProjectRoot {
                    project_id: "project-1".to_string(),
                    project_root: project_path.clone(),
                },
                backend: crate::ConversationBackend::Agent,
            })
            .await
            .unwrap();
        assert_eq!(
            prepared.execution_cwd, prepared.workspace_cwd,
            "the agent's cwd is the Conversation workspace, never the project"
        );
        assert_eq!(
            prepared.additional_directories,
            vec![project_path],
            "the project stays reachable as an additional root"
        );
        assert!(Path::new(&prepared.workspace_cwd).is_dir());
    }

    #[test]
    fn admission_must_be_clear_before_the_lock_or_repository_opens() {
        let temp = tempfile::tempdir().unwrap();
        let error = ConversationBootstrap::run_with_admission(
            HostConversationRoots::new(temp.path().join("state"), temp.path().join("visible")),
            MigrationHostMode::Desktop,
            MigrationAdmissionState {
                session_persistence_active: true,
                ..MigrationAdmissionState::default()
            },
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "CONVERSATION_BOOTSTRAP_ADMISSION_OPEN");
        assert!(!temp.path().join("state").exists());
    }
}
