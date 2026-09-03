//! End-to-end acceptance for the brand-merge orchestrator (T-MIG-RUN).
//!
//! # Why this reads and writes real trees
//!
//! The question the merge has to answer is not "does this function return the
//! right enum". It is "after the user clicks once, is every byte they had still
//! reachable, and is every byte they had before still where it was". Neither
//! half of that is expressible against a mock filesystem, so every test here
//! builds a complete pre-rename tree in a `TempDir`, points the *real*
//! orchestrator at it, and then reads the disk back.
//!
//! # The two properties every root is held to
//!
//! 1. **The new root has the data.** A merge that reports success and leaves
//!    the destination empty is the failure this whole feature exists to
//!    prevent.
//! 2. **The old root is byte-identical afterwards** (FORBID-05). The merge
//!    copies. It never deletes, renames or truncates a legacy root, so a user
//!    who reverts to the previous build still finds everything.
//!
//! Roots that are read *in place* by design — `~/Documents/<old name>` (M-06)
//! and the per-repository workspace directory (M-08/M-12) — are held to a third
//! property instead: the destination must NOT have been created. For those two
//! the plan's decision is "never relocate", so a copy appearing would be the
//! defect.
//!
//! # Serialization
//!
//! The recorded startup outcome of the SSH host-key migration (M-15) is a
//! process-wide record, not a thread-local seam, because it describes one
//! process-wide event. Cargo runs these tests on parallel threads inside one
//! process, so every test that reads or writes it takes [`EXCLUSIVE`] first.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use termul_manager_lib::brand::{self, BrandCanonical};
use termul_manager_lib::conversation::HostMigrationLock;
use termul_manager_lib::credentials::{self, CredentialBackend, CredentialError};
use termul_manager_lib::known_hosts_migration::{self, KnownHostsMigration, StartupOutcome};
use termul_manager_lib::migration_detect::{LegacyRoots, LegacySignalKind};
use termul_manager_lib::migration_run::{self, BrandMigrationReceipt, BrandMigrationRootStatus};
use termul_manager_lib::webview_storage_handoff::{self, HostPlatform};

/// Serializes every test that touches the process-wide startup-outcome record.
static EXCLUSIVE: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    // A poisoned lock means a sibling test panicked while holding it. The state
    // it guards is re-established by every test before use, so continuing is
    // correct and reporting a poison error instead would bury the real failure.
    EXCLUSIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const POST_RENAME_KNOWN_HOSTS: &str = "known_hosts_se-manager";
/// `tauri-plugin-store` file the SSH profile manager writes. Pinned on the Rust
/// side by `migration_detect`'s unit tests against `profile_manager::STORE_FILE`.
const SSH_PROFILE_STORE: &str = "ssh-profiles.json";

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

/// The state-root component on this platform, chosen the same way
/// `web::config::state_dir_names` chooses it.
fn state_dir_pair() -> (&'static str, &'static str) {
    #[cfg(windows)]
    {
        (renamed().state_dir_windows, brand::LEGACY.state_dir_windows)
    }
    #[cfg(not(windows))]
    {
        (renamed().state_dir, brand::LEGACY.state_dir)
    }
}

// ---------------------------------------------------------------------------
// A keychain that is a `(service, key)` map, which is what a real one is
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MapBackend {
    entries: Mutex<BTreeMap<(String, String), String>>,
    writes: Mutex<Vec<(String, String)>>,
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

    fn value(&self, service: &str, key: &str) -> Option<String> {
        self.entries
            .lock()
            .unwrap()
            .get(&(service.to_string(), key.to_string()))
            .cloned()
    }

    fn writes(&self) -> Vec<(String, String)> {
        self.writes.lock().unwrap().clone()
    }
}

