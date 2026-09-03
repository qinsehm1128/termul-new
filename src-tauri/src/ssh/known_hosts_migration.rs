//! Startup migration of the app-managed SSH host-key store (M-15, finding F-02).
//!
//! `~/.ssh/<brand>.known_hosts` — see [`crate::brand::BrandCanonical::ssh_known_hosts_file`]
//! — is the app's own host-key store. It is deliberately *not* the user's
//! `~/.ssh/known_hosts`: libssh2's `write_file` rewrites that file from scratch
//! and re-serialises only the lines it could parse, dropping `@cert-authority` /
//! `@revoked` markers and unsupported key types, so the app never writes to the
//! shared file (`connection.rs:248-263`).
//!
//! # Why this migration is unconditional and separate from the merge entry point
//!
//! Renaming this file raises no error whatsoever. The store just reads empty,
//! every host the user already trusted becomes "unknown", and the surrounding
//! `accept-new` policy re-trusts whatever answers on that address — precisely
//! the state a man-in-the-middle needs, produced by a rename.
//!
//! The other legacy roots migrate when the user starts the merge. This one
//! cannot wait for that click: SSH will be used this session whether or not the
//! user ever opens the merge banner, and the window opens the moment the
//! canonical file name differs from the store on disk. So it runs at startup,
//! unconditionally.
//!
//! # Copy, byte for byte, and never delete (FORBID-05)
//!
//! The copy is `fs::copy` over the raw bytes. Parsing and re-emitting would drop
//! exactly the markers this root exists to preserve. The legacy file is left
//! untouched, and an existing destination is never overwritten.
//!
//! # Failure is loud, and it fails closed
//!
//! A best-effort copy that logs and continues re-creates the same MITM window.
//! On failure the error is recorded in a process-wide state that
//! [`SshConnection::verify_host_key`](super::connection) consults: while it is
//! set, an *unknown* host is refused instead of being accepted under TOFU.
//! Better to fail to connect than to silently re-trust while the key store may
//! be incomplete. Verification of hosts that *are* known is unaffected.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::brand;

/// What the startup migration did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnownHostsMigration {
    /// The canonical name is already the name on disk — nothing to carry.
    /// (True for every build until the rename lands.)
    NotApplicable,
    /// No legacy store exists: a fresh install, or one that never accepted a
    /// host key.
    NotNeeded,
    /// The canonical store already exists. Never overwritten: it is the live
    /// store and may already hold keys accepted since the rename.
    AlreadyMigrated,
    /// The legacy store was copied to the canonical name.
    Copied { bytes: u64 },
}

/// Why the migration could not be completed.
#[derive(Debug)]
pub enum KnownHostsMigrationError {
    /// Neither `HOME` nor `USERPROFILE` is set, so `~/.ssh` cannot be located.
    NoHomeDirectory,
    /// The legacy store could not be read.
    Read { path: PathBuf, source: io::Error },
    /// The canonical store could not be written.
    Write { path: PathBuf, source: io::Error },
}

impl fmt::Display for KnownHostsMigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHomeDirectory => {
                formatter.write_str("cannot locate ~/.ssh: neither HOME nor USERPROFILE is set")
            }
            Self::Read { path, source } => {
                write!(formatter, "cannot read {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(formatter, "cannot write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for KnownHostsMigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoHomeDirectory => None,
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
        }
    }
}

/// Set once the migration has failed, for the lifetime of the process.
///
/// Process-wide rather than thread-local, unlike the brand and credential
/// seams: this is not a test injection point but a real safety interlock, and
/// the SSH connection it has to stop may be attempted from any thread.
static MIGRATION_FAILED: AtomicBool = AtomicBool::new(false);

/// Whether the startup migration failed, i.e. whether the app-managed host-key
/// store may be missing entries the user already trusted.
pub fn migration_failed() -> bool {
    MIGRATION_FAILED.load(Ordering::SeqCst)
}

/// Test seam for the fail-closed path; production only ever sets this via a
/// real failure in [`migrate_app_known_hosts`].
#[doc(hidden)]
pub fn set_migration_failed_for_test(failed: bool) {
    MIGRATION_FAILED.store(failed, Ordering::SeqCst);
}

