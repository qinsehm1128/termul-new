//! T-H04 — per-repository workspace directory across the rename.
//!
//! # Why this reads a repository off disk instead of inlining a literal
//!
//! The assertion this file replaces lived in `src/worktree/mod.rs`
//! (`test_is_se_manager_managed_{true,false}`) and had the shape
//! `assert!(<path literal>.contains(<workspace dir literal>))`, with both
//! operands written on the same source line.
//!
//! Such an assertion cannot fail — and a repo-wide `sed` rewrites subject and
//! expectation together and leaves it green while every user's existing
//! pre-rename tree silently stops being recognised. T-A18 removed it rather
//! than updating its literal: editing the literal would have preserved a test
//! that never had any power to detect anything.
//!
//! So this file takes its subject from `tests/fixtures/legacy-brand/fake-user-repo/`,
//! a sha256-frozen snapshot of what a pre-rename install leaves in a user's
//! repository, copies it into a `TempDir`, and drives the **real** production
//! HTTP handlers over it while `brand::override_canonical` reports the
//! post-rename workspace directory. The expectation therefore comes from the
//! brand seam and the subject comes from a frozen artifact: no single edit can
//! move both.
//!
//! # Reachability note (reported as a Wave-4 seam gap)
//!
//! `mod worktree` and `registry_path` are private to `termul_manager_lib`, so
//! `WorktreeManager::*` and `mcp_servers_api::registry_path` cannot be called
//! from `tests/`. The nearest public seam is the web route layer
//! (`web::worktree_api`, `web::mcp_servers_api`), which calls exactly the same
//! implementations the desktop Tauri commands call. Everything below goes
//! through those routes.
//!
//! `worktree_api` hands `WorktreeManager` to `tokio::task::spawn_blocking`,
//! and `override_canonical` is deliberately thread-local, so the injected value
//! is **not** visible inside that closure. `mcp_servers_api` resolves its path
//! on the request thread and therefore does see it. Both facts were load-bearing
//! for how the reds below liquidated.
//!
//! T-M08 resolved that asymmetry rather than working around it:
//! `worktree_api::create` now calls `WorkspaceDirs::resolve()` on the request
//! thread and moves the resolved value into the blocking closure, and
//! `WorktreeManager` never reads the seam itself. `tests/brand_seam_thread_affinity.rs`
//! is the standing gate on that (FORBID-07).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use tempfile::TempDir;

use termul_manager_lib::brand::{self, BrandCanonical, DEFAULT_CANONICAL};
use termul_manager_lib::web::auth::IngressProvenance;
use termul_manager_lib::web::mcp_servers_api;
use termul_manager_lib::web::project_registry::ProjectRegistry;
use termul_manager_lib::web::sink::WsRelaySink;
use termul_manager_lib::web::worktree_api::{
    self, WorktreeCreateRequest, WorktreeProjectPathRequest,
};
use termul_manager_lib::web::ws::{AppState, HistoryMode};
use termul_manager_lib::{
    AcpManager, CwdTracker, ExitCodeTracker, GitTracker, PtyManager, TerminalEventHub,
};

/// The workspace directory the app writes *after* the rename.
fn post_rename() -> BrandCanonical {
    BrandCanonical {
        workspace_dir: ".se-manager",
        ..DEFAULT_CANONICAL
    }
}

fn fixture_repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-brand/fake-user-repo")
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination)
        .unwrap_or_else(|e| panic!("create {} failed: {e}", destination.display()));
    for entry in std::fs::read_dir(source)
        .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", source.display()))
    {
        let entry = entry.expect("fixture dir entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target)
                .unwrap_or_else(|e| panic!("copy to {} failed: {e}", target.display()));
        }
    }
}

