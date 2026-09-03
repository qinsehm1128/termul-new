//! T-H13 — the two state roots a pre-rename install left on disk, resolved
//! through the real production resolvers rather than re-derived here.
//!
//! # Why this reads from disk
//!
//! A state root is not a string, it is a directory somebody's sessions are
//! sitting in. `assert_eq!(default_sessions_dir(), some_path_i_built_from_the
//! _same_constant)` proves nothing: the constant and the expectation move
//! together under a repo-wide `sed`, and the test stays green while the
//! resolver walks away from a populated directory.
//!
//! So each test here materializes the frozen fixture tree
//! (`tests/fixtures/legacy-brand/standalone-state/`,
//! `tests/fixtures/legacy-brand/documents-Termul/`) into a `TempDir` under the
//! *legacy* name, creates the *canonical* sibling empty, points the real env
//! vars at that temp root, injects the post-rename brand on this thread, and
//! then asks the real production function where to look. The fixture files
//! themselves are the oracle — nothing about "what a session file is called"
//! is inlined below.
//!
//! The fixtures are read-only and sha256-guarded by
//! `legacy_brand_fixture_manifest.rs`; every test copies before it touches.
//!
//! # Two decoupled identities, deliberately not merged
//!
//! - The standalone state root is named by `state_dir` / `state_dir_windows`:
//!   `$XDG_STATE_HOME/termul`, `~/.local/state/termul`, `%LOCALAPPDATA%\Termul`,
//!   `$TMPDIR/termul` (`src/web/config.rs:29-55` and `:287-315`).
//! - The visible session workspace root is named by `display_name`:
//!   `~/Documents/Termul` (`src/lib.rs:1524-1543`), and on the standalone side
//!   `<project_root>/Termul` (`src/web/config.rs:668`).
//!
//! One is a state directory, the other is a product display name. They happen
//! to share a stem today; renaming one does not rename the other. Merging them
//! into one test would encode a coupling that does not exist.
//!
//! # Feature gating
//!
//! `src/web/config.rs` is NOT behind `standalone-server` — `src/lib.rs:33`
//! declares `pub mod web;` unconditionally and `src/web/mod.rs:21` declares
//! `pub mod config;` unconditionally. Only the `termul-server` *binary* target
//! is gated (`Cargo.toml:28`). This file therefore compiles and runs both with
//! and without `--features standalone-server`; both were verified.
//!
//! # Seams Wave 4 must add
//!
//! 1. `web::config::default_sessions_dir` and
//!    `ServerConfig::service_account_state_dir` must build their path component
//!    from `crate::brand::canonical().state_dir` /
//!    `.state_dir_windows`, and must fall back to the
//!    `crate::brand::LEGACY.state_dir` tree when it is the one holding data.
//! 2. The desktop `~/Documents/<name>` root (`src/lib.rs:1533`, `:1538`) must
//!    read `crate::brand::canonical().display_name`. That expression lives
//!    inline inside the Tauri `setup` closure in `run()` and is not callable
//!    from an integration test, so it is asserted as a scoped source-text
//!    parity check below and flagged as a gap: Wave 4 should extract it into a
//!    named `pub fn` so it can be driven directly.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use termul_manager_lib::brand::{self, BrandCanonical};
use termul_manager_lib::web::config::{default_sessions_dir, ServerConfig};

/// The resolvers under test read *process*-global env vars while `cargo test`
/// runs test fns on parallel threads. The brand override is thread-local and
/// needs no lock; the environment does.
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

    fn remember(&mut self, key: &'static str) {
        self.saved.push((key, std::env::var_os(key)));
    }

    fn set(mut self, key: &'static str, value: impl AsRef<Path>) -> Self {
        self.remember(key);
        std::env::set_var(key, value.as_ref().as_os_str());
        self
    }

    fn unset(mut self, key: &'static str) -> Self {
        self.remember(key);
        std::env::remove_var(key);
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

/// Post-rename canonical values. `state_dir` and `display_name` are the two
/// identities this file covers; they are deliberately different words so a test
/// cannot accidentally satisfy one by resolving the other.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        state_dir: "se-manager",
        state_dir_windows: "Se",
        display_name: "Se",
        ..brand::DEFAULT_CANONICAL
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(relative: &str) -> PathBuf {
    manifest_dir().join("tests/fixtures/legacy-brand").join(relative)
}

/// Recursive copy. The fixture root is frozen, so nothing is ever written back.
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination)
        .unwrap_or_else(|e| panic!("create {} failed: {e}", destination.display()));
    let entries = fs::read_dir(source)
        .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", source.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target)
                .unwrap_or_else(|e| panic!("copy to {} failed: {e}", target.display()));
        }
    }
}

