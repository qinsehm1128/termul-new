//! Centralized host bootstrap for Conversation v2.
//!
//! This is the only component allowed to acquire the host migration lock. It completes migration
//! recovery before opening the canonical repository or publishing any Conversation-dependent
//! service.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::conversation::application::ConversationApplicationService;
use crate::conversation::creation::ConversationCreationService;
use crate::conversation::locator::{ConversationLocator, SessionWorkspaceLocator};
use crate::conversation::migration::{
    load_migration_map, BootstrapObservationReceiptV1, ConversationMigrationControlService,
    ConversationMigrationService, ConversationReader, HostMigrationLock, LegacyConversationReader,
    LegacyMigrationCallbacks, LegacyRootConfiguration, MigrationAdmissionState, MigrationContext,
    MigrationHostMode, MigrationPhase, ReaderPrecedence,
};
use crate::conversation::ordered_persistence::OrderedConversationPersistence;
use crate::conversation::persistence_adapter::ConversationPersistenceAdapter;
use crate::conversation::repository::{CatalogFlushCoordinator, ConversationRepository};
use crate::conversation::session_workspace::SessionWorkspaceService;
use crate::conversation::write_authority::{ConversationWriteAuthority, ConversationWriter};

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
    ///    [`crate::legacy_appdata`] exists specifically to prevent.
    ///
    /// The matching channel's data reaches the canonical root by
    /// [`crate::legacy_appdata::carry_forward`] instead, which runs before this
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
}

impl HostConversationRoots {
    /// The desktop host's roots, including everything a pre-rename install left
    /// behind (M-01, M-02, M-06).
    ///
    /// `state_root` is Tauri's `app_data_dir()`, which is named from the bundle
    /// identifier: renaming the identifier moves it, and every byte under the
    /// old one becomes unreachable. So this constructor does two distinct
    /// things before returning:
    ///
    /// 1. **Carries the matching pre-rename tree forward** (copy-only, never
    ///    overwriting, never deleting — see [`crate::legacy_appdata`]). prod
    ///    carries prod and dev carries dev; the two are separate installs.
    /// 2. **Declares both pre-rename identifier trees, and the pre-rename
    ///    `~/Documents/<display name>` workspace root, as legacy-readable**, so
    ///    the migration inventory and the startup detector can see data under
    ///    either one. Declaring is read-only: the user's `~/Documents` tree is
    ///    never moved or copied by this call.
    ///
    /// Resolving the brand seam here is deliberate and load-bearing:
    /// `brand::canonical()` is thread-local, so it must be read on the thread
    /// that owns the override, never inside a spawned closure (FORBID-07). This
    /// runs on the caller's thread — the Tauri `setup` thread in production.
    #[must_use]
    pub fn desktop(state_root: PathBuf, workspace_base: PathBuf) -> Self {
        let legacy_appdata_roots = crate::legacy_appdata::legacy_appdata_roots(&state_root);
        if let Some(source) = crate::legacy_appdata::matching_legacy_root(&state_root) {
            match crate::legacy_appdata::carry_forward(&source, &state_root) {
                Ok(report) if report.is_noop() => {}
                Ok(report) => log::info!(
                    "[legacy-appdata] carried the pre-rename app data root forward from {} copied={} already_present={} skipped_links={}",
                    source.display(),
                    report.copied,
                    report.already_present,
                    report.skipped_links
                ),
                // Non-fatal by design: the legacy tree is still on disk and is
                // still declared below, so a failed copy costs "the merge has
                // more to do", never data. Refusing to launch would not make
                // the user's data any more reachable.
                Err(error) => log::error!(
                    "[legacy-appdata] could not carry {} forward into {}: {error}",
                    source.display(),
                    state_root.display()
                ),
            }
        }
        let legacy_workspace_bases = legacy_workspace_base(&workspace_base)
            .into_iter()
            .collect();
        Self {
            state_root,
            workspace_base,
            // Unchanged: the desktop host has never had standalone-shaped
            // legacy leaves, and the carried-forward tree is reached through
            // `host_state_root` like it always was. See
            // `legacy_appdata_roots`'s doc for why the pre-rename roots do not
            // belong here.
            legacy_session_roots: Vec::new(),
            legacy_workspace_manifest_roots: Vec::new(),
            legacy_appdata_roots,
            legacy_workspace_bases,
        }
    }

