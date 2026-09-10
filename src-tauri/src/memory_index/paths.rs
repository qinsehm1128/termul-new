//! Where the index lives, and which vendor folders feed it.
//!
//! Two separate jobs, deliberately in one file because they share the same
//! rule about project identity: **the full canonical path is the key, the
//! folder basename is only a display name.**
//!
//! The vendors encode a project by replacing every `/` with `-`, which is a
//! many-to-one map: `/Users/qs/ns-req-go` and `/Users/qs/ns/req/go` both encode
//! to `-Users-qs-ns-req-go`. Encoding forward from a known full path lands on
//! exactly one folder; decoding a folder name back to a path does not. So this
//! module only ever goes forward, and the index namespace is keyed by a hash of
//! the full canonical path rather than by anything a user could collide.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    MemoryIndexError, MemoryIndexResult, ERR_PROJECT_ROOT_INVALID, ERR_STATE_ROOT_INSIDE_PROJECT,
    ERR_STATE_ROOT_INVALID,
};
use crate::cli_session::paths as vendor_paths;

/// Directory under the host state root that holds every project's index.
pub const INDEX_DIR_NAME: &str = "memory-index";
/// The SQLite database file inside one project's namespace.
pub const INDEX_FILE_NAME: &str = "index.sqlite3";
/// Hex characters of the path digest kept in the namespace key.
///
/// 16 hex chars = 64 bits. The key has to separate the project roots on one
/// machine, not resist an adversary, and a readable label rides alongside it.
const KEY_DIGEST_HEX_LEN: usize = 16;
/// Longest human-readable prefix kept in the namespace key.
const KEY_LABEL_MAX: usize = 32;

/// The vendors this index covers.
///
/// Deliberately **not** [`crate::cli_session::types::CliSessionAgentId`], which
/// has six variants. v1 covers three; a three-variant enum makes that a
/// type-level fact instead of a runtime filter someone can forget to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemoryVendor {
    ClaudeCode,
    Codex,
    Pi,
}

impl MemoryVendor {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Pi => "pi",
        }
    }

    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::ClaudeCode, Self::Codex, Self::Pi]
    }

    /// Parse a wire value. Returns `None` for the three vendors outside v1 so a
    /// client asking for `gemini-cli` gets a rejection, not silent emptiness.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude-code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            _ => None,
        }
    }
}

impl std::fmt::Display for MemoryVendor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A project root that has passed absolute/no-`..` validation and been resolved
/// through the filesystem when possible.
///
/// Carries both forms because they answer different questions: `canonical` is
/// the identity used for keying, while `declared` is what the caller passed and
/// is what an agent may have written into a vendor folder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectRoot {
    canonical: PathBuf,
    declared: PathBuf,
}

impl CanonicalProjectRoot {
    /// Validate and resolve a project root.
    ///
    /// Canonicalization is best-effort on purpose: a project directory that has
    /// been removed still has an index worth reading, and refusing to key it
    /// would make the index unreadable exactly when the transcripts are the
    /// only remaining record.
    pub fn resolve(project_root: &Path) -> MemoryIndexResult<Self> {
        let declared = normalize(project_root).ok_or_else(|| {
            MemoryIndexError::new(
                ERR_PROJECT_ROOT_INVALID,
                "project root must be absolute and must not contain '..'",
            )
        })?;
        let canonical = project_root
            .canonicalize()
            .ok()
            .and_then(|resolved| normalize(&resolved))
            .unwrap_or_else(|| declared.clone());
        Ok(Self {
            canonical,
            declared,
        })
    }

    #[must_use]
    pub fn canonical(&self) -> &Path {
        &self.canonical
    }

    #[must_use]
    pub fn declared(&self) -> &Path {
        &self.declared
    }

    /// Both forms, deduplicated — the set of paths a vendor folder could have
    /// been named after.
    #[must_use]
    pub fn lookup_forms(&self) -> Vec<PathBuf> {
        if self.canonical == self.declared {
            vec![self.canonical.clone()]
        } else {
            vec![self.canonical.clone(), self.declared.clone()]
        }
    }

