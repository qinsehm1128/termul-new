//! T-H19 — the two bundle-identifier appdata trees a pre-rename install left
//! behind, and the six subdirectories under them that the existing migration
//! pipeline has never looked at.
//!
//! # Why this reads from disk
//!
//! Renaming the bundle identifier moves `app_data_dir()`. Everything under the
//! old identifier — conversations, the remote-tunnel token, scheduled tasks,
//! the ACP catalog and its downloaded binaries and npm packages, the remote
//! intent store — stays exactly where it was, on a path the renamed app no
//! longer computes. There is no way to test that honestly against an inline
//! string: the question is not "is this constant spelled right", it is "is
//! there a directory full of the user's data that nothing points at any more".
//!
//! So every test here materializes the frozen fixture trees
//! (`tests/fixtures/legacy-brand/appdata-com.termul-manager.app/` and its
//! `.dev` twin) into a `TempDir`, hashes them, points the *real* production
//! root builders at the post-rename identifier, and then asks production what
//! it can see. The fixtures are read-only and sha256-guarded by
//! `legacy_brand_fixture_manifest.rs`; nothing here writes back to them.
//!
//! # The lever
//!
//! `HostConversationRoots` (`src/conversation/bootstrap.rs:37-69`) carries a
//! `legacy_session_roots: Vec<PathBuf>`. `standalone()` populates it from its
//! caller; `desktop()` hardcodes `Vec::new()`. That asymmetry is the seam the
//! rename migration has to use — the desktop host is precisely the one whose
//! root moves when the bundle identifier changes, and it is the one that today
//! declares it has no legacy roots at all.
//!
//! Downstream, `LegacyRootConfiguration::known_roots()`
//! (`src/conversation/migration/inventory.rs:84-119`) turns those roots into
//! the three source kinds the pipeline understands — `acp-sessions`,
//! `acp-chat-history`, `workspace-manifests`. That enumeration is the real
//! production answer to "what will the migration carry", and it is what the
//! per-subdirectory test below interrogates.
//!
//! # Seams Wave 4 must add
//!
//! 1. `HostConversationRoots::desktop` must populate `legacy_session_roots`
//!    with the `crate::brand::LEGACY.bundle_id` and `.bundle_id_dev` sibling
//!    trees of the canonical app-data root (both, not just prod).
//! 2. A copy-only merge that leaves the source bytes untouched. Every existing
//!    stage/finalize step is scoped to conversation data; there is no public
//!    entry point that carries the other six subdirectories, and
//!    `remote-tunnel/secrets.json` must land at mode 0600.
//! 3. The desktop app-data root itself (`src/lib.rs:1524-1548`) is resolved by
//!    Tauri from `tauri.conf.json`'s identifier, so the canonical/legacy pair
//!    has to be reachable from Rust — today `brand::LEGACY.bundle_id` has no
//!    production consumer at all.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use termul_manager_lib::brand::{self, BrandCanonical};
use termul_manager_lib::conversation::migration::{inventory_legacy_roots, LegacyRootConfiguration};
use termul_manager_lib::conversation::HostConversationRoots;

/// The six subdirectories that live under the appdata root and are absent from
/// `LegacyRootConfiguration::known_roots()`. Named here only so the failure
/// message can say *which* ones are uncovered; their existence is proven from
/// the frozen fixture, never assumed.
const UNCOVERED_SUBDIRS: &[&str] = &[
    "remote-tunnel",
    "scheduled-tasks/v1",
    "acp-catalog",
    "acp-registry-binaries",
    "acp-npm-packages",
    "remote",
];

/// `remote-tunnel/secrets.json` holds the frp auth token and the remote access
/// token. Git cannot carry a 0600 mode, so the temp copy is tightened to 0600
/// before the merge and asserted to still be 0600 after it.
const SECRETS_FILE: &str = "remote-tunnel/secrets.json";

fn post_rename() -> BrandCanonical {
    BrandCanonical {
        bundle_id: "com.se-manager.app",
        bundle_id_dev: "com.se-manager.app.dev",
        ..brand::DEFAULT_CANONICAL
    }
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(relative: &str) -> PathBuf {
    manifest_dir().join("tests/fixtures/legacy-brand").join(relative)
}

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

/// Relative path -> sha256, for every file under `root`.
fn digest_tree(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, found: &mut BTreeMap<String, String>) {
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
            let bytes = fs::read(&path)
                .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            found.insert(relative, format!("{:x}", hasher.finalize()));
        }
    }
    let mut found = BTreeMap::new();
    walk(root, root, &mut found);
    found
}

