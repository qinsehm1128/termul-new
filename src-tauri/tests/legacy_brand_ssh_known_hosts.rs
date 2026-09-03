//! T-H21 — `~/.ssh/known_hosts_termul`, the app-managed SSH host-key store.
//!
//! # Why this root exists at all, and why it must be migrated
//!
//! Finding F-02. This is the 15th data root: absent from all 14 roots in
//! `migration-plan.json` and from the analyze inventory. It is the only
//! brand-named file the app keeps *outside* its own app-data tree, which is
//! precisely why every inventory pass walked past it.
//!
//! Renaming it without migrating it does not fail loudly. The store simply
//! reads empty, every previously-trusted host becomes "unknown", and the
//! `accept-new` branch at `src/ssh/connection.rs:305-310` silently re-trusts
//! whatever answers on that address. That is not a cosmetic regression — it is
//! exactly the state a man-in-the-middle needs, produced by a rename.
//!
//! The file is kept separate from the user's own `~/.ssh/known_hosts` because
//! libssh2's `write_file` truncates and re-serializes only what it could parse,
//! dropping `@cert-authority` and `@revoked` markers and unsupported key types
//! (`src/ssh/connection.rs:248-263`). So a migration that "parses and rewrites"
//! destroys the very property this root was created to protect.
//!
//! # Why this reads from disk
//!
//! `assert_eq!(app_known_hosts_path(), ssh_dir().join("known_hosts_termul"))`
//! is a copy of the constant it checks: one repo-wide sed rewrites production
//! and assertion together and the suite stays green while a populated host-key
//! store is orphaned. So the subject here is
//! `tests/fixtures/legacy-brand/ssh-known-hosts/known_hosts_termul` — a frozen,
//! realistically shaped store carrying an ordinary `ssh-ed25519` entry, a
//! `@cert-authority` line and a `@revoked` line — and every expectation is read
//! from `brand::LEGACY` / `brand::canonical()` at runtime. The fixture is
//! sha256-guarded by `legacy_brand_fixture_manifest.rs` and never written back.
//!
//! # Why the path assertions are structural rather than a call
//!
//! `mod ssh` is private to `termul_manager_lib` (`src/lib.rs:31`, no `pub` and
//! no re-export), and `SshConnection::app_known_hosts_path` /
//! `verify_host_key` are private associated functions inside it. Integration
//! tests under `tests/` link this crate as an external dependency and see only
//! its `pub` items, so neither can be called from here. Those two contracts are
//! therefore asserted by parsing `connection.rs` with `syn` — including one
//! *positive* assertion, which matters: a sed over the brand string can delete
//! a literal but can never author a `brand::canonical()` call, so it cannot be
//! laundered green.
//!
//! The migration contract, by contrast, *is* executed: the legacy-root
//! inventory pipeline (`conversation::migration::inventory_legacy_roots`) is
//! public and is the production answer to "what will the migration carry".
//!
//! # Seams Wave 4/5 must add
//!
//! 1. `SshConnection::app_known_hosts_path` must join
//!    `crate::brand::canonical().ssh_known_hosts_file`, and the legacy literal
//!    must leave `connection.rs`.
//! 2. The migration must carry `~/.ssh/<LEGACY.ssh_known_hosts_file>` to
//!    `~/.ssh/<canonical.ssh_known_hosts_file>` **byte for byte** (copy-only,
//!    FORBID-05), and must not parse-and-rewrite.
//! 3. `verify_host_key` must be able to observe a failed/incomplete migration
//!    and refuse `accept-new` for unknown hosts while that state is set. A
//!    best-effort migration that fails open re-creates the MITM window this
//!    whole file exists to close.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use sha2::{Digest, Sha256};
use syn::visit::{self, Visit};
use syn::{Expr, ExprField, Ident, Lit, Member};
use termul_manager_lib::brand::{self, BrandCanonical};
use termul_manager_lib::conversation::migration::{inventory_legacy_roots, LegacyRootConfiguration};
use termul_manager_lib::known_hosts_migration::{
    self, migrate_app_known_hosts, KnownHostsMigration, KnownHostsMigrationError,
};

/// The production site under test.
const PRODUCTION_FILE: &str = "src/ssh/connection.rs";