impl CredentialBackend for MapBackend {
    fn get(&self, service: &str, key: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.value(service, key))
    }

    fn set(&self, service: &str, key: &str, value: &str) -> Result<(), CredentialError> {
        self.writes
            .lock()
            .unwrap()
            .push((service.to_string(), key.to_string()));
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

// ---------------------------------------------------------------------------
// The fake pre-rename tree
// ---------------------------------------------------------------------------

/// Every path the fixture plants, so assertions name a field rather than
/// re-deriving a path and drifting from what was written.
struct Tree {
    _temp: tempfile::TempDir,
    base: PathBuf,
    /// M-01/M-02 — the pre-rename `app_data_dir` for the prod channel.
    legacy_app_data: PathBuf,
    /// The post-rename `app_data_dir`; the merge's destination.
    app_data: PathBuf,
    /// M-06 — `~/Documents/<pre-rename display name>`.
    legacy_documents: PathBuf,
    /// M-07 — the pre-rename standalone state root, and its destination.
    legacy_state: PathBuf,
    state: PathBuf,
    /// M-08/M-12 — an opened repository holding a pre-rename workspace dir.
    repo: PathBuf,
    /// M-15 — a `~/.ssh` holding the pre-rename host-key store.
    ssh: PathBuf,
    /// The keychain keys the enumeration must reconstruct (M-09, M-10).
    project_secret_key: String,
    ssh_password_key: String,
    roots: LegacyRoots,
}

impl Tree {
    /// A complete pre-rename tree covering M-01, M-02, M-05, M-06, M-07, M-08,
    /// M-09, M-10, M-12 and M-15.
    fn plant() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().canonicalize().expect("canonicalize");

        // -- M-01 / M-02: the pre-rename application-data tree --------------
        let legacy_app_data = base.join("appdata").join(brand::LEGACY.bundle_id);
        let app_data = base.join("appdata").join(renamed().bundle_id);
        fs::create_dir_all(legacy_app_data.join("conversations/v2")).expect("conversations");
        fs::create_dir_all(legacy_app_data.join("remote-tunnel")).expect("remote-tunnel");
        fs::create_dir_all(&app_data).expect("canonical app data");
        fs::write(
            legacy_app_data.join("conversations/v2/one.json"),
            b"{\"conversationId\":\"one\"}",
        )
        .expect("conversation record");
        fs::write(
            legacy_app_data.join("remote-tunnel/secrets.json"),
            b"{\"frpToken\":\"t\"}",
        )
        .expect("tunnel secrets");

        // The two stores the keychain enumeration reconstructs its keys from
        // live INSIDE the application-data root, so they only become readable
        // after M-01/M-02 has carried them across in the same pass.
        let project_secret_key = "project:p1:env:TOKEN".to_string();
        fs::write(
            legacy_app_data.join(termul_manager_lib::migration_detect::PERSISTENCE_STORE_FILE),
            serde_json::to_vec(&serde_json::json!({
                "projects": {
                    "projects": [{
                        "id": "p1",
                        "name": "demo",
                        "path": base.join("repo").to_string_lossy(),
                        "envVars": [{"key": "TOKEN", "value": "[REDACTED]", "isSecret": true}]
                    }],
                    "activeProjectId": "p1",
                    "updatedAt": "2026-01-01T00:00:00.000Z"
                }
            }))
            .expect("serialize project store"),
        )
        .expect("write project store");
        let ssh_password_key = "prof-1-password".to_string();
        fs::write(
            legacy_app_data.join(SSH_PROFILE_STORE),
            serde_json::to_vec(&serde_json::json!({
                "profiles": [{"id": "prof-1", "name": "box"}]
            }))
            .expect("serialize ssh store"),
        )
        .expect("write ssh store");

        // -- M-05: the macOS handoff rides along with the app-data root -----
        fs::write(
            legacy_app_data.join("webview-storage-handoff.json"),
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "writtenUnder": brand::LEGACY.bundle_id,
                "consumedUnder": null,
                "entries": {format!("{}theme", brand::LEGACY.storage_key_prefix): "dark"}
            }))
            .expect("serialize handoff"),
        )
        .expect("write handoff");

        // -- M-06: the pre-rename Documents workspace root ------------------
        let legacy_documents = base.join("documents").join(brand::LEGACY.display_name);
        fs::create_dir_all(legacy_documents.join("project-a")).expect("legacy documents");
        fs::write(legacy_documents.join("project-a/notes.md"), b"# mine\n").expect("user file");
        fs::create_dir_all(base.join("documents").join(renamed().display_name))
            .expect("canonical documents");

        // -- M-07: the pre-rename standalone state root ---------------------
        let (canonical_state, legacy_state_name) = state_dir_pair();
        let legacy_state = base.join("state").join(legacy_state_name);
        let state = base.join("state").join(canonical_state);
        fs::create_dir_all(legacy_state.join("sessions")).expect("legacy state root");
        fs::write(legacy_state.join("store.json"), b"{\"seeded\":true}").expect("store.json");
        fs::write(legacy_state.join("sessions/a.json"), b"session-a").expect("session");

        // -- M-08 / M-12: an opened repository with a pre-rename workspace ---
        let repo = base.join("repo");
        fs::create_dir_all(repo.join(brand::LEGACY.workspace_dir)).expect("legacy workspace dir");
        fs::write(
            repo.join(brand::LEGACY.workspace_dir)
                .join("managed-skills.json"),
            b"{\"schemaVersion\":1,\"managedByTermul\":true}",
        )
        .expect("managed skills manifest");

        // -- M-15: the app-managed host-key store ---------------------------
        let ssh = base.join("ssh");
        fs::create_dir_all(&ssh).expect("ssh dir");
        fs::write(
            ssh.join(brand::LEGACY.ssh_known_hosts_file),
            b"@cert-authority *.example.com ssh-ed25519 AAAA\n",
        )
        .expect("host-key store");

        let roots = LegacyRoots {
            app_data_dir: app_data.clone(),
            app_local_data_dir: None,
            workspace_base: base.join("documents").join(renamed().display_name),
            state_root_parent: Some(base.join("state")),
            project_roots: vec![repo.clone()],
            ssh_dir: Some(ssh.clone()),
            platform: HostPlatform::MacOs,
        };

        Self {
            _temp: temp,
            base,
            legacy_app_data,
            app_data,
            legacy_documents,
            legacy_state,
            state,
            repo,
            ssh,
            project_secret_key,
            ssh_password_key,
            roots,
        }
    }

    /// The keychain the merge sees, pre-seeded under the two pre-rename
    /// services (M-09, M-10).
    fn keychain(&self) -> Arc<MapBackend> {
        MapBackend::seeded(&[
            (
                brand::LEGACY.keychain_service,
                &self.project_secret_key,
                "s3cret",
            ),
            (
                brand::LEGACY.keychain_ssh_service,
                &self.ssh_password_key,
                "ssh-pw",
            ),
        ])
    }
}