/// What the startup pass did, kept so the merge banner can *report* it.
///
/// The variants carry no path: this is read by a detector and rendered to a
/// user, and `~/.ssh` is not information a banner needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOutcome {
    /// The pass ran and reached one of the [`KnownHostsMigration`] states.
    Completed(KnownHostsMigration),
    /// The pass ran and failed. The interlock is set; unknown hosts are refused.
    Failed { reason: String },
}

/// The outcome [`run_at_startup`] recorded, or `None` when it has not run in
/// this process (every non-desktop composition — `se-server`, the browser
/// client — never calls it).
///
/// Process-wide rather than thread-local, for the same reason
/// [`MIGRATION_FAILED`] is: it describes one process-wide event, not a
/// per-thread test injection. Nothing here is a substitute for running the
/// migration — this is a *record*, and the only legitimate use of it is to
/// report. A caller that wants the copy performed calls
/// [`migrate_app_known_hosts`].
static STARTUP_OUTCOME: Mutex<Option<StartupOutcome>> = Mutex::new(None);

/// The recorded outcome of the startup pass, if it has run.
#[must_use]
pub fn startup_outcome() -> Option<StartupOutcome> {
    // A poisoned lock means some other thread panicked mid-record. The record
    // is still readable and reporting a stale outcome beats propagating a panic
    // into a detector that is required never to fail.
    match STARTUP_OUTCOME.lock() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn record_startup_outcome(outcome: StartupOutcome) {
    match STARTUP_OUTCOME.lock() {
        Ok(mut slot) => *slot = Some(outcome),
        Err(poisoned) => *poisoned.into_inner() = Some(outcome),
    }
}

/// Test seam for the reporting path. Production only ever records through
/// [`run_at_startup`].
#[doc(hidden)]
pub fn set_startup_outcome_for_test(outcome: Option<StartupOutcome>) {
    match STARTUP_OUTCOME.lock() {
        Ok(mut slot) => *slot = outcome,
        Err(poisoned) => *poisoned.into_inner() = outcome,
    }
}

/// The user's `~/.ssh` directory.
fn ssh_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".ssh"))
}

/// Carry the app-managed host-key store from the pre-rename file name to the
/// current one. Idempotent, copy-only, and never destructive.
///
/// Call this on the thread that owns the brand seam — [`brand::canonical`] is
/// thread-local, so resolving it inside a spawned closure would silently read
/// the shipped default (FORBID-07).
pub fn migrate_app_known_hosts() -> Result<KnownHostsMigration, KnownHostsMigrationError> {
    let Some(directory) = ssh_dir() else {
        MIGRATION_FAILED.store(true, Ordering::SeqCst);
        return Err(KnownHostsMigrationError::NoHomeDirectory);
    };
    migrate_app_known_hosts_in(&directory)
}

/// The migration itself, against an explicit `~/.ssh` directory so it is
/// testable without touching the developer's real one.
///
/// Records the failure interlock itself, so no caller can accidentally treat an
/// `Err` as "there was nothing to carry".
pub(crate) fn migrate_app_known_hosts_in(
    ssh_dir: &Path,
) -> Result<KnownHostsMigration, KnownHostsMigrationError> {
    let outcome = migrate_app_known_hosts_inner(ssh_dir);
    if outcome.is_err() {
        MIGRATION_FAILED.store(true, Ordering::SeqCst);
    }
    outcome
}

