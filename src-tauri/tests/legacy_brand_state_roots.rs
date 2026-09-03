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
//! # Seam status
//!
//! 1. **Landed (T-M07).** `web::config::default_sessions_dir` and
//!    `ServerConfig::service_account_state_dir` both resolve through
//!    `web::config::state_root_under`, which names the root from
//!    `crate::brand::canonical().state_dir` / `.state_dir_windows` and carries
//!    the `crate::brand::LEGACY` tree forward into it (copy-only, repeatable).
//!    The two ledger entries below are struck; the four candidate parents each
//!    get their own resolution test.
//! 2. **Landed (T-M06 / T-A16).** Both `display_name` roots now read
//!    `crate::brand::canonical().display_name`: the standalone twin in
//!    `web::config`, driven directly through the real CLI parser, and the
//!    desktop `~/Documents/<name>` root, which lives inline inside the Tauri
//!    `setup` closure in `run()` with no `pub fn` to call and is therefore
//!    asserted as a scoped source-text parity check. Both ledger entries below
//!    are struck. The old root is *declared* legacy-readable rather than
//!    merged — the user's files there are never moved, so the reachability
//!    assertion runs against that declaration.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use se_manager_lib::brand::{self, BrandCanonical};
use se_manager_lib::conversation::HostConversationRoots;
use se_manager_lib::web::config::{default_sessions_dir, ServerConfig};

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

/// The state-root component on *this* platform. Windows capitalises it; the
/// unix roots do not.
fn platform_state_dir(values: BrandCanonical) -> &'static str {
    #[cfg(windows)]
    {
        values.state_dir_windows
    }
    #[cfg(not(windows))]
    {
        values.state_dir
    }
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
        .unset("SE_SESSIONS_DIR");
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
        .unset("SE_SESSIONS_DIR");
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
// The four candidate parents, one resolution test each.
//
// Both resolvers pick a *parent* from the environment and then name the root
// inside it. The two tests above pin the naming and the carry-forward through
// one parent; these pin which parent each candidate branch chooses, so a branch
// that silently stopped being reachable (or started resolving relative to the
// CWD) is visible on its own rather than only through whichever candidate the
// developer's machine happens to hit.
//
// The legacy tree is planted in every one of them: a candidate that resolves to
// the right *name* but skips the carry-forward would still orphan the sessions.
// ---------------------------------------------------------------------------

/// Candidate 1 — `$XDG_STATE_HOME/<state_dir>`.
#[cfg(unix)]
#[test]
fn candidate_xdg_state_home_resolves_and_carries_the_legacy_root_forward() {
    let _lock = env_lock();
    let (_temp, state_home) = plant_standalone_state(post_rename().state_dir);
    let _env = EnvScope::new()
        .set("XDG_STATE_HOME", &state_home)
        // A `HOME` that would resolve somewhere else, so a pass cannot come
        // from the next candidate down.
        .set("HOME", state_home.join("not-this-one"))
        .unset("SE_SESSIONS_DIR");
    let _brand = brand::override_canonical(post_rename());

    let resolved = default_sessions_dir().expect("XDG_STATE_HOME is a resolvable candidate");
    assert_eq!(
        resolved,
        state_home.join(brand::canonical().state_dir).join("sessions"),
        "XDG_STATE_HOME must name the state root directly beneath itself"
    );
    assert!(
        !relative_files(&resolved).is_empty(),
        "the legacy sessions must have been carried into {}",
        resolved.display()
    );
}

/// Candidate 2 — `$HOME/.local/state/<state_dir>`, used when `XDG_STATE_HOME`
/// is unset.
#[cfg(unix)]
#[test]
fn candidate_home_local_state_resolves_and_carries_the_legacy_root_forward() {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let state_home = home.join(".local").join("state");
    copy_tree(
        &fixture("standalone-state"),
        &state_home.join(brand::LEGACY.state_dir),
    );

    let _env = EnvScope::new()
        .unset("XDG_STATE_HOME")
        .set("HOME", &home)
        .unset("SE_SESSIONS_DIR");
    let _brand = brand::override_canonical(post_rename());

    let resolved = default_sessions_dir().expect("HOME is a resolvable candidate");
    assert_eq!(
        resolved,
        state_home.join(brand::canonical().state_dir).join("sessions"),
        "with XDG_STATE_HOME unset the root must sit under ~/.local/state"
    );
    assert!(
        !relative_files(&resolved).is_empty(),
        "the legacy sessions must have been carried into {}",
        resolved.display()
    );
}