    #[must_use]
    pub fn standalone(
        state_root: PathBuf,
        workspace_base: PathBuf,
        legacy_session_root: Option<PathBuf>,
        legacy_workspace_manifest_root: Option<PathBuf>,
    ) -> Self {
        // `<project_root>/<display name>` is the standalone twin of
        // `~/Documents/<display name>` and is named by the same identity, so
        // the same pre-rename sibling can be sitting next to it (T-A16). Read
        // on the caller's thread like the desktop constructor (FORBID-07), and
        // read-only in exactly the same sense: the user's workspaces are never
        // moved on the strength of this field.
        let legacy_workspace_bases = legacy_workspace_base(&workspace_base)
            .into_iter()
            .collect();
        Self {
            state_root,
            workspace_base,
            legacy_session_roots: legacy_session_root.into_iter().collect(),
            legacy_workspace_manifest_roots: legacy_workspace_manifest_root.into_iter().collect(),
            // The standalone host names its state root from `state_dir`, not
            // from a bundle identifier. Its own legacy fallback is T-M07's,
            // applied by the caller before it gets here.
            legacy_appdata_roots: Vec::new(),
            legacy_workspace_bases,
        }
    }

    #[must_use]
    pub fn private_conversation_root(&self) -> PathBuf {
        self.state_root.join("conversations").join("v2")
    }
}