fn migrate_app_known_hosts_inner(
    ssh_dir: &Path,
) -> Result<KnownHostsMigration, KnownHostsMigrationError> {
    let legacy_name = brand::LEGACY.ssh_known_hosts_file;
    let canonical_name = brand::canonical().ssh_known_hosts_file;
    if legacy_name == canonical_name {
        return Ok(KnownHostsMigration::NotApplicable);
    }

    let legacy = ssh_dir.join(legacy_name);
    if !legacy.exists() {
        return Ok(KnownHostsMigration::NotNeeded);
    }

    let canonical = ssh_dir.join(canonical_name);
    if canonical.exists() {
        // The live store wins. Overwriting it would discard host keys accepted
        // since the rename, which is the same data loss in the other direction.
        return Ok(KnownHostsMigration::AlreadyMigrated);
    }

    // A byte-for-byte copy. Parsing and re-emitting would drop the
    // `@cert-authority` / `@revoked` markers that are the whole reason this
    // store is kept apart from the user's own known_hosts.
    let bytes = std::fs::copy(&legacy, &canonical).map_err(|source| {
        // `fs::copy` reports one error for both ends; attribute it to whichever
        // side actually cannot be reached.
        if std::fs::File::open(&legacy).is_err() {
            KnownHostsMigrationError::Read {
                path: legacy.clone(),
                source,
            }
        } else {
            KnownHostsMigrationError::Write {
                path: canonical.clone(),
                source,
            }
        }
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) =
            std::fs::set_permissions(&canonical, std::fs::Permissions::from_mode(0o600))
        {
            return Err(KnownHostsMigrationError::Write {
                path: canonical,
                source: error,
            });
        }
    }

    Ok(KnownHostsMigration::Copied { bytes })
}