/// Candidate 3 — `%LOCALAPPDATA%\<state_dir_windows>`.
///
/// A separate identity from the unix one: Windows capitalises the component, so
/// resolving it through `state_dir` rather than `state_dir_windows` would be
/// invisible on a case-insensitive filesystem right up until somebody mounts a
/// case-sensitive volume.
#[cfg(windows)]
#[test]
fn candidate_localappdata_resolves_and_carries_the_legacy_root_forward() {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let local_appdata = temp.path().join("LocalAppData");
    copy_tree(
        &fixture("standalone-state"),
        &local_appdata.join(brand::LEGACY.state_dir_windows),
    );

    let _env = EnvScope::new()
        .set("LOCALAPPDATA", &local_appdata)
        .unset("SE_SESSIONS_DIR");
    let _brand = brand::override_canonical(post_rename());

    let resolved = default_sessions_dir().expect("LOCALAPPDATA is a resolvable candidate");
    assert_eq!(
        resolved,
        local_appdata
            .join(brand::canonical().state_dir_windows)
            .join("sessions"),
        "the Windows root is named by state_dir_windows, not state_dir"
    );
    assert!(
        !relative_files(&resolved).is_empty(),
        "the legacy sessions must have been carried into {}",
        resolved.display()
    );
}

/// Candidate 4 — the OS temp dir, reached only by `service_account_state_dir`
/// and only when no platform state dir is discoverable at all.
///
/// `default_sessions_dir` deliberately returns `None` here instead: a durable
/// session store that lands in a directory the OS reclaims on reboot is worse
/// than a loud startup failure, and that asymmetry between the two resolvers is
/// part of the contract.
#[test]
fn candidate_temp_dir_is_the_last_resort_and_still_carries_the_legacy_root_forward() {
    let _lock = env_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let tmp_root = temp.path().join("tmp");
    fs::create_dir_all(&tmp_root).expect("create the probe temp dir");
    copy_tree(
        &fixture("standalone-state"),
        &tmp_root.join(platform_state_dir(brand::LEGACY)),
    );

    let mut env = EnvScope::new()
        .unset("XDG_STATE_HOME")
        .unset("HOME")
        .unset("LOCALAPPDATA")
        .unset("SE_SESSIONS_DIR");
    // `std::env::temp_dir()` reads TMPDIR on unix and TEMP/TMP on Windows.
    env = env
        .set("TMPDIR", &tmp_root)
        .set("TEMP", &tmp_root)
        .set("TMP", &tmp_root);
    let _env = env;
    let _brand = brand::override_canonical(post_rename());

    assert!(
        default_sessions_dir().is_none(),
        "with no platform state dir, the durable session store must refuse to \
         resolve rather than land in the OS temp dir"
    );

    let config = probe_config(tmp_root.clone());
    let resolved = config.service_account_state_dir();
    assert_eq!(
        resolved,
        tmp_root.join(platform_state_dir(brand::canonical())),
        "the temp-dir fallback must still be named from the brand seam"
    );
    assert!(
        !relative_files(&resolved).is_empty(),
        "the legacy state must have been carried into {}",
        resolved.display()
    );
}

// ---------------------------------------------------------------------------
// Identity 2 — the visible session workspace root (named by `display_name`).
// ---------------------------------------------------------------------------

/// Cleared by T-A16. The `#[should_panic]` pinned the first assertion below —
/// the root was a hardcoded `<project_root>/Termul` — and `web::config` now
/// names it from the seam, so the marker is gone and this is a live guard.
///
/// The second half changed shape when the marker came off, and deliberately.
/// As written it asserted the legacy workspaces were readable *at the
/// post-rename root*, which can only be true if something moved or copied
/// them. The accepted design is the opposite: the user's files in the old root
/// are never touched, and the root is instead **declared** legacy-readable
/// (M-06) so detection and the merge banner can see it. So reachability is
/// asserted through that declaration, and the fixture's bytes are asserted
/// unchanged — which is the property the old assertion would have destroyed.
#[test]
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
        .set("SE_SESSIONS_DIR", &sessions_dir)
        .unset("SE_CONVERSATION_WORKSPACE_ROOT")
        .unset("SE_PROJECT_ROOT")
        .unset("SE_STORE_FILE");
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

    // The real constructor the standalone host uses (`server_main.rs:148`),
    // fed the root the parser just resolved.
    let roots = HostConversationRoots::standalone(
        temp.path().join("server-state"),
        resolved.clone(),
        None,
        None,
    );
    let legacy_base = canonical_project_root.join(brand::LEGACY.display_name);
    assert_eq!(
        roots.legacy_workspace_bases,
        vec![legacy_base.clone()],
        "the pre-rename {:?} root sitting next to {} must be declared legacy-readable, or the \
         merge banner never mentions it and the user has no way to reach a workspace they can \
         still see in their own file manager",
        brand::LEGACY.display_name,
        resolved.display(),
    );

    for relative in &legacy_workspace_files {
        assert!(
            legacy_base.join(relative).is_file(),
            "workspace file {relative} is no longer readable under the declared legacy root {}",
            legacy_base.display(),
        );
    }

    // Read-only means read-only: nothing was moved out of the old root and
    // nothing was copied into the new one.
    assert_eq!(
        relative_files(&legacy_base),
        legacy_workspace_files,
        "the legacy workspace root must be byte-for-byte untouched"
    );
    assert!(
        relative_files(&resolved).is_empty(),
        "the post-rename workspace root must still be empty; declaring the legacy root \
         read-only must not copy anything into it"
    );
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