/// Every file under `root`, as `/`-joined paths relative to it.
fn relative_files(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, found: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, root, found);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("under root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            found.insert(relative);
        }
    }
    let mut found = BTreeSet::new();
    walk(root, root, &mut found);
    found
}

/// A `ServerConfig` whose only job is to be a receiver for
/// `service_account_state_dir`, which reads the environment and ignores `self`.
fn probe_config(project_root: PathBuf) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 8080,
        event_log_capacity: 4096,
        permission_timeout_secs: 60,
        permission_reconnect_grace_secs: 60,
        project_root,
        projects_file: None,
        sessions_dir: None,
        conversation_workspace_root: PathBuf::from("/tmp"),
        workspace_manifests_dir: None,
        acp_catalog_dir: None,
        store_file: None,
        remote_access_token_file: None,
        allowed_origins: Vec::new(),
    }
}

/// Lays out `<tmp>/state/<LEGACY.state_dir>` (populated from the fixture) next
/// to `<tmp>/state/<canonical.state_dir>` (the fresh post-rename root), and
/// returns `(tempdir, state_home)`.
fn plant_standalone_state(canonical_state_dir: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_home = temp.path().join("state");
    copy_tree(
        &fixture("standalone-state"),
        &state_home.join(brand::LEGACY.state_dir),
    );
    // The post-rename install's own root: present, but empty. Without it the
    // test could not tell "resolver found the legacy tree on purpose" from
    // "resolver had nowhere else to go".
    fs::create_dir_all(state_home.join(canonical_state_dir)).expect("create canonical state root");
    (temp, state_home)
}

// ---------------------------------------------------------------------------
// Identity 1 — the standalone state root (named by `state_dir`).
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "default_sessions_dir must resolve under crate::brand::canonical().state_dir")]
fn standalone_sessions_dir_resolves_under_the_canonical_state_dir_and_still_sees_legacy_sessions() {
    let _lock = env_lock();
    let (_temp, state_home) = plant_standalone_state(post_rename().state_dir);
    let legacy_sessions = state_home.join(brand::LEGACY.state_dir).join("sessions");
    // The fixture is the oracle for "what a session file is called".
    let legacy_session_files = relative_files(&legacy_sessions);
    assert!(
        !legacy_session_files.is_empty(),
        "the frozen standalone-state fixture must carry at least one session file"
    );

    let _env = EnvScope::new()
        .set("XDG_STATE_HOME", &state_home)
        .set("HOME", &state_home)
        .unset("TERMUL_SESSIONS_DIR");
    let _brand = brand::override_canonical(post_rename());

    let resolved = default_sessions_dir().expect("a sessions dir is resolvable under XDG_STATE_HOME");

    assert_eq!(
        resolved,
        state_home.join(brand::canonical().state_dir).join("sessions"),
        "default_sessions_dir must resolve under crate::brand::canonical().state_dir, \
         not a hardcoded {:?}; got {}",
        brand::LEGACY.state_dir,
        resolved.display(),
    );

    // Second half of the same contract: renaming the root must not orphan the
    // sessions already in the legacy one.
    for relative in &legacy_session_files {
        assert!(
            resolved.join(relative).is_file(),
            "session {relative} from the legacy state root is unreachable at the \
             post-rename sessions dir {}",
            resolved.display(),
        );
    }
}

#[test]
#[should_panic(
    expected = "service_account_state_dir must resolve under crate::brand::canonical().state_dir"
)]
fn standalone_service_account_state_dir_resolves_under_the_canonical_state_dir_and_still_sees_legacy_state(
) {
    let _lock = env_lock();
    let (_temp, state_home) = plant_standalone_state(post_rename().state_dir);
    let legacy_root = state_home.join(brand::LEGACY.state_dir);
    // store.json, workspace-manifests/, acp-catalog/, sessions/ — all of it,
    // enumerated from disk rather than listed here.
    let legacy_files = relative_files(&legacy_root);
    assert!(
        legacy_files.len() >= 4,
        "the frozen standalone-state fixture must carry the whole state root, got {legacy_files:?}"
    );

    let _env = EnvScope::new()
        .set("XDG_STATE_HOME", &state_home)
        .set("HOME", &state_home)
        .unset("TERMUL_SESSIONS_DIR");
    let _brand = brand::override_canonical(post_rename());

    let config = probe_config(state_home.clone());
    let resolved = config.service_account_state_dir();

    assert_eq!(
        resolved,
        state_home.join(brand::canonical().state_dir),
        "service_account_state_dir must resolve under crate::brand::canonical().state_dir, \
         not a hardcoded {:?}; got {}",
        brand::LEGACY.state_dir,
        resolved.display(),
    );

    for relative in &legacy_files {
        assert!(
            resolved.join(relative).is_file(),
            "{relative} from the legacy state root is unreachable at the post-rename \
             state root {}",
            resolved.display(),
        );
    }
}

