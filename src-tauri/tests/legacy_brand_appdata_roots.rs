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
//! # The lever, and how Wave 4 answered it
//!
//! When these tests were written (Wave 1) they asserted a specific *design*:
//! that `HostConversationRoots::desktop` would declare the legacy trees in
//! `legacy_session_roots`, and that `LegacyRootConfiguration::known_roots()`
//! would then carry them. Wave 4 delivered the capability by a different
//! mechanism, and the assertions below were retargeted onto it. Two findings
//! forced that, and both are load-bearing:
//!
//! 1. **Shape.** `standalone_session_roots` entries are consumed as
//!    `LegacySourceKind::LegacyHostSessions` *leaf* directories — the inventory
//!    scans the path itself, it never appends `acp-sessions`. An `app_data_dir`
//!    root sits one level above that, so routing it through the field would
//!    point the session scanner at a directory of unrelated subtrees.
//! 2. **Channel.** Both identifier trees have to be *visible* (a user who
//!    renamed while holding both installs must be told about both), but only
//!    the *matching* one may be carried. prod carries prod and dev carries dev;
//!    merging them lets a dev experiment overwrite real user data.
//!
//! So the delivered seam is:
//!
//! - `crate::legacy_appdata::carry_forward` — a whole-tree, copy-only merge
//!   running *beside* the conversation pipeline rather than inside it. It has
//!   no source-kind filter, which is exactly why it reaches the six
//!   subdirectories the pipeline never looked at.
//! - `HostConversationRoots::legacy_appdata_roots` — a read-only declaration of
//!   both identifier trees, for the startup detector and the merge banner.
//!   `legacy_session_roots` stays empty on desktop, and one of the tests below
//!   pins that so the rejected design cannot quietly come back.
//!
//! `remote-tunnel/secrets.json` must still land at mode 0600.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use se_manager_lib::brand::{self, BrandCanonical};
use se_manager_lib::conversation::migration::{inventory_legacy_roots, LegacyRootConfiguration};
use se_manager_lib::conversation::HostConversationRoots;

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
/// identifier trees — including the install channel this process is not.
///
/// The second half is the anti-regression: those roots must NOT reach
/// `legacy_session_roots`. Putting them there is the rejected design, and it
/// fails in two ways at once (an appdata root is not an `acp-sessions` leaf,
/// and the dev tree would be merged into a release install).
#[test]
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

    let declared = &host_roots.legacy_appdata_roots;
    let mut undetected = Vec::new();
    for tree in roots.legacy_trees() {
        if !declared.iter().any(|root| root.starts_with(tree)) {
            undetected.push(tree.display().to_string());
        }
    }
    assert!(
        undetected.is_empty(),
        "HostConversationRoots::desktop must declare the legacy bundle-identifier trees \
         so detection and the merge banner can report them; undetected: {undetected:?}; \
         declared: {declared:?}",
    );

    assert!(
        host_roots.legacy_session_roots.is_empty(),
        "an app_data_dir root is not an acp-sessions leaf, and this vector spans BOTH install \
         channels — routing it into LegacyRootConfiguration would merge a dev build's data into \
         a release install; got {:?}",
        host_roots.legacy_session_roots,
    );
}