/// The pre-rename sibling of `workspace_base` — `~/Documents/<old display
/// name>` — when it exists on disk (M-06).
///
/// `workspace_base` is `<documents>/<display_name>`, so the legacy root is its
/// sibling under the same parent. `None` when the final component is not the
/// canonical display name (the user pointed `SE_CONVERSATION_WORKSPACE_ROOT`
/// somewhere of their own), when the rename has not landed yet and the two
/// names are equal, or when nothing is there.
///
/// This is a *detection* helper and nothing more. The directory it returns is
/// full of the user's own project files; the merge never moves or copies it,
/// and the caller stores the result in a field documented read-only.
///
/// Reads the brand seam, so it must be called on the thread that owns it
/// (FORBID-07).
///
/// `pub(crate)` so `migration_detect` reports the same root this constructor
/// declares. A second copy of the "is the final component the canonical display
/// name?" rule would let the banner list a directory the host never registered.
#[must_use]
pub(crate) fn legacy_workspace_base(workspace_base: &Path) -> Option<PathBuf> {
    let canonical = crate::brand::canonical();
    let legacy_name = crate::brand::LEGACY.display_name;
    if legacy_name == canonical.display_name {
        return None;
    }
    if workspace_base.file_name()?.to_str()? != canonical.display_name {
        return None;
    }
    let legacy_base = workspace_base.parent()?.join(legacy_name);
    legacy_base.is_dir().then_some(legacy_base)
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

#[cfg(test)]
struct BootstrapTestHook {
    before_repository_open: Box<dyn Fn() + Send + Sync>,
    lock_acquire_count: AtomicUsize,
    store_open_count: AtomicUsize,
}

#[cfg(test)]
static BOOTSTRAP_TEST_HOOKS: LazyLock<Mutex<HashMap<PathBuf, Arc<BootstrapTestHook>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(test)]
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
        #[cfg(test)]
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
        let mut report = migration_service
            .recover_and_run(MigrationContext {
                lock_guard: &lock_guard,
                host_state_root: &roots.state_root,
                operation_key: &operation_key,
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

        #[cfg(test)]
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
            .unwrap_or_else(|| crate::conversation::migration::MigrationMapV1 {
                schema_version: crate::conversation::migration::MIGRATION_MAP_SCHEMA_VERSION,
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
            .filter(|item| {
                item.status == crate::conversation::migration::RecoveryStatus::Unresolved
            })
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
                    crate::conversation::migration::MigrationControlContext {
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

fn migration_operation_key(configuration: &LegacyRootConfiguration) -> String {
    let mut digest = Sha256::new();
    digest.update(b"conversation-layout-v2\0");
    digest.update(configuration.host_state_root.as_os_str().as_encoded_bytes());
    let mut legacy_roots = configuration
        .known_roots()
        .into_iter()
        .map(|spec| spec.path)
        .collect::<Vec<_>>();
    legacy_roots.sort();
    for root in legacy_roots {
        digest.update(b"\0");
        digest.update(root.as_os_str().as_encoded_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
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
mod legacy_root_declaration_tests {
    use super::*;
    use crate::brand::{self, BrandCanonical};

    fn post_rename() -> BrandCanonical {
        BrandCanonical {
            bundle_id: "com.se-manager.app",
            bundle_id_dev: "com.se-manager.app.dev",
            display_name: "Se",
            ..brand::DEFAULT_CANONICAL
        }
    }

    /// M-06. `~/Documents/<old display name>` is named by `display_name`, an
    /// identity completely separate from the bundle identifier that names
    /// `app_data_dir`.
    #[test]
    fn legacy_workspace_base_finds_the_pre_rename_documents_root() {
        let temp = tempfile::tempdir().unwrap();
        let documents = temp.path();
        fs::create_dir_all(documents.join(brand::LEGACY.display_name)).unwrap();
        let _brand = brand::override_canonical(post_rename());

        assert_eq!(
            legacy_workspace_base(&documents.join(post_rename().display_name)),
            Some(documents.join(brand::LEGACY.display_name))
        );
    }

    #[test]
    fn legacy_workspace_base_ignores_a_user_supplied_root() {
        let temp = tempfile::tempdir().unwrap();
        let documents = temp.path();
        fs::create_dir_all(documents.join(brand::LEGACY.display_name)).unwrap();
        let _brand = brand::override_canonical(post_rename());

        // SE_CONVERSATION_WORKSPACE_ROOT pointed somewhere of the user's
        // own choosing: there is no rename relationship to infer.
        assert_eq!(
            legacy_workspace_base(&documents.join("my-own-projects")),
            None
        );
    }

    #[test]
    fn legacy_workspace_base_is_none_before_the_rename_lands() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join(brand::DEFAULT_CANONICAL.display_name)).unwrap();
        assert_eq!(
            legacy_workspace_base(&temp.path().join(brand::DEFAULT_CANONICAL.display_name)),
            None,
            "with canonical == legacy the sibling IS the canonical root"
        );
    }

    /// The pre-rename `app_data_dir` trees are reported for detection, but they
    /// must never reach `LegacyRootConfiguration`: `standalone_session_roots`
    /// entries are scanned as `acp-sessions` leaves, and the vector
    /// deliberately includes the install channel this process is not.
    #[test]
    fn pre_rename_appdata_roots_are_declared_for_detection_but_never_migrated() {
        let temp = tempfile::tempdir().unwrap();
        let support = temp.path().join("Application Support");
        for name in [brand::LEGACY.bundle_id, brand::LEGACY.bundle_id_dev] {
            fs::create_dir_all(support.join(name)).unwrap();
        }
        let state_root = support.join(post_rename().bundle_id);
        fs::create_dir_all(&state_root).unwrap();
        let _brand = brand::override_canonical(post_rename());

        let roots = HostConversationRoots::desktop(state_root.clone(), temp.path().join("Se"));

        assert_eq!(
            roots.legacy_appdata_roots,
            vec![
                support.join(brand::LEGACY.bundle_id),
                support.join(brand::LEGACY.bundle_id_dev),
            ],
            "both identifier trees must be visible to the detector"
        );
        assert!(
            roots.legacy_session_roots.is_empty(),
            "an app_data_dir root is not an acp-sessions leaf and the dev tree \
             must not be merged into a release install; got {:?}",
            roots.legacy_session_roots
        );

        let configuration = LegacyRootConfiguration {
            host_state_root: roots.state_root.clone(),
            standalone_session_roots: roots.legacy_session_roots.clone(),
            standalone_workspace_manifest_roots: roots.legacy_workspace_manifest_roots.clone(),
        };
        for spec in configuration.known_roots() {
            assert!(
                spec.path.starts_with(&state_root),
                "the inventory must only ever scan under the canonical root, got {}",
                spec.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_desktop_and_standalone_roots_are_distinct_and_publish_identical_services() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = ConversationBootstrap::run(
            HostConversationRoots::desktop(
                temp.path().join("desktop-state"),
                temp.path().join("desktop-visible"),
            ),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        let standalone = ConversationBootstrap::run(
            HostConversationRoots::standalone(
                temp.path().join("server-state"),
                temp.path().join("server-visible"),
                None,
                None,
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
            crate::conversation::ConversationHostKind::Desktop
        );
        assert_eq!(
            standalone.application.host_status().unwrap().host_kind,
            crate::conversation::ConversationHostKind::Standalone
        );
        assert!(desktop.repository.list_conversations().is_empty());
        assert!(standalone.repository.list_conversations().is_empty());
    }

    fn strip_rust_comments(source: &str) -> String {
        let mut output = String::with_capacity(source.len());
        let mut characters = source.chars().peekable();
        let mut in_block = false;
        while let Some(character) = characters.next() {
            if in_block {
                if character == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    in_block = false;
                }
                continue;
            }
            if character == '/' && characters.peek() == Some(&'*') {
                characters.next();
                in_block = true;
                continue;
            }
            if character == '/' && characters.peek() == Some(&'/') {
                characters.next();
                for line_character in characters.by_ref() {
                    if line_character == '\n' {
                        output.push('\n');
                        break;
                    }
                }
                continue;
            }
            output.push(character);
        }
        output
    }

    #[test]
    fn bootstrap_precedes_all_mutable_store_opens() {
        let desktop_source = strip_rust_comments(include_str!("../lib.rs"));
        let desktop_setup = desktop_source.find(".setup(|app|").unwrap();
        let desktop = &desktop_source[desktop_setup..];
        let desktop_bootstrap = desktop.find("ConversationBootstrap::run").unwrap();
        for forbidden in [
            "app.manage(",
            "PtyManager::new",
            "ChatHistoryStore::open_read_only",
            "WorkspaceManifestService::open_read_only",
            "AcpCatalogService::open",
            "AcpManager::with_conversation_services",
            "RemoteServerState::with_desktop_authority",
        ] {
            let position = desktop.find(forbidden).unwrap();
            assert!(
                desktop_bootstrap < position,
                "desktop bootstrap must precede {forbidden}"
            );
        }
        let before_desktop_bootstrap = &desktop[..desktop_bootstrap];
        for forbidden_root_access in [
            "SessionPersistence::open",
            "ChatHistoryStore::open",
            "WorkspaceManifestService::open",
            "acp-sessions",
            "acp-chat-history",
            "workspace-manifests",
            "conversations/v2",
        ] {
            assert!(
                !before_desktop_bootstrap.contains(forbidden_root_access),
                "desktop plugin/config setup must not access {forbidden_root_access} before bootstrap"
            );
        }

        let standalone = strip_rust_comments(include_str!("../server_main.rs"));
        let standalone_bootstrap = standalone.find("ConversationBootstrap::run").unwrap();
        for forbidden in [
            "WorkspaceManifestService::open_read_only",
            "AcpCatalogService::open",
            "FileProjectRegistry::load",
            "AcpManager::with_conversation_services",
            "PtyManager::new",
            "match serve(",
        ] {
            let position = standalone.find(forbidden).unwrap();
            assert!(
                standalone_bootstrap < position,
                "standalone bootstrap must precede {forbidden}"
            );
        }
        let before_standalone_bootstrap = &standalone[..standalone_bootstrap];
        for forbidden_store in [
            "SessionPersistence::open",
            "WorkspaceManifestService::open",
            "AcpCatalogService::open",
            "FileProjectRegistry::load",
            "AcpManager::",
            "PtyManager::new",
            "serve(",
        ] {
            assert!(
                !before_standalone_bootstrap.contains(forbidden_store),
                "standalone must not admit {forbidden_store} before bootstrap"
            );
        }
    }

    #[tokio::test]
    async fn projectless_conversation_exists_before_acp_new() {
        let temp = tempfile::tempdir().unwrap();
        let bootstrap = ConversationBootstrap::run(
            HostConversationRoots::desktop(temp.path().join("state"), temp.path().join("visible")),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        let manager = Arc::new(crate::AcpManager::with_conversation_services(
            Vec::new(),
            Arc::clone(&bootstrap.creation),
            Arc::clone(&bootstrap.persistence_adapter),
        ));
        let agent_id = crate::acp::AgentId("fake-agent".to_string());
        let (observed_tx, observed_rx) = std::sync::mpsc::sync_channel(1);
        manager.install_test_agent_for_new_session(agent_id.clone(), observed_tx);

        let created = manager
            .new_session_with_context(
                &agent_id,
                temp.path()
                    .join("ignored-cwd")
                    .to_string_lossy()
                    .into_owned(),
                Vec::new(),
                crate::acp::SessionCreationContext {
                    execution_target: Some(crate::conversation::ExecutionTarget::Workspace),
                    ..crate::acp::SessionCreationContext::default()
                },
            )
            .await
            .unwrap();
        let (execution_cwd_seen_by_acp, existed_before_acp) = observed_rx.recv().unwrap();
        assert!(existed_before_acp);
        assert_eq!(created.persistence, "conversation");
        assert_eq!(
            created.execution_cwd.as_deref(),
            Some(execution_cwd_seen_by_acp.as_str())
        );
        assert_eq!(created.workspace_cwd, created.execution_cwd);
        let conversation_id = created.conversation_id.unwrap();
        assert!(bootstrap
            .repository
            .get_conversation(conversation_id)
            .is_ok());
        assert_eq!(
            bootstrap
                .repository
                .current_binding(conversation_id)
                .unwrap()
                .unwrap()
                .agent_session_id,
            "opaque/fake-session"
        );
    }

    #[tokio::test]
    async fn binding_and_close_failures_return_safe_compound_receipt_and_persist_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let bootstrap = ConversationBootstrap::run(
            HostConversationRoots::desktop(state.clone(), temp.path().join("visible")),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        bootstrap.repository.fail_next_agent_binding_appends(2);
        let manager = Arc::new(crate::AcpManager::with_conversation_services(
            Vec::new(),
            Arc::clone(&bootstrap.creation),
            Arc::clone(&bootstrap.persistence_adapter),
        ));
        let agent_id = crate::acp::AgentId("fake-agent".to_string());
        let (observed_tx, _observed_rx) = std::sync::mpsc::sync_channel(1);
        manager.install_test_agent_for_new_session_with_close_result(
            agent_id.clone(),
            observed_tx,
            Err("provider close leaked SUPER_SECRET=do-not-return".to_string()),
        );

        let error = manager
            .new_session_with_context(
                &agent_id,
                temp.path()
                    .join("ignored-cwd")
                    .to_string_lossy()
                    .into_owned(),
                Vec::new(),
                crate::acp::SessionCreationContext {
                    execution_target: Some(crate::conversation::ExecutionTarget::Workspace),
                    ..crate::acp::SessionCreationContext::default()
                },
            )
            .await
            .unwrap_err();
        let failure = crate::conversation::AgentCompensationFailure::from_wire_error(&error)
            .expect("compound failure must use the stable wire receipt");
        assert_eq!(failure.primary_code, "CONVERSATION_BIND_FAILED");
        assert_eq!(
            failure.provider_close_code.as_deref(),
            Some("ACP_CLOSE_FAILED")
        );
        assert_eq!(
            failure.failure_record_code.as_deref(),
            Some("CONVERSATION_DURABILITY_FAILED")
        );
        assert!(failure.recovery_marker_code.is_none());
        assert!(failure.recovery_record_code.is_none());
        assert!(failure.recovery_id.is_some());
        assert!(!error.contains("SUPER_SECRET"));
        assert!(!error.contains("opaque/fake-session"));

        let record = bootstrap
            .repository
            .list_conversations()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(record.conversation_id, failure.conversation_id);
        assert_eq!(
            record.lifecycle_state,
            crate::conversation::ConversationLifecycleState::RecoveryRequired
        );
        assert!(bootstrap
            .repository
            .current_binding(record.conversation_id)
            .unwrap()
            .is_none());

        let recovery_bytes = fs::read(
            state
                .join("conversation-migrations")
                .join("workspace-recovery-v1")
                .join(crate::conversation::migration::RECOVERY_ITEMS_FILE),
        )
        .unwrap();
        let recovery: crate::conversation::migration::RecoveryQueueV1 =
            serde_json::from_slice(&recovery_bytes).unwrap();
        assert_eq!(recovery.items.len(), 1);
        let serialized = String::from_utf8(recovery_bytes).unwrap();
        assert!(!serialized.contains("SUPER_SECRET"));
        assert!(!serialized.contains("opaque/fake-session"));
        assert!(serialized.contains("acpCompensationFailed"));
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
            HostConversationRoots::desktop(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(outcome.repository.root(), state.join("conversations/v2"));
        assert_eq!(hook.lock_acquire_count.load(Ordering::SeqCst), 1);
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 1);
        let journal: crate::conversation::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE),
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
            .join(crate::conversation::migration::MIGRATION_LOCK_FILE)
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
            HostConversationRoots::desktop(state.clone(), visible.clone()),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(first.migration_phase, MigrationPhase::ObservationWindow);
        drop(first);

        let control = ConversationMigrationControlService::new(&state).unwrap();
        let rollback = crate::conversation::migration::MigrationMaintenanceRequestV1 {
            action: crate::conversation::migration::MigrationMaintenanceAction::Rollback,
            request_id: uuid::Uuid::new_v4().to_string(),
            requested_at_utc: Utc::now(),
            approval_receipt: None,
        };
        control.request(rollback).unwrap();
        let rolled_back = ConversationBootstrap::run(
            HostConversationRoots::desktop(state.clone(), visible.clone()),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(rolled_back.migration_phase, MigrationPhase::RolledBack);
        drop(rolled_back);
        assert!(control.pending().unwrap().is_none());

        let reapply = crate::conversation::migration::MigrationMaintenanceRequestV1 {
            action: crate::conversation::migration::MigrationMaintenanceAction::Reapply,
            request_id: uuid::Uuid::new_v4().to_string(),
            requested_at_utc: Utc::now(),
            approval_receipt: None,
        };
        control.request(reapply.clone()).unwrap();
        let reapplied = ConversationBootstrap::run(
            HostConversationRoots::desktop(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .unwrap();
        assert_eq!(reapplied.migration_phase, MigrationPhase::ObservationWindow);
        assert!(control.pending().unwrap().is_none());

        let journal: crate::conversation::migration::MigrationJournalV1 = serde_json::from_slice(
            &fs::read(
                state
                    .join("conversation-migrations")
                    .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE),
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
                HostConversationRoots::desktop(first_state, first_visible),
                MigrationHostMode::Desktop,
            )
        });
        entered_rx.recv().unwrap();
        assert_eq!(hook.store_open_count.load(Ordering::SeqCst), 0);
        let journal_before_ready: crate::conversation::migration::MigrationJournalV1 =
            serde_json::from_slice(
                &fs::read(
                    state
                        .join("conversation-migrations")
                        .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE),
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
            HostConversationRoots::desktop(state.clone(), visible),
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
        let journal_after_ready: crate::conversation::migration::MigrationJournalV1 =
            serde_json::from_slice(
                &fs::read(
                    state
                        .join("conversation-migrations")
                        .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE),
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
                .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE),
            b"not-json",
        )
        .unwrap();
        let journal_path = state
            .join("conversation-migrations")
            .join(crate::conversation::migration::MIGRATION_JOURNAL_FILE);
        let failure = ConversationBootstrap::run(
            HostConversationRoots::desktop(state.clone(), visible),
            MigrationHostMode::Desktop,
        )
        .err()
        .unwrap();
        assert_eq!(failure.code, "MIGRATION_JOURNAL_CORRUPT");
        assert_eq!(fs::read(journal_path).unwrap(), b"not-json");
        assert!(!state.join("conversations/v2").exists());
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
            HostConversationRoots::desktop(state, visible),
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
            HostConversationRoots::desktop(temp.path().join("state"), temp.path().join("visible")),
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
            .prepare_conversation(crate::conversation::PrepareConversationRequest {
                schema_version: crate::conversation::PREPARE_CONVERSATION_SCHEMA_VERSION,
                conversation_id: None,
                project_attachment: Some(crate::conversation::ProjectAttachment {
                    schema_version: crate::conversation::PROJECT_ATTACHMENT_SCHEMA_VERSION,
                    project_id: "project-1".to_string(),
                    attached_at_utc: Utc::now(),
                    project_path_snapshot: project_path.clone(),
                    worktree_path: None,
                    worktree_branch: None,
                }),
                execution_target: crate::conversation::ExecutionTarget::ProjectRoot {
                    project_id: "project-1".to_string(),
                    project_root: project_path.clone(),
                },
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
            HostConversationRoots::desktop(temp.path().join("state"), temp.path().join("visible")),
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