    /// Display name only. Never a matching key: two different projects can share
    /// a basename, and the vendor encoding is not reversible anyway.
    #[must_use]
    pub fn display_label(&self) -> String {
        self.canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The namespace key: a readable label plus a digest of the **full**
    /// canonical path.
    ///
    /// The digest is what makes the key injective where the vendor encoding is
    /// not — `/Users/qs/ns-req-go` and `/Users/qs/ns/req/go` share a vendor
    /// folder name but get different keys here.
    #[must_use]
    pub fn namespace_key(&self) -> String {
        let digest = {
            let mut hasher = Sha256::new();
            hasher.update(b"memory-index-project-v1\0");
            hasher.update(self.canonical.as_os_str().as_encoded_bytes());
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let digest = &digest[..KEY_DIGEST_HEX_LEN];
        let label = sanitize_label(&self.display_label());
        if label.is_empty() {
            digest.to_string()
        } else {
            format!("{label}-{digest}")
        }
    }
}

/// Reject relative paths and anything containing `..`, then trim a trailing
/// separator so `/repo` and `/repo/` key identically.
fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        Some(PathBuf::from(path))
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Keep the label a safe single path component: lowercase alphanumerics and
/// dashes only, collapsed, bounded.
fn sanitize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(KEY_LABEL_MAX));
    let mut last_was_dash = false;
    for character in raw.chars() {
        if out.chars().count() >= KEY_LABEL_MAX {
            break;
        }
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Resolved on-disk location of one project's index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexLocation {
    pub state_root: PathBuf,
    pub project: CanonicalProjectRoot,
    pub namespace_key: String,
    pub index_dir: PathBuf,
    pub database_path: PathBuf,
}

impl IndexLocation {
    /// Resolve `<state_root>/memory-index/<namespace-key>/index.sqlite3`.
    ///
    /// `state_root` is **injected by the host**, never inferred here: the
    /// desktop names it from Tauri's `app_data_dir()` and the standalone server
    /// from its service-account state dir, and the two must not share a mutable
    /// root. Inferring it in a shared service is how those two roots become one.
    ///
    /// Fails when the state root sits inside the project. That configuration
    /// would put the index in the user's repository — the one placement the
    /// design analysis explicitly reversed — so it is rejected loudly rather
    /// than honored quietly.
    pub fn resolve(state_root: &Path, project_root: &Path) -> MemoryIndexResult<Self> {
        let state_root = normalize(state_root).ok_or_else(|| {
            MemoryIndexError::new(
                ERR_STATE_ROOT_INVALID,
                "state root must be absolute and must not contain '..'",
            )
        })?;
        let project = CanonicalProjectRoot::resolve(project_root)?;
        for form in project.lookup_forms() {
            if state_root == form || state_root.starts_with(&form) {
                return Err(MemoryIndexError::new(
                    ERR_STATE_ROOT_INSIDE_PROJECT,
                    format!(
                        "state root {} is inside project {}; the index must stay host-private",
                        state_root.display(),
                        form.display()
                    ),
                ));
            }
        }
        let namespace_key = project.namespace_key();
        let index_dir = state_root.join(INDEX_DIR_NAME).join(&namespace_key);
        let database_path = index_dir.join(INDEX_FILE_NAME);
        Ok(Self {
            state_root,
            project,
            namespace_key,
            index_dir,
            database_path,
        })
    }

    /// Create the namespace directory. Separate from [`Self::resolve`] so path
    /// resolution stays pure and testable without touching the filesystem.
    pub fn ensure_dir(&self) -> MemoryIndexResult<()> {
        std::fs::create_dir_all(&self.index_dir).map_err(|error| {
            MemoryIndexError::new(
                ERR_STATE_ROOT_INVALID,
                format!(
                    "could not create index namespace {}: {error}",
                    self.index_dir.display()
                ),
            )
        })
    }
}

/// Store roots for one vendor, as the vendor itself names them.
#[must_use]
pub fn vendor_store_roots(vendor: MemoryVendor) -> Vec<PathBuf> {
    match vendor {
        MemoryVendor::ClaudeCode => vendor_paths::claude_projects_dir().into_iter().collect(),
        MemoryVendor::Codex => vendor_paths::default_codex_home()
            .map(|home| home.join("sessions"))
            .into_iter()
            .collect(),
        MemoryVendor::Pi => vendor_paths::pi_sessions_dir().into_iter().collect(),
    }
}

/// A scan root for one vendor, plus whether the root itself proves project
/// ownership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorScanRoot {
    pub vendor: MemoryVendor,
    pub path: PathBuf,
    /// `Some(path)` when the folder name encodes this project, so every
    /// transcript beneath it belongs to the project without reading its body.
    /// `None` for Codex, whose store is not indexed by project at all — there
    /// ownership has to come from each file's own `session_meta.payload.cwd`.
    pub proven_cwd: Option<PathBuf>,
}