/// After a merge the canonical root must hold equivalent content, and the
/// legacy trees' bytes must be byte-for-byte unchanged — a rename migration
/// copies, it never moves or deletes.
///
/// "Equivalent content" is scoped to the *matching* install channel. This
/// process resolved the prod identifier, so the prod tree is carried and the
/// dev tree is deliberately left where it is: `bundle_id` and `bundle_id_dev`
/// name two separate installs whose contents genuinely differ, and a running
/// process is only ever one of them.
#[test]
fn merge_copies_legacy_trees_and_leaves_the_source_bytes_untouched() {
    let roots = plant_appdata_roots();
    let before: Vec<BTreeMap<String, String>> =
        roots.legacy_trees().iter().map(|t| digest_tree(t)).collect();
    let dev_only: BTreeSet<String> = {
        let prod = digest_tree(&roots.legacy_prod);
        digest_tree(&roots.legacy_dev)
            .into_keys()
            .filter(|relative| !prod.contains_key(relative))
            .collect()
    };
    assert!(
        !dev_only.is_empty(),
        "the dev fixture must hold at least one file the prod fixture does not, or the channel \
         isolation below asserts nothing"
    );
    let _brand = brand::override_canonical(post_rename());

    // The real production chain: the desktop constructor performs the
    // carry-forward before it returns.
    let _host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());

    // Equivalent content at the new root, for the matching channel.
    for (relative, digest) in &before[0] {
        let merged = roots.canonical.join(relative);
        let bytes = fs::read(&merged).unwrap_or_else(|e| {
            panic!(
                "{relative} from {} was not merged into {}: {e}",
                roots.legacy_prod.display(),
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

    // The other channel is NOT merged in.
    for relative in &dev_only {
        assert!(
            !roots.canonical.join(relative).exists(),
            "{relative} belongs to the dev install and must not be merged into the release \
             root — the two are separate installs with genuinely different contents"
        );
    }

    // And every source is untouched: same files, same bytes, still there.
    for (tree, snapshot) in roots.legacy_trees().iter().zip(&before) {
        assert_eq!(
            &digest_tree(tree),
            snapshot,
            "the migration must copy, never move or delete: {} changed",
            tree.display()
        );
    }
}

/// The six subdirectories the conversation pipeline does not cover must be
/// carried anyway.
///
/// "Covered" is deliberately *behavioural*, not path-shaped: the check is
/// whether a real file living under the subdirectory actually arrives at the
/// canonical root with its bytes intact. Declaring an ancestor root is not
/// enough — that is precisely the failure this test exists to catch.
///
/// The second assertion records WHY a separate mechanism was needed: each
/// `LegacySourceKind` has its own scanner (`inventory_host_sessions`,
/// `inventory_chat_history`, `inventory_workspace_manifests`) and every one of
/// them filters to conversation data, so the pipeline enumerates none of these
/// files no matter what roots it is handed. If some future change makes the
/// pipeline cover them, this assertion fails and says so — that is a design
/// change to make deliberately, not to discover.
#[test]
fn every_appdata_subdirectory_is_carried_even_though_the_pipeline_ignores_it() {
    let roots = plant_appdata_roots();
    let source_digests = digest_tree(&roots.legacy_prod);
    let _brand = brand::override_canonical(post_rename());

    let host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());

    let mut uncovered = Vec::new();
    for subdir in UNCOVERED_SUBDIRS {
        let source = roots.legacy_prod.join(subdir);
        assert!(
            source.is_dir(),
            "the frozen fixture must carry {subdir}; got {}",
            source.display()
        );
        let in_subdir: Vec<(&String, &String)> = source_digests
            .iter()
            .filter(|(relative, _)| relative.starts_with(&format!("{subdir}/")))
            .collect();
        assert!(
            !in_subdir.is_empty(),
            "the frozen fixture must hold at least one file under {subdir}, or this subdirectory \
             asserts nothing"
        );
        for (relative, digest) in in_subdir {
            let merged = roots.canonical.join(relative);
            match fs::read(&merged) {
                Ok(bytes) => {
                    let mut hasher = Sha256::new();
                    hasher.update(&bytes);
                    assert_eq!(
                        &format!("{:x}", hasher.finalize()),
                        digest,
                        "carried {relative} does not match the legacy source bytes"
                    );
                }
                Err(_) => uncovered.push(relative.clone()),
            }
        }
    }
    assert!(
        uncovered.is_empty(),
        "{} appdata files were left stranded under the old bundle identifier: {uncovered:?}",
        uncovered.len(),
    );

    // The conversation pipeline still does not enumerate any of them — which is
    // why the carry-forward above is a separate mechanism rather than a wider
    // root list.
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
    let carried_by_pipeline: BTreeSet<PathBuf> = inventory
        .roots
        .iter()
        .flat_map(|root| {
            let base = PathBuf::from(&root.canonical_path);
            root.files.iter().map(move |file| {
                base.join(file.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
        })
        .collect();
    for subdir in UNCOVERED_SUBDIRS {
        let source = roots.legacy_prod.join(subdir);
        assert!(
            !carried_by_pipeline.iter().any(|path| path.starts_with(&source)),
            "the conversation pipeline now enumerates {subdir}; the carry-forward's reason for \
             existing has changed and this file's premise needs revisiting. known roots: {known:?}"
        );
    }
}

/// `remote-tunnel/secrets.json` carries the frp auth token and the remote
/// access token. A merge that widened it to 0644 would publish both to every
/// local account, so the mode is part of the contract, not an implementation
/// detail.
#[cfg(unix)]
#[test]
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
    let _host_roots =
        HostConversationRoots::desktop(roots.canonical.clone(), roots.workspace_base.clone());

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