/// The post-rename canonical value. Deliberately a different spelling from the
/// legacy one so no assertion below can be satisfied by accident.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        ssh_known_hosts_file: "known_hosts_se-manager",
        ..brand::DEFAULT_CANONICAL
    }
}

/// The migration probe reads `HOME` (that is how `~/.ssh` is found at all)
/// while `cargo test` runs test fns on parallel threads. The brand override is
/// thread-local and needs no lock; the environment does.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    // A `should_panic` test poisons the mutex on the way out by design.
    ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restores every env var it touched when dropped, including on unwind.
struct EnvScope {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvScope {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn set(mut self, key: &'static str, value: impl AsRef<Path>) -> Self {
        self.saved.push((key, std::env::var_os(key)));
        std::env::set_var(key, value.as_ref().as_os_str());
        self
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            match value {
                Some(previous) => std::env::set_var(key, previous),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The frozen store's directory. The *file name* inside it is never spelled
/// here — it is read off disk and compared against the brand seam.
fn fixture_dir() -> PathBuf {
    manifest_dir().join("tests/fixtures/legacy-brand/ssh-known-hosts")
}

/// The single file under the frozen root, located by listing rather than by
/// name, so `frozen_store_is_named_by_the_brand_seam` below is a real
/// comparison of two independent sources.
fn frozen_store_path() -> PathBuf {
    let mut entries: Vec<PathBuf> = fs::read_dir(fixture_dir())
        .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", fixture_dir().display()))
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    assert_eq!(
        entries.len(),
        1,
        "the frozen ssh-known-hosts root must hold exactly one store; got {entries:?}"
    );
    entries.remove(0)
}

fn frozen_store_bytes() -> Vec<u8> {
    let path = frozen_store_path();
    fs::read(&path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()))
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Lines carrying an OpenSSH marker (`@cert-authority`, `@revoked`, …).
///
/// These are the reason this root is not the user's `~/.ssh/known_hosts`, and
/// the thing any parse-and-rewrite implementation loses.
fn marker_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.trim_start().starts_with('@'))
        .collect()
}

/// Ordinary (unmarked, uncommented) host entries.
fn host_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with('@')
        })
        .collect()
}

/// Plants the frozen store into a temp `~/.ssh` under the **legacy** name, and
/// creates the canonical sibling's parent so a resolver has somewhere else it
/// could legitimately look. Returns `(tempdir, base, home, legacy_store_path)`.
///
/// `base` is the *canonicalized* temp root. The migration pipeline rejects any
/// durable path containing a symlink component, and on macOS `$TMPDIR` lives
/// under `/var`, which is a symlink to `/private/var`. Without this the
/// migration test would panic on `/var` rather than on its own assertion.
fn plant_ssh_dir() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let base = temp.path().canonicalize().expect("canonicalize the temp root");
    let home = base.join("home");
    let ssh_dir = home.join(".ssh");
    fs::create_dir_all(&ssh_dir).expect("create .ssh");

    let legacy = ssh_dir.join(brand::LEGACY.ssh_known_hosts_file);
    fs::copy(frozen_store_path(), &legacy).expect("plant the legacy store");

    // The user's own shared store, which the app must never rewrite. Present so
    // a wrong implementation that migrates *this* file instead is visible.
    fs::write(
        ssh_dir.join("known_hosts"),
        b"# the user's own file; system ssh owns it\n",
    )
    .expect("plant the user's own known_hosts");

    (temp, base, home, legacy)
}

// ---------------------------------------------------------------------------
// Reading the production source
// ---------------------------------------------------------------------------

/// Every identifier appearing anywhere in a subtree.
#[derive(Default)]
struct IdentScan {
    seen: std::collections::BTreeSet<String>,
}

impl<'ast> Visit<'ast> for IdentScan {
    fn visit_ident(&mut self, node: &'ast Ident) {
        self.seen.insert(node.to_string());
    }
}

fn subtree_mentions(expr: &Expr, ident: &str) -> bool {
    let mut scan = IdentScan::default();
    scan.visit_expr(expr);
    scan.seen.contains(ident)
}

/// Which brand fields the file reads, and which raw strings it still hardcodes.
#[derive(Default)]
struct FileFacts {
    brand_fields_read: std::collections::BTreeSet<String>,
    legacy_fields_read: std::collections::BTreeSet<String>,
    string_literals: Vec<String>,
}