/// Scan roots for one project.
///
/// Claude and pi are resolved by encoding the project path forward into the
/// folder they actually use, so a hit is exact. Codex has no per-project
/// directory (`cli_session::paths` skips it for that reason), so its whole
/// session store is returned with `proven_cwd: None` and ownership is decided
/// per file.
#[must_use]
pub fn vendor_scan_roots(project: &CanonicalProjectRoot) -> Vec<VendorScanRoot> {
    let mut out: Vec<VendorScanRoot> = Vec::new();
    let mut push_unique = |root: VendorScanRoot| {
        if out
            .iter()
            .any(|existing| existing.vendor == root.vendor && existing.path == root.path)
        {
            return;
        }
        out.push(root);
    };

    for base in vendor_store_roots(MemoryVendor::ClaudeCode) {
        for form in project.lookup_forms() {
            let dir = base.join(vendor_paths::encode_claude_project_dir(&form));
            if dir.is_dir() {
                push_unique(VendorScanRoot {
                    vendor: MemoryVendor::ClaudeCode,
                    path: dir,
                    proven_cwd: Some(form.clone()),
                });
            }
        }
    }
    for base in vendor_store_roots(MemoryVendor::Pi) {
        for form in project.lookup_forms() {
            let dir = base.join(vendor_paths::encode_pi_project_dir(&form));
            if dir.is_dir() {
                push_unique(VendorScanRoot {
                    vendor: MemoryVendor::Pi,
                    path: dir,
                    proven_cwd: Some(form.clone()),
                });
            }
        }
    }
    for base in vendor_store_roots(MemoryVendor::Codex) {
        if base.is_dir() {
            push_unique(VendorScanRoot {
                vendor: MemoryVendor::Codex,
                path: base,
                proven_cwd: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str) -> CanonicalProjectRoot {
        // Bypass canonicalize for synthetic paths: the point of these tests is
        // the keying rule, not the filesystem.
        CanonicalProjectRoot {
            canonical: PathBuf::from(path),
            declared: PathBuf::from(path),
        }
    }

    /// The property the whole keying scheme exists for. These two real-shaped
    /// paths collapse to the same vendor folder name; if they also shared an
    /// index namespace, one project's transcripts would be served as the
    /// other's memory.
    #[test]
    fn paths_that_share_a_vendor_folder_name_get_different_namespaces() {
        let hyphenated = root("/Users/qs/ns-req-go");
        let nested = root("/Users/qs/ns/req/go");
        assert_eq!(
            vendor_paths::encode_claude_project_dir(hyphenated.canonical()),
            vendor_paths::encode_claude_project_dir(nested.canonical()),
            "these paths must actually collide in the vendor encoding, or this \
             test is not covering the case it was written for"
        );
        assert_ne!(hyphenated.namespace_key(), nested.namespace_key());
    }

    #[test]
    fn namespace_key_keeps_a_readable_label_and_a_full_path_digest() {
        let key = root("/Users/qs/project/me/termul").namespace_key();
        assert!(key.starts_with("termul-"), "unexpected key {key}");
        assert_eq!(key.len(), "termul-".len() + KEY_DIGEST_HEX_LEN);
        // Same basename, different parent → same label, different digest.
        let other = root("/Users/qs/elsewhere/termul").namespace_key();
        assert!(other.starts_with("termul-"));
        assert_ne!(key, other);
    }

    #[test]
    fn namespace_key_survives_a_label_with_no_usable_characters() {
        let key = root("/srv/···").namespace_key();
        assert_eq!(key.len(), KEY_DIGEST_HEX_LEN, "unexpected key {key}");
    }

    #[test]
    fn trailing_separator_does_not_change_identity() {
        let plain = CanonicalProjectRoot::resolve(Path::new("/nonexistent-project-root")).unwrap();
        let slashed = CanonicalProjectRoot::resolve(Path::new("/nonexistent-project-root/")).unwrap();
        assert_eq!(plain.namespace_key(), slashed.namespace_key());
    }

    #[test]
    fn relative_and_parent_project_roots_are_rejected() {
        assert_eq!(
            CanonicalProjectRoot::resolve(Path::new("relative/path"))
                .unwrap_err()
                .code,
            ERR_PROJECT_ROOT_INVALID
        );
        assert_eq!(
            CanonicalProjectRoot::resolve(Path::new("/repo/../etc"))
                .unwrap_err()
                .code,
            ERR_PROJECT_ROOT_INVALID
        );
    }

    /// AC1. The index path is a function of the host state root, and the
    /// project path appears in it only as an opaque key.
    #[test]
    fn index_lives_under_the_host_state_root_and_never_in_the_project() {
        let location = IndexLocation::resolve(
            Path::new("/Users/qs/Library/Application Support/com.se-manager.app"),
            Path::new("/Users/qs/project/me/termul"),
        )
        .unwrap();
        assert_eq!(
            location.database_path,
            PathBuf::from("/Users/qs/Library/Application Support/com.se-manager.app")
                .join(INDEX_DIR_NAME)
                .join(&location.namespace_key)
                .join(INDEX_FILE_NAME)
        );
        assert!(!location
            .index_dir
            .starts_with("/Users/qs/project/me/termul"));
    }

    /// A state root inside the project is the exact placement the analysis
    /// reversed. Rejecting it is what keeps that reversal from being undone by
    /// a configuration mistake.
    #[test]
    fn a_state_root_inside_the_project_is_refused() {
        let error = IndexLocation::resolve(
            Path::new("/Users/qs/project/me/termul/.se-manager"),
            Path::new("/Users/qs/project/me/termul"),
        )
        .unwrap_err();
        assert_eq!(error.code, ERR_STATE_ROOT_INSIDE_PROJECT);
    }

    #[test]
    fn a_sibling_state_root_sharing_a_path_prefix_is_allowed() {
        // `/repo-state` starts with the string `/repo` but is not inside it.
        IndexLocation::resolve(Path::new("/repo-state"), Path::new("/repo"))
            .expect("a sibling directory is not inside the project");
    }

    /// v1 covers three vendors. The three that were explicitly ruled out must
    /// not resolve — an unknown id has to be a rejection, not an empty result
    /// set that reads like "this project has no Gemini history".
    #[test]
    fn vendor_ids_cover_v1_only() {
        for (value, expected) in [
            ("claude-code", Some(MemoryVendor::ClaudeCode)),
            ("codex", Some(MemoryVendor::Codex)),
            ("pi", Some(MemoryVendor::Pi)),
        ] {
            assert_eq!(MemoryVendor::parse(value), expected);
        }
        for outside_v1 in ["gemini-cli", "cursor", "opencode", "", "Codex"] {
            assert_eq!(
                MemoryVendor::parse(outside_v1),
                None,
                "{outside_v1} must not resolve — v1 covers three vendors"
            );
        }
    }

    #[test]
    fn scan_roots_prove_ownership_for_dir_indexed_vendors_only() {
        let temp = tempfile::tempdir().unwrap();
        // `claude_projects_dir()` appends `projects` to CLAUDE_CONFIG_DIR, so
        // the fixture has to be laid out the way the vendor actually does.
        let claude_config = temp.path().join("claude-config");
        let claude = claude_config.join("projects");
        let pi = temp.path().join("pi-sessions");
        let codex = temp.path().join("codex-home");
        let project = PathBuf::from("/Users/dev/projects/se-manager");
        std::fs::create_dir_all(claude.join(vendor_paths::encode_claude_project_dir(&project)))
            .unwrap();
        std::fs::create_dir_all(pi.join(vendor_paths::encode_pi_project_dir(&project))).unwrap();
        std::fs::create_dir_all(codex.join("sessions")).unwrap();

        let _guard = VendorEnvGuard::set(&claude_config, &pi, &codex);
        let roots = vendor_scan_roots(&root(project.to_str().unwrap()));

        let claude_root = roots
            .iter()
            .find(|entry| entry.vendor == MemoryVendor::ClaudeCode)
            .expect("claude root");
        assert_eq!(claude_root.proven_cwd.as_deref(), Some(project.as_path()));
        let pi_root = roots
            .iter()
            .find(|entry| entry.vendor == MemoryVendor::Pi)
            .expect("pi root");
        assert_eq!(pi_root.proven_cwd.as_deref(), Some(project.as_path()));
        let codex_root = roots
            .iter()
            .find(|entry| entry.vendor == MemoryVendor::Codex)
            .expect("codex root");
        assert!(
            codex_root.proven_cwd.is_none(),
            "the codex store is not indexed by project, so its root proves nothing"
        );
    }

    /// `vendor_scan_roots` reads process env through `cli_session::paths`, so
    /// the env tests have to serialize and restore.
    struct VendorEnvGuard {
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    static VENDOR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl VendorEnvGuard {
        fn set(claude_config: &Path, pi: &Path, codex: &Path) -> Self {
            let lock = VENDOR_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let keys = ["CLAUDE_CONFIG_DIR", "PI_CODING_AGENT_DIR", "CODEX_HOME"];
            let previous = keys
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect();
            std::env::set_var("CLAUDE_CONFIG_DIR", claude_config);
            std::env::set_var("PI_CODING_AGENT_DIR", pi);
            std::env::set_var("CODEX_HOME", codex);
            Self {
                previous,
                _lock: lock,
            }
        }
    }

    impl Drop for VendorEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.previous {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}