fn git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Materialise the frozen fixture as a real git repository in a temp dir.
///
/// Returns the canonicalised repo root — `resolve_request_path` canonicalises
/// every incoming path, and on macOS `/var/folders/...` resolves through a
/// symlink to `/private/var/folders/...`. Handing the routes the uncanonical
/// form would fail the project-root containment check for the wrong reason.
fn materialise_legacy_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("acme-api");
    copy_tree(&fixture_repo(), &repo);

    // The fixture stores the ignore file dotless so a tracked `.gitignore`
    // cannot shadow this repository's own rules for everything beneath it.
    std::fs::rename(repo.join("gitignore"), repo.join(".gitignore"))
        .expect("materialise fixture gitignore as .gitignore");

    // `.keep` exists only so git preserves the legacy worktree's directory
    // shape in *this* repository. Dropping it leaves the directory empty, which
    // is what `git worktree add` accepts when adopting an existing path.
    let legacy_worktree = repo.join(".termul/worktrees/feat-billing");
    std::fs::remove_file(legacy_worktree.join(".keep")).expect("drop worktree shape placeholder");

    git(&repo, &["init", "--initial-branch=main", "."]);
    git(&repo, &["config", "user.email", "harness@example.invalid"]);
    git(&repo, &["config", "user.name", "legacy brand harness"]);
    // The developer's global config must not decide whether this fixture can be
    // committed: signing or a global hooks path would fail the setup, not the
    // contract under test.
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README.md"), "acme-api\n").expect("seed a commit subject");
    git(&repo, &["add", "README.md", ".gitignore"]);
    git(&repo, &["commit", "--no-verify", "-m", "initial"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feat-billing",
            ".termul/worktrees/feat-billing",
        ],
    );

    std::fs::canonicalize(&repo).expect("canonicalise the materialised repo root")
}

/// The smallest `AppState` the worktree / MCP-registry routes read from.
///
/// `web::test_pty_manager` is `#[cfg(test)] pub(crate)` and so invisible here;
/// this rebuilds the same shape from the crate's public constructors.
fn app_state(project_root: PathBuf) -> AppState {
    let events = TerminalEventHub::standalone();
    let cwd = Arc::new(CwdTracker::new(events.clone()));
    let git_tracker = Arc::new(GitTracker::new(None, events.clone()));
    let exit = Arc::new(ExitCodeTracker::new(events.clone()));
    let pty = Arc::new(PtyManager::new(
        events.clone(),
        cwd.clone(),
        git_tracker.clone(),
        exit.clone(),
    ));
    AppState {
        acp: Arc::new(AcpManager::new(vec![])),
        pty,
        terminal_events: events,
        cwd_tracker: cwd,
        git_tracker,
        exit_code_tracker: exit,
        relay: Arc::new(WsRelaySink::new()),
        registry: Arc::new(ProjectRegistry::new()),
        registry_persistence: None,
        projects_file: None,
        history_mode: HistoryMode::LiveOnly,
        conversation: None,
        workspace_manifest: None,
        acp_catalog: None,
        acp_install: None,
        store: None,
        project_root: Arc::new(parking_lot::RwLock::new(project_root)),
    }
}

async fn read_registry(state: &AppState) -> termul_manager_lib::web::fs_api::IpcBody<Value> {
    mcp_servers_api::get(State(state.clone())).await.0
}

/// Control test — proves the harness itself is wired.
///
/// If this ever fails, every assertion below is suspect: they could be failing
/// on a broken fixture path or a mis-built `AppState` rather than on the
/// contract they name.
#[tokio::test]
async fn harness_reaches_the_real_routes_over_the_frozen_legacy_repo() {
    let temp = TempDir::new().unwrap();
    let repo = materialise_legacy_repo(&temp);
    let state = app_state(repo.clone());

    let response = worktree_api::list(
        State(state.clone()),
        Json(WorktreeProjectPathRequest {
            project_path: repo.to_string_lossy().into_owned(),
        }),
    )
    .await;
    let body = axum::body::to_bytes(
        axum::response::IntoResponse::into_response(response).into_body(),
        usize::MAX,
    )
    .await
    .unwrap();
    let listed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(listed["success"], true, "worktree list route: {listed}");
    assert!(
        listed["data"]
            .as_array()
            .expect("worktree list data array")
            .iter()
            .any(|entry| entry["branch"] == "feat-billing"),
        "the legacy worktree under .termul/worktrees/ must stay enumerable: {listed}"
    );

    let registry = read_registry(&state).await;
    assert_eq!(registry.success, true, "mcp registry read: {registry:?}");
    assert_eq!(
        registry.data.as_ref().expect("registry data")[0]["name"],
        "filesystem",
        "the legacy .termul/mcp-servers.json must stay readable"
    );
}