/// The three roots a pre/post-rename desktop install has on disk.
struct PlantedRoots {
    _temp: tempfile::TempDir,
    /// `<appdata>/com.termul-manager.app`
    legacy_prod: PathBuf,
    /// `<appdata>/com.termul-manager.app.dev`
    legacy_dev: PathBuf,
    /// `<appdata>/com.se-manager.app` — the post-rename root, freshly created.
    canonical: PathBuf,
    workspace_base: PathBuf,
}

impl PlantedRoots {
    fn legacy_trees(&self) -> [&PathBuf; 2] {
        [&self.legacy_prod, &self.legacy_dev]
    }
}

/// Materializes both frozen identifier trees plus an empty canonical sibling.
/// The canonical root exists but is empty: without it a "found the data" test
/// could not distinguish a deliberate legacy lookup from having nowhere else to
/// look.
fn plant_appdata_roots() -> PlantedRoots {
    let temp = tempfile::tempdir().expect("tempdir");
    // The migration's durability check rejects any path with a symlink
    // component, and on macOS `$TMPDIR` lives under `/var -> /private/var`.
    // Resolve it once here so the real production code runs instead of
    // bailing out on the harness's own scratch directory.
    let root = temp.path().canonicalize().expect("canonicalize tempdir");
    let appdata = root.join("Application Support");

    let legacy_prod = appdata.join(brand::LEGACY.bundle_id);
    let legacy_dev = appdata.join(brand::LEGACY.bundle_id_dev);
    copy_tree(
        &fixture(&format!("appdata-{}", brand::LEGACY.bundle_id)),
        &legacy_prod,
    );
    copy_tree(
        &fixture(&format!("appdata-{}", brand::LEGACY.bundle_id_dev)),
        &legacy_dev,
    );

    let canonical = appdata.join(post_rename().bundle_id);
    fs::create_dir_all(&canonical).expect("create canonical appdata root");

    let workspace_base = root.join("workspaces");
    fs::create_dir_all(&workspace_base).expect("create workspace base");

    PlantedRoots {
        _temp: temp,
        legacy_prod,
        legacy_dev,
        canonical,
        workspace_base,
    }
}

/// The frozen trees really are the pre-rename ones, and they really do carry
/// the six subdirectories the pipeline ignores. A precondition, not the red:
/// it stops a rewritten fixture from turning every assertion below into a
/// vacuous pass.
#[test]
fn the_frozen_appdata_fixtures_carry_both_identifiers_and_all_six_subdirs() {
    let roots = plant_appdata_roots();

    for tree in roots.legacy_trees() {
        assert!(
            tree.is_dir(),
            "legacy identifier tree {} is missing",
            tree.display()
        );
    }
    // The prod tree is the complete one; the dev twin exists to prove the
    // detector must find *both*, so it only carries a representative subset.
    assert!(!digest_tree(&roots.legacy_prod).is_empty());
    assert!(!digest_tree(&roots.legacy_dev).is_empty());

    let mut present = Vec::new();
    for subdir in UNCOVERED_SUBDIRS {
        if roots.legacy_prod.join(subdir).is_dir() {
            present.push(*subdir);
        }
    }
    assert_eq!(
        present, UNCOVERED_SUBDIRS,
        "the frozen prod appdata fixture must carry every uncovered subdirectory"
    );
    assert!(
        roots.legacy_prod.join(SECRETS_FILE).is_file(),
        "the frozen prod appdata fixture must carry {SECRETS_FILE}"
    );
}

