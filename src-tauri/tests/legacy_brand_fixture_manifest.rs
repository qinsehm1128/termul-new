//! T-H00 — sha256 guard over the frozen Rust legacy-brand fixture root.
//!
//! The fixtures encode what a *pre*-rename install left on disk. Every Wave-1
//! harness test reads them from disk rather than inlining a literal, so that a
//! repo-wide `sed 's/termul/se-manager/g'` cannot rewrite the assertion and its
//! subject in one stroke and leave the suite green.
//!
//! This manifest is what makes that structural rather than aspirational: a sed
//! that also rewrote the fixtures breaks every hash, and a sha256 is a hex
//! constant containing no brand string — the same sed cannot repair it.
//!
//! If this test goes red: a fixture changed. That is the failure, not the test.
//! Do not regenerate the manifest to make it green.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const MANIFEST_NAME: &str = "MANIFEST.sha256";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-brand")
}

/// Every fixture file as a `/`-joined path relative to the root, sorted.
fn list_fixture_files(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<String>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read_dir {} failed: {e}", dir.display()))
            .map(|entry| entry.expect("dir entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, root, found);
                continue;
            }
            if path.file_name().is_some_and(|name| name == MANIFEST_NAME) {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("fixture path is under the root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            found.push(relative);
        }
    }

    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// Parse `<sha256>  <path>` lines into `path -> sha256`.
fn read_manifest(root: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(root.join(MANIFEST_NAME))
        .expect("frozen fixture manifest is present");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (hash, path) = line
                .split_once("  ")
                .unwrap_or_else(|| panic!("malformed manifest line: {line}"));
            (path.to_string(), hash.to_string())
        })
        .collect()
}

fn sha256_of(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    format!("{:x}", hasher.finalize())
}

#[test]
fn manifest_covers_exactly_the_files_present_under_the_frozen_root() {
    let root = fixture_root();
    let on_disk = list_fixture_files(&root);
    let recorded: Vec<String> = read_manifest(&root).keys().cloned().collect();
    // Both directions matter: a *deleted* fixture is as much a breach of the
    // freeze as a modified one, and an *added* unhashed fixture is a hole.
    assert_eq!(on_disk, recorded);
}

#[test]
fn every_fixture_matches_its_recorded_sha256() {
    let root = fixture_root();
    let manifest = read_manifest(&root);
    let mut mismatches = Vec::new();
    for (relative, expected) in &manifest {
        let actual = sha256_of(&root.join(relative));
        if &actual != expected {
            mismatches.push(format!("{relative}: expected {expected}, got {actual}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "frozen fixtures changed:\n{}",
        mismatches.join("\n")
    );
}

/// Asserts the *reason* the freeze exists, so a future well-meaning
/// regeneration of the manifest cannot silently launder a rewritten fixture.
#[test]
fn fixtures_still_contain_the_legacy_brand_strings_the_harness_depends_on() {
    let root = fixture_root();

    let keychain = std::fs::read_to_string(root.join("keychain-entries.json")).unwrap();
    assert!(keychain.contains("\"com.termul.manager\""));
    // The fourth spelling analyze missed. Losing it silently loses every SSH
    // credential, so the freeze must pin it explicitly.
    assert!(keychain.contains("\"termul-ssh\""));

    let mcp = std::fs::read_to_string(root.join("agent-mcp-config.json")).unwrap();
    assert!(mcp.contains("\"termul\""));
    assert!(mcp.contains("termul-manager"));

    let skill = std::fs::read_to_string(root.join("user-skills/termul-scheduled-tasks.md")).unwrap();
    assert!(skill.contains("<!-- managed-by-termul:termul-scheduled-tasks -->"));

    assert!(root.join("fake-user-repo/.termul/mcp-servers.json").is_file());
    assert!(root.join("fake-user-repo/.termul/worktrees/feat-billing").is_dir());
    // Stored without the leading dot: a tracked `.gitignore` inside the fixture
    // would shadow the repo's own ignore rules for everything beneath it.
    // Tests materialize it as `.gitignore` in a temp copy.
    let gitignore = std::fs::read_to_string(root.join("fake-user-repo/gitignore")).unwrap();
    assert!(gitignore.lines().any(|line| line.trim() == ".termul/"));

    let env_names = std::fs::read_to_string(root.join("env-names.txt")).unwrap();
    assert_eq!(env_names.lines().filter(|l| !l.trim().is_empty()).count(), 65);
    assert!(env_names.lines().all(|line| line.starts_with("TERMUL_")));
}