/// A registry write under the post-rename brand must land in the *canonical*
/// workspace directory.
///
/// `mcp_servers_api::{get,put}` call `registry_path` on the request thread, so
/// the thread-local override is in force at exactly the line that hardcodes
/// `.termul` (`src/web/mcp_servers_api.rs:21`). Today the write goes to
/// `.termul/` regardless of the seam, which is what this catches.
#[tokio::test]
async fn mcp_registry_write_lands_under_the_canonical_workspace_dir() {
    let temp = TempDir::new().unwrap();
    let repo = materialise_legacy_repo(&temp);
    let state = app_state(repo.clone());

    let _guard = brand::override_canonical(post_rename());
    let canonical_dir = brand::canonical().workspace_dir;

    let Json(written) = mcp_servers_api::put(
        State(state.clone()),
        Json(json!([{
            "name": "filesystem",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/example/code/acme-api"]
        }])),
    )
    .await;
    assert_eq!(written.success, true, "mcp registry write: {written:?}");

    let canonical_registry = repo.join(canonical_dir).join("mcp-servers.json");
    assert!(
        canonical_registry.is_file(),
        "MCP registry write ignored the brand seam: canonical() reports the \
         workspace directory is {canonical_dir:?} but nothing was written to \
         {}. src/web/mcp_servers_api.rs:21 joins a hardcoded \".termul\".",
        canonical_registry.display()
    );
}

/// FORBID-04: legacy values are read-only. Re-writing the legacy registry after
/// the rename re-establishes a second live copy of the contract on the user's
/// disk, and the two then diverge with no reconciliation rule.
///
/// The write above is the same call; only the assertion differs, so this is
/// registered separately rather than sequenced behind the first assertion.
#[tokio::test]
async fn legacy_mcp_registry_is_read_only_after_the_rename() {
    let temp = TempDir::new().unwrap();
    let repo = materialise_legacy_repo(&temp);
    let state = app_state(repo.clone());

    let legacy_registry = repo.join(".termul").join("mcp-servers.json");
    let before = std::fs::read_to_string(&legacy_registry).expect("fixture registry is present");

    let _guard = brand::override_canonical(post_rename());
    let Json(written) = mcp_servers_api::put(
        State(state.clone()),
        Json(json!([{ "name": "filesystem", "command": "npx" }])),
    )
    .await;
    assert_eq!(written.success, true, "mcp registry write: {written:?}");

    let after =
        std::fs::read_to_string(&legacy_registry).expect("legacy registry must never be deleted");
    assert_eq!(
        after,
        before,
        "legacy MCP registry was rewritten after the rename: {} changed, but \
         LEGACY values may only ever be read (brand.rs FORBID-04)",
        legacy_registry.display()
    );
}