/// A detector given the post-rename app-data root must find BOTH legacy
/// identifier trees. The real production constructor is
/// `HostConversationRoots::desktop`, which hardcodes `Vec::new()`.
#[test]
#[should_panic(expected = "HostConversationRoots::desktop must declare the legacy bundle-identifier trees")]
fn desktop_roots_detect_both_legacy_identifier_trees() {
    let roots = plant_appdata_roots();
    let _brand = brand::override_canonical(post_rename());
    assert_ne!(
        brand::canonical().bundle_id,
        brand::LEGACY.bundle_id,
        "the post-rename injection did not take"
    );

    // The real desktop bootstrap call, as made at src/lib.rs:1543.
    let host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());

    let declared = &host_roots.legacy_session_roots;
    let mut undetected = Vec::new();
    for tree in roots.legacy_trees() {
        if !declared.iter().any(|root| root.starts_with(tree)) {
            undetected.push(tree.display().to_string());
        }
    }
    assert!(
        undetected.is_empty(),
        "HostConversationRoots::desktop must declare the legacy bundle-identifier trees \
         so the rename migration can reach them; undetected: {undetected:?}; declared: {declared:?}",
    );
}

/// After a merge the canonical root must hold equivalent content, and the
/// legacy trees' bytes must be byte-for-byte unchanged — a rename migration
/// copies, it never moves or deletes.
#[test]
#[should_panic(expected = "the legacy inventory must cover both bundle-identifier trees")]
fn merge_copies_legacy_trees_and_leaves_the_source_bytes_untouched() {
    let roots = plant_appdata_roots();
    let before: Vec<BTreeMap<String, String>> =
        roots.legacy_trees().iter().map(|t| digest_tree(t)).collect();
    let _brand = brand::override_canonical(post_rename());

    // Real production chain: desktop roots -> legacy root configuration ->
    // the pipeline's own inventory pass.
    let host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());
    let configuration = LegacyRootConfiguration {
        host_state_root: host_roots.state_root.clone(),
        standalone_session_roots: host_roots.legacy_session_roots.clone(),
        standalone_workspace_manifest_roots: host_roots.legacy_workspace_manifest_roots.clone(),
    };

    let operation_dir = roots.canonical.join("migration-operation");
    fs::create_dir_all(&operation_dir).expect("create operation dir");
    let inventory = inventory_legacy_roots(
        &configuration,
        uuid::Uuid::new_v4(),
        chrono::Utc::now(),
        &operation_dir,
    )
    .expect("legacy inventory runs");

    let inventoried: BTreeSet<String> = inventory
        .roots
        .iter()
        .map(|root| root.canonical_path.clone())
        .collect();
    let mut missing = Vec::new();
    for tree in roots.legacy_trees() {
        let prefix = tree.to_string_lossy().into_owned();
        if !inventoried.iter().any(|path| path.starts_with(&prefix)) {
            missing.push(prefix);
        }
    }
    assert!(
        missing.is_empty(),
        "the legacy inventory must cover both bundle-identifier trees before anything can be \
         merged; missing: {missing:?}; inventoried: {inventoried:?}",
    );

    // Equivalent content at the new root.
    for (tree, snapshot) in roots.legacy_trees().iter().zip(&before) {
        for (relative, digest) in snapshot {
            let merged = roots.canonical.join(relative);
            let bytes = fs::read(&merged).unwrap_or_else(|e| {
                panic!(
                    "{relative} from {} was not merged into {}: {e}",
                    tree.display(),
                    roots.canonical.display()
                )
            });
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            assert_eq!(
                &format!("{:x}", hasher.finalize()),
                digest,
                "merged {relative} does not match the legacy source bytes"
            );
        }
    }

    // And the source is untouched: same files, same bytes, still there.
    for (tree, snapshot) in roots.legacy_trees().iter().zip(&before) {
        assert_eq!(
            &digest_tree(tree),
            snapshot,
            "the migration must copy, never move or delete: {} changed",
            tree.display()
        );
    }
}