// ---------------------------------------------------------------------------
// Identity 2 — the visible session workspace root (named by `display_name`).
// ---------------------------------------------------------------------------

#[test]
#[should_panic(
    expected = "conversation workspace root must be named by crate::brand::canonical().display_name"
)]
fn standalone_conversation_workspace_root_uses_the_canonical_display_name_and_still_sees_legacy_workspaces(
) {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    // `<project_root>/Termul` is the standalone twin of `~/Documents/Termul`;
    // both are named by `display_name`.
    let project_root = temp.path().join("home");
    copy_tree(
        &fixture("documents-Termul"),
        &project_root.join(brand::LEGACY.display_name),
    );
    fs::create_dir_all(project_root.join(post_rename().display_name))
        .expect("create canonical workspace root");
    let legacy_workspace_files = relative_files(&project_root.join(brand::LEGACY.display_name));
    assert!(
        !legacy_workspace_files.is_empty(),
        "the frozen documents-Termul fixture must carry at least one workspace"
    );
    // `--project-root` is canonicalized by `resolve_and_validate_project_root`.
    let canonical_project_root = project_root.canonicalize().expect("canonicalize project root");

    let sessions_dir = temp.path().join("sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let _env = EnvScope::new()
        .set("TERMUL_SESSIONS_DIR", &sessions_dir)
        .unset("TERMUL_CONVERSATION_WORKSPACE_ROOT")
        .unset("TERMUL_PROJECT_ROOT")
        .unset("TERMUL_STORE_FILE");
    let _brand = brand::override_canonical(post_rename());

    // The real CLI parser. `from_maintenance_args` is the auth-free public
    // entry into the same `from_args_with_auth_policy` body that resolves
    // `conversation_workspace_root` at src/web/config.rs:668.
    let config = ServerConfig::from_maintenance_args([
        "--project-root",
        &project_root.to_string_lossy(),
    ])
    .expect("maintenance args parse");

    let resolved = config.conversation_workspace_root();

    assert_eq!(
        resolved,
        canonical_project_root.join(brand::canonical().display_name),
        "conversation workspace root must be named by crate::brand::canonical().display_name, \
         not a hardcoded {:?}; got {}",
        brand::LEGACY.display_name,
        resolved.display(),
    );

    for relative in &legacy_workspace_files {
        assert!(
            resolved.join(relative).is_file(),
            "workspace file {relative} from the legacy {:?} root is unreachable at the \
             post-rename workspace root {}",
            brand::LEGACY.display_name,
            resolved.display(),
        );
    }
}

/// The desktop `~/Documents/<name>` root at `src/lib.rs:1524-1543` is built
/// inline inside the Tauri `setup` closure of `run()`. There is no `pub fn` to
/// call, so this asserts the same contract structurally: the `document_dir()`
/// / `home_dir()` fallback chain must join a component read from the brand
/// seam.
///
/// The assertion is deliberately *positive* — "the seam is referenced" rather
/// than "the old literal is absent". A `sed s/Termul/Se/g` can erase a literal
/// but can never author a `brand::canonical().display_name` call, so this
/// cannot be laundered green.
#[test]
#[should_panic(
    expected = "must join crate::brand::canonical().display_name"
)]
fn desktop_documents_workspace_root_is_built_from_the_display_name_seam() {
    let path = manifest_dir().join("src/lib.rs");
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("production file {} is unreadable: {e}", path.display()));
    let lines: Vec<&str> = source.lines().collect();

    let anchors: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("document_dir()"))
        .map(|(index, _)| index)
        .collect();
    assert!(
        !anchors.is_empty(),
        "src/lib.rs no longer resolves a document_dir(); this test must be retargeted"
    );

    // The whole `.or_else(...)` chain, document_dir through home_dir.
    let start = anchors.iter().min().copied().expect("anchor");
    let end = (anchors.iter().max().copied().expect("anchor") + 8).min(lines.len());
    let window = lines[start..end].join("\n");

    assert!(
        window.contains("brand::canonical().display_name"),
        "src/lib.rs:{}-{} must join crate::brand::canonical().display_name onto \
         document_dir()/home_dir() instead of hardcoding {:?}; window was:\n{window}",
        start + 1,
        end,
        brand::LEGACY.display_name,
    );
}