/// The merge's own bookkeeping, which lives inside the application-data root.
/// A second pass is *supposed* to write both: a ledger that stops recording
/// after the first run is useless, and the lock file carries the owner metadata
/// of whoever holds it. Excluded from the "zero writes" snapshot so that
/// assertion is about the user's data.
const BOOKKEEPING: [&str; 2] = ["brand-migration", "conversation-migrations"];

/// Every regular file under `root`, as `relative path -> sha256`.
///
/// A digest rather than the bytes: the assertion is "this tree did not change",
/// and a mismatch has to name the path that did rather than print two kilobytes
/// of `Vec<u8>` at the reader.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    collect(root, root, &mut files);
    files
}

fn collect(root: &Path, current: &Path, files: &mut BTreeMap<String, String>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.parent() == Some(root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| BOOKKEEPING.contains(&name))
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            collect(root, &path, files);
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let bytes = fs::read(&path).unwrap_or_default();
            files.insert(relative, digest(&bytes));
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn status(receipt: &BrandMigrationReceipt, kind: LegacySignalKind) -> BrandMigrationRootStatus {
    receipt
        .root(kind)
        .unwrap_or_else(|| panic!("the receipt has no row for {kind:?}"))
        .status
}

fn reason(receipt: &BrandMigrationReceipt, kind: LegacySignalKind) -> String {
    receipt
        .root(kind)
        .and_then(|row| row.reason.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// (1) The full tree
// ---------------------------------------------------------------------------

/// One pass over a complete pre-rename tree. Every root is checked twice: the
/// destination has the data, and the source is byte-identical afterwards.
#[test]
fn a_full_pre_rename_tree_is_carried_forward_without_touching_the_source() {
    let _exclusive = exclusive();
    let tree = Tree::plant();
    let before_app_data = snapshot(&tree.legacy_app_data);
    let before_documents = snapshot(&tree.legacy_documents);
    let before_state = snapshot(&tree.legacy_state);
    let before_repo = snapshot(&tree.repo);
    let before_ssh = snapshot(&tree.ssh);

    let _brand = brand::override_canonical(renamed());
    let keychain = tree.keychain();
    let _backend =
        credentials::override_backend(Arc::clone(&keychain) as Arc<dyn CredentialBackend>);
    known_hosts_migration::set_startup_outcome_for_test(Some(StartupOutcome::Completed(
        KnownHostsMigration::Copied { bytes: 46 },
    )));

    let receipt = migration_run::run_migration(&tree.roots).expect("the merge runs");

    // -- M-01 / M-02 ----------------------------------------------------
    assert_eq!(
        status(&receipt, LegacySignalKind::AppDataDir),
        BrandMigrationRootStatus::Migrated,
        "{}",
        reason(&receipt, LegacySignalKind::AppDataDir)
    );
    assert_eq!(
        fs::read(tree.app_data.join("conversations/v2/one.json")).expect("carried conversation"),
        b"{\"conversationId\":\"one\"}"
    );
    assert_eq!(
        fs::read(tree.app_data.join("remote-tunnel/secrets.json")).expect("carried secrets"),
        b"{\"frpToken\":\"t\"}"
    );

    // -- M-05: the handoff rides along with the application-data root ---
    let handoff = webview_storage_handoff::pending(&tree.app_data)
        .expect("the storage handoff was carried into the current root");
    assert_eq!(
        handoff
            .entries
            .get(&format!("{}theme", brand::LEGACY.storage_key_prefix))
            .map(String::as_str),
        Some("dark"),
        "the app-owned key the renderer has to replay did not survive the carry"
    );
    assert_eq!(
        status(&receipt, LegacySignalKind::LocalStorage),
        BrandMigrationRootStatus::Skipped,
        "WKWebView storage cannot be copied; the merge must report the handoff, \
         not claim a migration it did not perform"
    );

    // -- M-06: registered read-only, never relocated --------------------
    assert_eq!(
        status(&receipt, LegacySignalKind::DocumentsWorkspace),
        BrandMigrationRootStatus::Skipped
    );
    assert!(
        !tree
            .base
            .join("documents")
            .join(renamed().display_name)
            .join("project-a")
            .exists(),
        "the merge copied the user's own Documents tree; that root is read in \
         place and must never be relocated"
    );

    // -- M-07 -----------------------------------------------------------
    assert_eq!(
        status(&receipt, LegacySignalKind::StandaloneStateRoot),
        BrandMigrationRootStatus::Migrated,
        "{}",
        reason(&receipt, LegacySignalKind::StandaloneStateRoot)
    );
    assert_eq!(
        fs::read(tree.state.join("store.json")).expect("carried store.json"),
        b"{\"seeded\":true}"
    );
    assert_eq!(
        fs::read(tree.state.join("sessions/a.json")).expect("carried session"),
        b"session-a"
    );

    // -- M-08 / M-12: read in place, never relocated --------------------
    assert_eq!(
        status(&receipt, LegacySignalKind::RepoWorkspaceDir),
        BrandMigrationRootStatus::Skipped
    );
    assert!(
        !tree.repo.join(renamed().workspace_dir).exists(),
        "the merge created a workspace directory inside the user's repository; \
         M-08 reads two places and writes one, it never relocates"
    );

    // -- M-09 / M-10: both services carried, both originals kept --------
    assert_eq!(
        status(&receipt, LegacySignalKind::KeychainService),
        BrandMigrationRootStatus::Migrated,
        "{}",
        reason(&receipt, LegacySignalKind::KeychainService)
    );
    assert_eq!(
        keychain.value(renamed().keychain_service, &tree.project_secret_key),
        Some("s3cret".to_string()),
        "the project environment secret was not carried to the current service"
    );
    assert_eq!(
        keychain.value(renamed().keychain_ssh_service, &tree.ssh_password_key),
        Some("ssh-pw".to_string()),
        "the SSH password was not carried to the current service"
    );
    assert_eq!(
        keychain.value(brand::LEGACY.keychain_service, &tree.project_secret_key),
        Some("s3cret".to_string()),
        "the pre-rename keychain entry must be left in place (FORBID-05)"
    );
    assert_eq!(
        keychain.value(brand::LEGACY.keychain_ssh_service, &tree.ssh_password_key),
        Some("ssh-pw".to_string()),
        "the pre-rename keychain entry must be left in place (FORBID-05)"
    );
    assert!(
        !reason(&receipt, LegacySignalKind::KeychainService).contains("TOKEN"),
        "the receipt is rendered on screen; it must not name the environment \
         variable a project keeps a secret under"
    );

    // -- M-15: reported from the startup pass ---------------------------
    assert_eq!(
        status(&receipt, LegacySignalKind::SshKnownHosts),
        BrandMigrationRootStatus::Migrated
    );

    // -- FORBID-05: every source is byte-identical ----------------------
    assert_eq!(
        snapshot(&tree.legacy_app_data),
        before_app_data,
        "the pre-rename application-data tree changed"
    );
    assert_eq!(
        snapshot(&tree.legacy_documents),
        before_documents,
        "the pre-rename Documents workspace changed"
    );
    assert_eq!(
        snapshot(&tree.legacy_state),
        before_state,
        "the pre-rename standalone state root changed"
    );
    assert_eq!(
        snapshot(&tree.repo),
        before_repo,
        "the user's repository changed"
    );
    assert_eq!(snapshot(&tree.ssh), before_ssh, "the ~/.ssh tree changed");

    known_hosts_migration::set_startup_outcome_for_test(None);
}

// ---------------------------------------------------------------------------
// (2) Idempotence
// ---------------------------------------------------------------------------

/// The banner's button can be pressed as often as the user likes. The second
/// press must carry nothing and write nothing.
#[test]
fn a_second_pass_carries_nothing_and_writes_nothing() {
    let _exclusive = exclusive();
    let tree = Tree::plant();
    let _brand = brand::override_canonical(renamed());
    let keychain = tree.keychain();
    let _backend =
        credentials::override_backend(Arc::clone(&keychain) as Arc<dyn CredentialBackend>);
    known_hosts_migration::set_startup_outcome_for_test(Some(StartupOutcome::Completed(
        KnownHostsMigration::Copied { bytes: 46 },
    )));

    let first = migration_run::run_migration(&tree.roots).expect("first pass");
    assert_eq!(
        status(&first, LegacySignalKind::AppDataDir),
        BrandMigrationRootStatus::Migrated
    );
    let writes_after_first = keychain.writes().len();
    assert!(
        writes_after_first > 0,
        "the first pass carried no credential"
    );

    // Snapshot the destinations only. The journal and the lock file are
    // *supposed* to record the second attempt; a ledger that stops recording
    // after the first run is useless.
    let app_data_after_first = snapshot(&tree.app_data);
    let state_after_first = snapshot(&tree.state);

    let second = migration_run::run_migration(&tree.roots).expect("second pass");

    for kind in [
        LegacySignalKind::AppDataDir,
        LegacySignalKind::DocumentsWorkspace,
        LegacySignalKind::StandaloneStateRoot,
        LegacySignalKind::KeychainService,
        LegacySignalKind::LocalStorage,
        LegacySignalKind::RepoWorkspaceDir,
    ] {
        assert_eq!(
            status(&second, kind),
            BrandMigrationRootStatus::Skipped,
            "{kind:?} reported {:?} on the second pass — {}",
            status(&second, kind),
            reason(&second, kind)
        );
    }
    // M-15 is not this pass's work at all: it reports what startup did, so it
    // reads the same on every pass rather than decaying to "skipped".
    assert_eq!(
        status(&second, LegacySignalKind::SshKnownHosts),
        status(&first, LegacySignalKind::SshKnownHosts),
        "the SSH row is a report of the startup pass; a second merge must not \
         change it"
    );

    assert_eq!(
        snapshot(&tree.app_data),
        app_data_after_first,
        "the second pass wrote into the application-data root"
    );
    assert_eq!(
        snapshot(&tree.state),
        state_after_first,
        "the second pass wrote into the standalone state root"
    );
    assert_eq!(
        keychain.writes().len(),
        writes_after_first,
        "the second pass wrote to the keychain: {:?}",
        &keychain.writes()[writes_after_first..]
    );

    known_hosts_migration::set_startup_outcome_for_test(None);
}

// ---------------------------------------------------------------------------
// (3) Failure isolation
// ---------------------------------------------------------------------------

/// One unwritable root must not strand the others behind it, and the receipt
/// has to say which one failed rather than reporting a clean success.
#[cfg(unix)]
#[test]
fn one_unwritable_root_fails_alone_and_is_named_in_the_receipt() {
    use std::os::unix::fs::PermissionsExt;

    let _exclusive = exclusive();
    let tree = Tree::plant();
    let _brand = brand::override_canonical(renamed());
    let keychain = tree.keychain();
    let _backend =
        credentials::override_backend(Arc::clone(&keychain) as Arc<dyn CredentialBackend>);
    known_hosts_migration::set_startup_outcome_for_test(Some(StartupOutcome::Completed(
        KnownHostsMigration::Copied { bytes: 46 },
    )));

    // The standalone state root's parent is made read-only, so creating the
    // post-rename root inside it fails while every other root is untouched.
    let state_parent = tree.base.join("state");
    fs::set_permissions(&state_parent, fs::Permissions::from_mode(0o500))
        .expect("make the state parent read-only");

    let outcome = migration_run::run_migration(&tree.roots);

    // Restore before asserting, so a failing assertion cannot leave a
    // read-only directory behind and block tempdir cleanup.
    fs::set_permissions(&state_parent, fs::Permissions::from_mode(0o700))
        .expect("restore the state parent");
    known_hosts_migration::set_startup_outcome_for_test(None);

    let receipt = outcome.expect("a single failing root must not fail the whole pass");
    assert_eq!(
        status(&receipt, LegacySignalKind::StandaloneStateRoot),
        BrandMigrationRootStatus::Failed,
        "an unwritable state root was not reported as failed"
    );
    assert!(
        reason(&receipt, LegacySignalKind::StandaloneStateRoot)
            .contains(&tree.legacy_state.display().to_string()),
        "the failure must name the root it is about, got: {}",
        reason(&receipt, LegacySignalKind::StandaloneStateRoot)
    );

    assert_eq!(
        status(&receipt, LegacySignalKind::AppDataDir),
        BrandMigrationRootStatus::Migrated,
        "the application-data root was stranded behind an unrelated failure"
    );
    assert!(tree.app_data.join("conversations/v2/one.json").is_file());
    assert_eq!(
        status(&receipt, LegacySignalKind::KeychainService),
        BrandMigrationRootStatus::Migrated,
        "the keychain was stranded behind an unrelated failure"
    );
    assert_eq!(
        keychain.value(renamed().keychain_service, &tree.project_secret_key),
        Some("s3cret".to_string())
    );
}

// ---------------------------------------------------------------------------
// (4) Concurrency
// ---------------------------------------------------------------------------

/// Subprocess leg of [`only_one_process_merges_at_a_time`]. Mirrors the lock
/// harness at `src/conversation/migration/mod.rs:2232-2261`; it is a no-op when
/// the environment does not name a root, which is how it behaves during an
/// ordinary run of this file.
#[test]
fn subprocess_lock_owner() {
    let Some(root) = std::env::var_os("SE_TEST_MIGRATION_LOCK_ROOT") else {
        return;
    };
    let barrier =
        PathBuf::from(std::env::var_os("SE_TEST_MIGRATION_LOCK_BARRIER").expect("barrier path"));
    let lock = HostMigrationLock::new(&PathBuf::from(root)).expect("prepare lock");
    let _guard = lock.acquire().expect("acquire lock");
    fs::write(barrier, b"locked").expect("signal the parent");
    std::thread::sleep(std::time::Duration::from_secs(60));
}

/// Two processes, one merge. The second must refuse rather than interleave
/// copies into a tree the first is halfway through.
#[test]
fn only_one_process_merges_at_a_time() {
    let _exclusive = exclusive();
    let tree = Tree::plant();
    let barrier = tree.base.join("lock-acquired");

    let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "subprocess_lock_owner", "--nocapture"])
        .env("SE_TEST_MIGRATION_LOCK_ROOT", &tree.app_data)
        .env("SE_TEST_MIGRATION_LOCK_BARRIER", &barrier)
        .spawn()
        .expect("spawn the lock owner");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !barrier.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(barrier.exists(), "the subprocess never acquired the lock");

    let _brand = brand::override_canonical(renamed());
    let keychain = tree.keychain();
    let _backend =
        credentials::override_backend(Arc::clone(&keychain) as Arc<dyn CredentialBackend>);

    let outcome = migration_run::run_migration(&tree.roots);

    child.kill().expect("kill the lock owner");
    child.wait().expect("reap the lock owner");

    let error = outcome.expect_err("a second process must not merge while the lock is held");
    assert_eq!(
        error.code.as_str(),
        "MIGRATION_IN_PROGRESS",
        "got {error} instead of a lock-contention refusal"
    );
    assert!(
        !tree.app_data.join("conversations/v2/one.json").exists(),
        "the excluded process copied anyway; the lock is not gating the merge"
    );
    assert!(
        keychain
            .value(renamed().keychain_service, &tree.project_secret_key)
            .is_none(),
        "the excluded process carried a credential anyway"
    );

    // The lock is released with the owner, so the merge can run afterwards.
    let receipt = migration_run::run_migration(&tree.roots)
        .expect("the merge runs once the lock owner is gone");
    assert_eq!(
        status(&receipt, LegacySignalKind::AppDataDir),
        BrandMigrationRootStatus::Migrated
    );
}

// ---------------------------------------------------------------------------
// (5) M-15 is reported, never re-run
// ---------------------------------------------------------------------------

/// The receipt carries the SSH host-key row, its status comes from the startup
/// pass, and the merge writes nothing into `~/.ssh`.
///
/// The second assertion is the one that matters. Handing this root's migration
/// to a user's click would leave the fail-open window it exists to close — every
/// trusted host reading as unknown, `accept-new` re-trusting whatever answers —
/// open for as long as the banner is ignored.
#[test]
fn the_ssh_row_comes_from_startup_and_the_merge_never_re_runs_it() {
    let _exclusive = exclusive();
    let tree = Tree::plant();
    let _brand = brand::override_canonical(renamed());
    let _backend = credentials::override_backend(tree.keychain() as Arc<dyn CredentialBackend>);

    for (recorded, expected) in [
        (
            StartupOutcome::Completed(KnownHostsMigration::Copied { bytes: 46 }),
            BrandMigrationRootStatus::Migrated,
        ),
        (
            StartupOutcome::Completed(KnownHostsMigration::NotNeeded),
            BrandMigrationRootStatus::Skipped,
        ),
        (
            StartupOutcome::Completed(KnownHostsMigration::NotApplicable),
            BrandMigrationRootStatus::NotApplicable,
        ),
        (
            StartupOutcome::Failed {
                reason: "cannot write ~/.ssh".to_string(),
            },
            BrandMigrationRootStatus::Failed,
        ),
    ] {
        known_hosts_migration::set_startup_outcome_for_test(Some(recorded.clone()));
        let receipt = migration_run::run_migration(&tree.roots).expect("the merge runs");
        assert_eq!(
            status(&receipt, LegacySignalKind::SshKnownHosts),
            expected,
            "the merge did not surface the recorded startup outcome {recorded:?}"
        );
    }
    known_hosts_migration::set_startup_outcome_for_test(None);

    // The pre-rename store is right there, and the merge had `ssh_dir` in hand
    // the whole time. Nothing may have appeared under the current name.
    assert!(
        tree.ssh.join(brand::LEGACY.ssh_known_hosts_file).is_file(),
        "the fixture's pre-rename store went missing"
    );
    assert!(
        !tree.ssh.join(POST_RENAME_KNOWN_HOSTS).exists(),
        "the merge carried the host-key store forward. That copy belongs to \
         startup: a security root's migration must not wait for a user's click."
    );

    // And the failure text reaches the user rather than being swallowed.
    known_hosts_migration::set_startup_outcome_for_test(Some(StartupOutcome::Failed {
        reason: "cannot write ~/.ssh".to_string(),
    }));
    let receipt = migration_run::run_migration(&tree.roots).expect("the merge runs");
    assert!(
        reason(&receipt, LegacySignalKind::SshKnownHosts).contains("cannot write ~/.ssh"),
        "the startup failure reason was dropped: {}",
        reason(&receipt, LegacySignalKind::SshKnownHosts)
    );
    known_hosts_migration::set_startup_outcome_for_test(None);
}