impl<'ast> Visit<'ast> for FileFacts {
    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(name) = &node.member {
            // `<anything mentioning `brand`>.<field>` — catches
            // `brand::canonical().ssh_known_hosts_file`, the `crate::`-prefixed
            // form, and a hoisted `let brand = crate::brand::canonical();`.
            if subtree_mentions(&node.base, "brand") {
                self.brand_fields_read.insert(name.to_string());
                if subtree_mentions(&node.base, "LEGACY") {
                    self.legacy_fields_read.insert(name.to_string());
                }
            }
        }
        visit::visit_expr_field(self, node);
    }

    fn visit_lit(&mut self, node: &'ast Lit) {
        if let Lit::Str(value) = node {
            self.string_literals.push(value.value());
        }
        visit::visit_lit(self, node);
    }
}

fn production_facts() -> FileFacts {
    let path = manifest_dir().join(PRODUCTION_FILE);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("production file {} is unreadable: {e}", path.display()));
    let ast: syn::File = syn::parse_file(&source)
        .unwrap_or_else(|e| panic!("production file {} does not parse: {e}", path.display()));
    let mut facts = FileFacts::default();
    facts.visit_file(&ast);
    facts
}

// ---------------------------------------------------------------------------
// Guards over the frozen record — preconditions, not the reds
// ---------------------------------------------------------------------------

/// The store's *file name* on disk and `brand::LEGACY.ssh_known_hosts_file` are
/// two independent sources. Editing either alone turns this red, which is what
/// makes the reds below trustworthy: they cannot be satisfied by a fixture that
/// quietly moved.
#[test]
fn frozen_store_is_named_by_the_brand_seam() {
    let path = frozen_store_path();
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_else(|| panic!("{} has no UTF-8 file name", path.display()));

    assert_eq!(
        name,
        brand::LEGACY.ssh_known_hosts_file,
        "the frozen store must still be named by brand::LEGACY.ssh_known_hosts_file"
    );
    // The whole point of the root: it is *not* the user's own known_hosts.
    assert_ne!(
        brand::LEGACY.ssh_known_hosts_file, "known_hosts",
        "collapsing the app-managed store onto the user's shared known_hosts \
         hands libssh2's writer a file it will strip markers from"
    );
    assert_ne!(
        post_rename().ssh_known_hosts_file,
        brand::LEGACY.ssh_known_hosts_file,
        "the post-rename injection must be a different spelling or nothing below proves anything"
    );
}

/// The oracle is real: the frozen store carries all three line shapes the
/// migration has to survive. Without this, `markers_survive_a_byte_preserving_copy`
/// and the migration red below could both pass over an empty file.
#[test]
fn frozen_store_carries_the_line_shapes_this_root_exists_for() {
    let text = String::from_utf8(frozen_store_bytes()).expect("the frozen store is UTF-8");

    let hosts = host_lines(&text);
    assert!(
        hosts.iter().any(|line| line.contains("ssh-ed25519")),
        "the frozen store must carry an ordinary ssh-ed25519 entry; got {hosts:?}"
    );

    let markers = marker_lines(&text);
    assert!(
        markers.iter().any(|line| line.starts_with("@cert-authority")),
        "the frozen store must carry a @cert-authority line — libssh2's write_file \
         drops it, which is why this root is separate from ~/.ssh/known_hosts; got {markers:?}"
    );
    assert!(
        markers.iter().any(|line| line.starts_with("@revoked")),
        "the frozen store must carry a @revoked line — losing it silently re-trusts \
         a key the user explicitly revoked; got {markers:?}"
    );
}

/// The migration contract, stated as an executable predicate rather than a
/// comment: a byte-preserving copy keeps the marker lines verbatim, and any
/// implementation that parses and re-serializes will not satisfy it.
///
/// This is the assertion the ledger entry below re-uses, so proving it holds
/// for a plain `fs::copy` is what makes that red mean "the migration is
/// missing" rather than "the check is impossible".
#[test]
fn markers_survive_a_byte_preserving_copy() {
    let (_temp, _base, _home, legacy) = plant_ssh_dir();
    let copied = legacy.with_file_name(post_rename().ssh_known_hosts_file);
    fs::copy(&legacy, &copied).expect("copy the store");

    assert_byte_identical_store(&legacy, &copied);
}