/// Startup entry point: run the migration and record its outcome.
///
/// Never panics and never aborts startup — a failure sets the interlock that
/// makes `verify_host_key` refuse unknown hosts, which is the safe response,
/// and refusing to launch the app would not make the user any safer.
pub fn run_at_startup() {
    let outcome = migrate_app_known_hosts();
    match &outcome {
        Ok(KnownHostsMigration::NotApplicable) | Ok(KnownHostsMigration::NotNeeded) => {}
        Ok(KnownHostsMigration::AlreadyMigrated) => {
            log::info!("[SSH] App-managed known_hosts store was already migrated");
        }
        Ok(KnownHostsMigration::Copied { bytes }) => {
            log::info!(
                "[SSH] Migrated the app-managed known_hosts store ({} bytes copied; the old file was left in place)",
                bytes
            );
        }
        Err(error) => {
            log::error!(
                "[SSH] Failed to migrate the app-managed known_hosts store: {}. Unknown hosts will be refused this session rather than trusted on first use.",
                error
            );
        }
    }
    // Recorded so the merge banner can show what already happened. The banner
    // must never be the thing that *causes* it: this root's window opens the
    // moment the app starts, long before a user could click anything.
    record_startup_outcome(match outcome {
        Ok(migration) => StartupOutcome::Completed(migration),
        Err(error) => StartupOutcome::Failed {
            reason: error.to_string(),
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const POST_RENAME: &str = "known_hosts_se-manager";

    /// The frozen store, carrying the marker lines this root exists to keep.
    fn frozen_store() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/legacy-brand/ssh-known-hosts")
            .join(brand::LEGACY.ssh_known_hosts_file);
        fs::read(&path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()))
    }

    fn renamed() -> brand::BrandCanonical {
        brand::BrandCanonical {
            ssh_known_hosts_file: POST_RENAME,
            ..brand::DEFAULT_CANONICAL
        }
    }

    fn plant(directory: &Path) -> PathBuf {
        let legacy = directory.join(brand::LEGACY.ssh_known_hosts_file);
        fs::write(&legacy, frozen_store()).expect("plant the legacy store");
        legacy
    }

    #[test]
    fn copies_the_store_byte_for_byte_and_leaves_the_original_alone() {
        let temp = tempfile::tempdir().expect("tempdir");
        let legacy = plant(temp.path());
        let _brand = brand::override_canonical(renamed());

        let outcome = migrate_app_known_hosts_in(temp.path()).expect("migration runs");
        let expected = frozen_store();
        assert_eq!(
            outcome,
            KnownHostsMigration::Copied {
                bytes: expected.len() as u64
            }
        );

        let migrated = temp.path().join(POST_RENAME);
        assert_eq!(fs::read(&migrated).expect("read migrated"), expected);
        assert_eq!(
            fs::read(&legacy).expect("read legacy"),
            expected,
            "a migration copies; it never rewrites the source"
        );
    }

    #[test]
    fn the_marker_lines_survive_verbatim() {
        let temp = tempfile::tempdir().expect("tempdir");
        plant(temp.path());
        let _brand = brand::override_canonical(renamed());
        migrate_app_known_hosts_in(temp.path()).expect("migration runs");

        let migrated = String::from_utf8(fs::read(temp.path().join(POST_RENAME)).expect("read"))
            .expect("UTF-8");
        let markers: Vec<&str> = migrated
            .lines()
            .filter(|line| line.trim_start().starts_with('@'))
            .collect();
        assert!(
            markers.iter().any(|line| line.starts_with("@cert-authority")),
            "the @cert-authority line was lost: {markers:?}"
        );
        assert!(
            markers.iter().any(|line| line.starts_with("@revoked")),
            "the @revoked line was lost — a revoked key would be silently \
             re-trusted: {markers:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_copy_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let legacy = plant(temp.path());
        // Deliberately world-readable at the source: the destination must be
        // tightened regardless of what the old file carried.
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o644)).expect("relax source mode");
        let _brand = brand::override_canonical(renamed());
        migrate_app_known_hosts_in(temp.path()).expect("migration runs");

        let mode = fs::metadata(temp.path().join(POST_RENAME))
            .expect("stat migrated")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "got mode {:o}", mode & 0o777);
    }

    #[test]
    fn a_second_run_is_a_no_op() {
        let temp = tempfile::tempdir().expect("tempdir");
        plant(temp.path());
        let _brand = brand::override_canonical(renamed());
        migrate_app_known_hosts_in(temp.path()).expect("first run");

        let migrated = temp.path().join(POST_RENAME);
        let before = fs::metadata(&migrated).expect("stat migrated");
        let modified_before = before.modified().expect("mtime");

        let outcome = migrate_app_known_hosts_in(temp.path()).expect("second run");
        assert_eq!(outcome, KnownHostsMigration::AlreadyMigrated);
        assert_eq!(
            fs::metadata(&migrated).expect("stat again").modified().expect("mtime"),
            modified_before,
            "the second run must not write"
        );
    }

    #[test]
    fn an_existing_destination_is_never_overwritten() {
        let temp = tempfile::tempdir().expect("tempdir");
        plant(temp.path());
        let migrated = temp.path().join(POST_RENAME);
        let live = b"live.example.com ssh-ed25519 AAAAaccepted-after-the-rename\n";
        fs::write(&migrated, live).expect("plant a live canonical store");
        let _brand = brand::override_canonical(renamed());

        let outcome = migrate_app_known_hosts_in(temp.path()).expect("migration runs");
        assert_eq!(outcome, KnownHostsMigration::AlreadyMigrated);
        assert_eq!(
            fs::read(&migrated).expect("read"),
            live,
            "keys accepted since the rename must not be discarded"
        );
    }

    #[test]
    fn no_legacy_store_is_not_needed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _brand = brand::override_canonical(renamed());
        assert_eq!(
            migrate_app_known_hosts_in(temp.path()).expect("migration runs"),
            KnownHostsMigration::NotNeeded
        );
    }

    #[test]
    fn an_unchanged_brand_has_nothing_to_carry() {
        let temp = tempfile::tempdir().expect("tempdir");
        plant(temp.path());
        // No override: canonical == legacy, which is every build until Wave 5.
        assert_eq!(
            migrate_app_known_hosts_in(temp.path()).expect("migration runs"),
            KnownHostsMigration::NotApplicable
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_destination_is_an_error_not_an_ok() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let ssh_dir = temp.path().join(".ssh");
        fs::create_dir_all(&ssh_dir).expect("create .ssh");
        plant(&ssh_dir);
        fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o500))
            .expect("make the directory read-only");
        let _brand = brand::override_canonical(renamed());

        set_migration_failed_for_test(false);
        let error = migrate_app_known_hosts_in(&ssh_dir);
        let flag = migration_failed();
        // Restore both before asserting, so a failing assertion cannot leave the
        // interlock set for the rest of this process or block tempdir cleanup.
        set_migration_failed_for_test(false);
        fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700)).expect("restore mode");

        let error = error.expect_err("an unwritable destination must not return Ok");
        assert!(
            matches!(error, KnownHostsMigrationError::Write { .. }),
            "expected a write error, got {error:?}"
        );
        assert!(
            flag,
            "a failed migration must set the interlock verify_host_key reads; \
             without it the accept-new path silently re-trusts every host"
        );
        assert!(
            !ssh_dir.join(POST_RENAME).exists(),
            "a failed migration must not leave a partial store behind"
        );
    }
}