/// The six subdirectories the existing legacy-stage pipeline does not cover.
///
/// "Covered" is deliberately *behavioural*, not path-shaped: the check is
/// whether the real `inventory_legacy_roots` pass actually enumerates a file
/// living under the subdirectory. Declaring an ancestor root is not enough —
/// each `LegacySourceKind` has its own scanner (`inventory_host_sessions`,
/// `inventory_chat_history`, `inventory_workspace_manifests`), and every one
/// of them filters to conversation data. A migration that merely *pointed* at
/// the legacy tree would still leave the scheduled tasks and the tunnel
/// credentials behind, so a prefix match would be a false green.
#[test]
#[should_panic(expected = "are not carried by the legacy-stage pipeline")]
fn every_appdata_subdirectory_is_covered_by_the_legacy_stage_pipeline() {
    let roots = plant_appdata_roots();
    let _brand = brand::override_canonical(post_rename());

    let host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());
    let configuration = LegacyRootConfiguration {
        host_state_root: host_roots.state_root.clone(),
        standalone_session_roots: host_roots.legacy_session_roots.clone(),
        standalone_workspace_manifest_roots: host_roots.legacy_workspace_manifest_roots.clone(),
    };
    let known: Vec<PathBuf> = configuration
        .known_roots()
        .into_iter()
        .map(|spec| spec.path)
        .collect();

    let operation_dir = roots.canonical.join("migration-operation");
    fs::create_dir_all(&operation_dir).expect("create operation dir");
    let inventory = inventory_legacy_roots(
        &configuration,
        uuid::Uuid::new_v4(),
        chrono::Utc::now(),
        &operation_dir,
    )
    .expect("legacy inventory runs");

    // Every absolute file path the pipeline says it will carry.
    let carried: BTreeSet<PathBuf> = inventory
        .roots
        .iter()
        .flat_map(|root| {
            let base = PathBuf::from(&root.canonical_path);
            root.files.iter().map(move |file| {
                base.join(file.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
        })
        .collect();

    let mut uncovered = Vec::new();
    for subdir in UNCOVERED_SUBDIRS {
        let source = roots.legacy_prod.join(subdir);
        assert!(
            source.is_dir(),
            "the frozen fixture must carry {subdir}; got {}",
            source.display()
        );
        if !carried.iter().any(|path| path.starts_with(&source)) {
            uncovered.push(*subdir);
        }
    }

    assert!(
        uncovered.is_empty(),
        "{} appdata subdirectories are not carried by the legacy-stage pipeline and would be \
         stranded under the old bundle identifier: {uncovered:?}; \
         known roots: {known:?}; files the pipeline would carry: {carried:?}",
        uncovered.len(),
    );
}

/// `remote-tunnel/secrets.json` carries the frp auth token and the remote
/// access token. A merge that widened it to 0644 would publish both to every
/// local account, so the mode is part of the contract, not an implementation
/// detail.
#[cfg(unix)]
#[test]
#[should_panic(expected = "must be merged into the canonical root at mode 0600")]
fn remote_tunnel_secrets_keeps_mode_0600_after_the_merge() {
    use std::os::unix::fs::PermissionsExt;

    let roots = plant_appdata_roots();
    let source = roots.legacy_prod.join(SECRETS_FILE);
    // Git cannot record 0600, so restore the real on-disk mode on the copy
    // before asserting the merge preserves it.
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600))
        .expect("tighten the temp copy to 0600");
    let source_mode = fs::metadata(&source).expect("stat source").permissions().mode() & 0o777;
    assert_eq!(source_mode, 0o600, "the temp copy starts at 0600");
    let source_bytes = fs::read(&source).expect("read source secrets");

    let _brand = brand::override_canonical(post_rename());
    let host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());
    let configuration = LegacyRootConfiguration {
        host_state_root: host_roots.state_root.clone(),
        standalone_session_roots: host_roots.legacy_session_roots.clone(),
        standalone_workspace_manifest_roots: host_roots.legacy_workspace_manifest_roots.clone(),
    };
    let operation_dir = roots.canonical.join("migration-operation");
    fs::create_dir_all(&operation_dir).expect("create operation dir");
    let _inventory = inventory_legacy_roots(
        &configuration,
        uuid::Uuid::new_v4(),
        chrono::Utc::now(),
        &operation_dir,
    )
    .expect("legacy inventory runs");

    let merged = roots.canonical.join(SECRETS_FILE);
    let merged_mode = fs::metadata(&merged)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or_else(|e| {
            panic!(
                "{SECRETS_FILE} must be merged into the canonical root at mode 0600, but {} does \
                 not exist: {e}",
                merged.display()
            )
        });
    assert_eq!(
        merged_mode, 0o600,
        "{SECRETS_FILE} must be merged into the canonical root at mode 0600, got {merged_mode:o}"
    );
    assert_eq!(
        fs::read(&merged).expect("read merged secrets"),
        source_bytes,
        "the merged secrets must be byte-identical to the legacy source"
    );
    // And the source keeps both its bytes and its mode.
    assert_eq!(
        fs::metadata(&source).expect("stat source").permissions().mode() & 0o777,
        0o600,
        "the migration must not relax the legacy secrets file"
    );
}