/// `migrated` must be the same bytes as `source`, and `source` must be
/// untouched (FORBID-05: migration copies, it never moves or rewrites).
fn assert_byte_identical_store(source: &Path, migrated: &Path) {
    let source_bytes = fs::read(source)
        .unwrap_or_else(|e| panic!("read source {} failed: {e}", source.display()));
    let migrated_bytes = fs::read(migrated)
        .unwrap_or_else(|e| panic!("read migrated {} failed: {e}", migrated.display()));

    assert_eq!(
        sha256(&source_bytes),
        sha256(&frozen_store_bytes()),
        "the legacy store was modified in place; a migration copies, it never rewrites"
    );

    let source_text = String::from_utf8(source_bytes).expect("source is UTF-8");
    let migrated_text = String::from_utf8(migrated_bytes).expect("migrated is UTF-8");
    assert_eq!(
        marker_lines(&source_text),
        marker_lines(&migrated_text),
        "the @cert-authority / @revoked lines did not survive verbatim — a \
         parse-and-rewrite implementation loses exactly these, which is the \
         reason this root is kept apart from the user's own known_hosts"
    );
    assert_eq!(
        sha256(migrated_text.as_bytes()),
        sha256(source_text.as_bytes()),
        "the migrated store is not byte-identical to the legacy one"
    );
}

// ---------------------------------------------------------------------------
// M-15 — the startup migration, executed end to end
// ---------------------------------------------------------------------------

/// The store must arrive under the new name intact, and the old file must still
/// be sitting there afterwards (FORBID-05: copy, never move).
///
/// This calls `migrate_app_known_hosts` — the exact entry point `lib.rs` wires
/// into startup — rather than a test-only helper, so what is proved here is what
/// ships.
#[test]
fn the_startup_migration_carries_the_frozen_store_byte_for_byte() {
    let _lock = env_lock();
    let (_temp, _base, home, legacy) = plant_ssh_dir();
    let _env = EnvScope::new().set("HOME", &home).set("USERPROFILE", &home);
    let _brand = brand::override_canonical(post_rename());

    let outcome = migrate_app_known_hosts().expect("the startup migration runs");
    assert_eq!(
        outcome,
        KnownHostsMigration::Copied {
            bytes: frozen_store_bytes().len() as u64
        },
        "the whole frozen store must be carried"
    );

    let migrated = legacy.with_file_name(post_rename().ssh_known_hosts_file);
    // sha256 equality both ways, and the @cert-authority / @revoked lines
    // verbatim — a parse-and-rewrite implementation cannot satisfy this.
    assert_byte_identical_store(&legacy, &migrated);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&migrated)
            .expect("stat the migrated store")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the host-key store must stay owner-only; got {:o}",
            mode & 0o777
        );
    }

    assert_eq!(
        fs::read(home.join(".ssh/known_hosts")).expect("the user's own store is still there"),
        b"# the user's own file; system ssh owns it\n",
        "the migration must not touch the user's shared known_hosts"
    );
}

/// Idempotent. Startup runs this every launch; the second and every subsequent
/// run must be a no-op, not a re-copy that could clobber keys accepted since.
#[test]
fn a_second_startup_finds_the_store_already_migrated_and_writes_nothing() {
    let _lock = env_lock();
    let (_temp, _base, home, legacy) = plant_ssh_dir();
    let _env = EnvScope::new().set("HOME", &home).set("USERPROFILE", &home);
    let _brand = brand::override_canonical(post_rename());

    migrate_app_known_hosts().expect("first startup");
    let migrated = legacy.with_file_name(post_rename().ssh_known_hosts_file);
    let modified_before = fs::metadata(&migrated)
        .expect("stat the migrated store")
        .modified()
        .expect("mtime");

    assert_eq!(
        migrate_app_known_hosts().expect("second startup"),
        KnownHostsMigration::AlreadyMigrated
    );
    assert_eq!(
        fs::metadata(&migrated)
            .expect("stat again")
            .modified()
            .expect("mtime"),
        modified_before,
        "the second run must not write"
    );
}