/// A worktree created with no explicit `targetPath` must land under the
/// canonical workspace directory, while the legacy worktree stays where it is.
///
/// `WorktreeManager::create` used to build `"{project}/.termul/worktrees/{name}/"`
/// from a literal, *inside* a `spawn_blocking` closure — where the thread-local
/// override is not visible and a seam read would have returned
/// `DEFAULT_CANONICAL` regardless. T-M08 took the only shape that actually
/// works: `worktree_api::create` resolves `WorkspaceDirs` on the request thread
/// and passes it in, so what this test injects is what the closure builds with.
/// A literal, or a seam read moved inside the closure, both fail here.
#[tokio::test]
async fn default_worktree_target_lands_under_the_canonical_workspace_dir() {
    let temp = TempDir::new().unwrap();
    let repo = materialise_legacy_repo(&temp);
    let state = app_state(repo.clone());

    let _guard = brand::override_canonical(post_rename());
    let canonical_dir = brand::canonical().workspace_dir;

    let response = worktree_api::create(
        State(state.clone()),
        axum::Extension(IngressProvenance::LocalOperator),
        Json(WorktreeCreateRequest {
            project_path: repo.to_string_lossy().into_owned(),
            name: "feat-payments".to_string(),
            branch: "feat-payments".to_string(),
            is_new_branch: true,
            start_ref: None,
            target_path: None,
        }),
    )
    .await;
    let bytes = axum::body::to_bytes(
        axum::response::IntoResponse::into_response(response).into_body(),
        usize::MAX,
    )
    .await
    .unwrap();
    let created: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created["success"], true, "worktree create route: {created}");

    // The legacy checkout is untouched by creating a new one.
    assert!(
        repo.join(".termul/worktrees/feat-billing").is_dir(),
        "the legacy worktree must survive a post-rename create"
    );

    let placed = created["data"]["path"]
        .as_str()
        .expect("worktree create returns a path")
        .to_string();
    let expected_segment = format!("/{canonical_dir}/worktrees/");
    assert!(
        placed.contains(&expected_segment),
        "default worktree target ignored the brand seam: canonical() reports \
         the workspace directory is {canonical_dir:?}, so the default target \
         must contain {expected_segment:?}, but the route placed it at \
         {placed:?}. src/worktree/mod.rs:488 formats a hardcoded \".termul\"."
    );
}

/// Creating a worktree teaches `.gitignore` the *new* directory without
/// un-teaching it the old one, and says it once.
///
/// Three separate failure modes in one flow, none of which the assertions above
/// would notice:
///
/// * dropping the pre-rename line — the repository may still hold that
///   directory, and un-ignoring it dumps a whole worktree tree into the user's
///   `git status`;
/// * never adding the new line — the directory this build writes to shows up as
///   untracked instead;
/// * checking "is either line present" rather than each line on its own — which
///   looks correct on the first create and silently appends a duplicate on the
///   second, or never appends the new line at all because the old one is there.
///
/// The frozen fixture's ignore file already carries the pre-rename line
/// (`fake-user-repo/gitignore`), so the "still there" half is anchored to a
/// sha256-guarded artifact rather than to something this test wrote itself.
#[tokio::test]
async fn creating_a_worktree_adds_the_canonical_ignore_line_and_keeps_the_legacy_one() {
    let temp = TempDir::new().unwrap();
    let repo = materialise_legacy_repo(&temp);
    let state = app_state(repo.clone());

    let gitignore = repo.join(".gitignore");
    let legacy_line = format!("{}/", brand::LEGACY.workspace_dir);
    let before = std::fs::read_to_string(&gitignore).expect("the fixture ships a .gitignore");
    assert!(
        before.lines().any(|line| line.trim() == legacy_line),
        "the frozen fixture must already ignore {legacy_line:?}; got:\n{before}"
    );

    let _guard = brand::override_canonical(post_rename());
    let canonical_line = format!("{}/", brand::canonical().workspace_dir);
    assert_ne!(
        canonical_line, legacy_line,
        "the post-rename injection did not take"
    );

    for name in ["feat-payments", "feat-search"] {
        let response = worktree_api::create(
            State(state.clone()),
            axum::Extension(IngressProvenance::LocalOperator),
            Json(WorktreeCreateRequest {
                project_path: repo.to_string_lossy().into_owned(),
                name: name.to_string(),
                branch: name.to_string(),
                is_new_branch: true,
                start_ref: None,
                target_path: None,
            }),
        )
        .await;
        let bytes = axum::body::to_bytes(
            axum::response::IntoResponse::into_response(response).into_body(),
            usize::MAX,
        )
        .await
        .unwrap();
        let created: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(created["success"], true, "worktree create {name}: {created}");
    }

    let after = std::fs::read_to_string(&gitignore).expect("read .gitignore back");
    let count = |needle: &str| after.lines().filter(|line| line.trim() == needle).count();
    assert_eq!(
        count(&canonical_line),
        1,
        "two creates must leave exactly one {canonical_line:?} line; got:\n{after}"
    );
    assert_eq!(
        count(&legacy_line),
        1,
        "the pre-rename ignore line must survive untouched and unduplicated; got:\n{after}"
    );
}