/// An existing destination is never overwritten. By the second launch after a
/// rename the canonical store may already hold hosts accepted since — copying
/// over it would be the same data loss in the other direction.
#[test]
fn an_existing_canonical_store_is_never_overwritten() {
    let _lock = env_lock();
    let (_temp, _base, home, legacy) = plant_ssh_dir();
    let migrated = legacy.with_file_name(post_rename().ssh_known_hosts_file);
    let accepted_since = b"live.example.com ssh-ed25519 AAAAaccepted-after-the-rename\n";
    fs::write(&migrated, accepted_since).expect("plant a live canonical store");

    let _env = EnvScope::new().set("HOME", &home).set("USERPROFILE", &home);
    let _brand = brand::override_canonical(post_rename());

    assert_eq!(
        migrate_app_known_hosts().expect("the startup migration runs"),
        KnownHostsMigration::AlreadyMigrated
    );
    assert_eq!(
        fs::read(&migrated).expect("read the canonical store"),
        accepted_since,
        "host keys accepted since the rename must not be discarded"
    );
    assert_eq!(
        sha256(&fs::read(&legacy).expect("read the legacy store")),
        sha256(&frozen_store_bytes()),
        "and the legacy store is still untouched"
    );
}

/// Failure is explicit, and it is recorded.
///
/// "Migration failed" must never be indistinguishable from "there was nothing
/// to carry": that is precisely the fail-open state F-02 describes. The Err and
/// the recorded interlock are what
/// `ssh::connection::tests::accept_new_refuses_and_writes_nothing_after_a_failed_migration`
/// then consumes to refuse unknown hosts.
#[cfg(unix)]
#[test]
fn an_unwritable_destination_returns_err_and_records_the_failure() {
    use std::os::unix::fs::PermissionsExt;

    let _lock = env_lock();
    let (_temp, _base, home, legacy) = plant_ssh_dir();
    let ssh_dir = legacy.parent().expect("the .ssh dir").to_path_buf();
    let _env = EnvScope::new().set("HOME", &home).set("USERPROFILE", &home);
    let _brand = brand::override_canonical(post_rename());

    known_hosts_migration::set_migration_failed_for_test(false);
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o500))
        .expect("make the directory read-only");
    let outcome = migrate_app_known_hosts();
    let recorded = known_hosts_migration::migration_failed();
    // Restore before asserting so a failure cannot leave the interlock set for
    // the rest of this process or block the tempdir's cleanup.
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700)).expect("restore mode");
    known_hosts_migration::set_migration_failed_for_test(false);

    let error = outcome.expect_err("an unwritable destination must not return Ok");
    assert!(
        matches!(error, KnownHostsMigrationError::Write { .. }),
        "the error must name the write that failed, got {error:?}"
    );
    assert!(
        recorded,
        "a failed migration must set the interlock verify_host_key reads; without \
         it the accept-new path silently re-trusts every host the user already knew"
    );
    assert!(
        !ssh_dir.join(post_rename().ssh_known_hosts_file).exists(),
        "a failed migration must not leave a partial store behind"
    );
    assert_eq!(
        sha256(&fs::read(&legacy).expect("read the legacy store")),
        sha256(&frozen_store_bytes()),
        "and it must not have damaged the store it failed to copy"
    );
}

// ---------------------------------------------------------------------------
// The ledger — reds that go green when the capability lands
// ---------------------------------------------------------------------------

/// LEDGER — the legacy file name must leave `connection.rs`. `brand.rs` states
/// the rule: it and `src/shared/brand.ts` are the only two non-fixture files
/// permitted to hold a legacy brand string. A file name frozen into the
/// production source cannot be renamed without orphaning the store already
/// written under it.
#[test]
fn app_known_hosts_file_name_is_not_frozen_into_a_literal() {
    let facts = production_facts();
    let legacy = brand::LEGACY.ssh_known_hosts_file;
    assert!(
        !facts.string_literals.iter().any(|value| value == legacy),
        "{PRODUCTION_FILE} still hardcodes the legacy app-managed known_hosts file name \
         ({legacy:?}, see {PRODUCTION_FILE}:253). Only brand.rs may hold a legacy brand \
         string; this name must come from brand::canonical() so a rename can migrate the \
         host keys already stored under the old one."
    );
}

/// LEDGER — and it must be *replaced by the seam*, not merely deleted. A sed
/// can erase a literal; it can never author a `brand::canonical()` call, so
/// this positive assertion cannot be laundered green.
#[test]
fn app_known_hosts_path_is_built_from_the_brand_seam() {
    let _guard = brand::override_canonical(post_rename());
    assert_ne!(
        brand::canonical().ssh_known_hosts_file,
        brand::LEGACY.ssh_known_hosts_file,
        "the post-rename injection did not take"
    );

    let facts = production_facts();
    assert!(
        facts.brand_fields_read.contains("ssh_known_hosts_file"),
        "{PRODUCTION_FILE} must read crate::brand::canonical().ssh_known_hosts_file when \
         building the app-managed store path (see app_known_hosts_path at \
         {PRODUCTION_FILE}:252-254); brand fields read today: {:?}",
        facts.brand_fields_read,
    );
}

/// LEDGER — the migration must actually carry this root.
///
/// This one is executed, not inspected: `inventory_legacy_roots` is the public
/// production answer to "what will the migration carry", and the frozen store
/// is planted in a real `~/.ssh` that `HOME` points at. Today
/// `LegacyRootConfiguration::known_roots()`
/// (`src/conversation/migration/inventory.rs:84-119`) enumerates exactly three
/// conversation-data roots and has no field through which an `~/.ssh` path
/// could even be supplied, so the store is invisible to the pipeline — which is
/// the finding, executed.
#[test]
#[should_panic(expected = "the app-managed SSH host-key store is not carried by the migration")]
fn migration_carries_the_app_managed_known_hosts_store() {
    let _lock = env_lock();
    let (_temp, base, home, legacy) = plant_ssh_dir();
    let legacy_sha = sha256(&fs::read(&legacy).expect("read planted store"));
    assert_eq!(
        legacy_sha,
        sha256(&frozen_store_bytes()),
        "the planted store must be the frozen bytes"
    );

    let _env = EnvScope::new().set("HOME", &home).set("USERPROFILE", &home);
    let _brand = brand::override_canonical(post_rename());

    let state_root = base.join("state");
    fs::create_dir_all(&state_root).expect("create state root");
    let operation_dir = base.join("migration-operation");
    fs::create_dir_all(&operation_dir).expect("create operation dir");

    let configuration = LegacyRootConfiguration {
        host_state_root: state_root,
        standalone_session_roots: Vec::new(),
        standalone_workspace_manifest_roots: Vec::new(),
    };
    let inventory = inventory_legacy_roots(
        &configuration,
        uuid::Uuid::new_v4(),
        chrono::Utc::now(),
        &operation_dir,
    )
    .expect("legacy inventory runs");

    let carried: Vec<String> = inventory
        .roots
        .iter()
        .flat_map(|root| {
            let base = root.canonical_path.clone();
            root.files
                .iter()
                .map(move |file| format!("{base}/{}", file.relative_path))
        })
        .collect();
    let carries_store = carried
        .iter()
        .any(|path| path.ends_with(brand::LEGACY.ssh_known_hosts_file));

    assert!(
        carries_store,
        "the app-managed SSH host-key store is not carried by the migration: \
         {} holds the user's trusted host keys and nothing in \
         LegacyRootConfiguration::known_roots() looks at ~/.ssh. After the rename \
         the store reads empty, every known host becomes unknown, and accept-new \
         re-trusts whatever answers. Files the pipeline would carry: {carried:?}",
        legacy.display(),
    );
}

/// LEDGER — a failed migration must fail *closed*.
///
/// A best-effort copy that logs and continues re-creates the MITM window: the
/// canonical store is empty, `check_port` returns `NotFound` for every host the
/// user already trusted, and `verify_host_key` walks straight into the
/// `accept-new` branch. So the accept-new path has to be able to see that the
/// migration did not complete and refuse. Structural, because `verify_host_key`
/// is a private associated fn in a private module and cannot be called here.
#[test]
fn accept_new_is_gated_on_a_successful_store_migration() {
    let facts = production_facts();
    // The compat read has to exist for the gate to be meaningful: production
    // must know the legacy store's name in order to notice it was not carried.
    let knows_the_legacy_store = facts.legacy_fields_read.contains("ssh_known_hosts_file");

    assert!(
        knows_the_legacy_store,
        "accept-new is not gated on the host-key store migration: {PRODUCTION_FILE} never \
         consults brand::LEGACY.ssh_known_hosts_file, so it cannot tell 'this user had no \
         known hosts' from 'this user's known hosts were left behind under the old name'. \
         Those two states must not both take the accept-new branch at \
         {PRODUCTION_FILE}:305-310. Legacy brand fields read today: {:?}",
        facts.legacy_fields_read,
    );
}
