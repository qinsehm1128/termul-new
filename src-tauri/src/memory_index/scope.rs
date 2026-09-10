//! One project, and only one.
//!
//! `cli_session`'s scope helper (`allowed_scope_roots_from_registry`) walks the
//! default project plus **every non-archived project** in the registry. That is
//! correct for the feature it serves — a picker that lists the sessions of all
//! your projects — and wrong for this one. A memory bank that answers "what do
//! we know about this project" from another project's transcripts is not a
//! degraded answer; it is a cross-project information leak.
//!
//! So this is a separate, deliberately narrower gate: a fence holds exactly one
//! canonical project root, and it refuses to be built from a list.

use std::path::{Path, PathBuf};

use super::paths::{CanonicalProjectRoot, IndexLocation};
use super::types::SessionScope;
use super::{MemoryIndexError, MemoryIndexResult, ERR_OUT_OF_SCOPE, ERR_PROJECT_ROOT_INVALID};

/// The single-project boundary every read and write passes through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFence {
    project: CanonicalProjectRoot,
}

impl ProjectFence {
    /// Build a fence for one project root.
    pub fn single(project_root: &Path) -> MemoryIndexResult<Self> {
        Ok(Self {
            project: CanonicalProjectRoot::resolve(project_root)?,
        })
    }

    /// Build a fence from a caller-supplied list, requiring exactly one entry.
    ///
    /// Exists so callers that already speak in `scopePaths` lists (the HTTP and
    /// Tauri surfaces both do) get a hard rejection rather than a silent
    /// widening to the first or the union of the list.
    pub fn from_roots(roots: &[PathBuf]) -> MemoryIndexResult<Self> {
        match roots {
            [] => Err(MemoryIndexError::new(
                ERR_PROJECT_ROOT_INVALID,
                "the memory index is per-project; exactly one project root is required, got none",
            )),
            [single] => Self::single(single),
            many => Err(MemoryIndexError::new(
                ERR_OUT_OF_SCOPE,
                format!(
                    "the memory index is per-project; exactly one project root is required, got {}",
                    many.len()
                ),
            )),
        }
    }

    #[must_use]
    pub fn project(&self) -> &CanonicalProjectRoot {
        &self.project
    }

    #[must_use]
    pub fn namespace_key(&self) -> String {
        self.project.namespace_key()
    }

    /// Display name for the UI. Never a matching key.
    #[must_use]
    pub fn display_label(&self) -> String {
        self.project.display_label()
    }

    /// Resolve this project's index location under a host-injected state root.
    pub fn index_location(&self, state_root: &Path) -> MemoryIndexResult<IndexLocation> {
        IndexLocation::resolve(state_root, self.project.canonical())
    }

    /// Is `path` inside the fence?
    ///
    /// One-directional on purpose. The `cli_session` predicate also accepts a
    /// root that *contains* the candidate (`root.starts_with(&cwd_path)`), which
    /// makes a session run in `/Users/qs` count as a session of
    /// `/Users/qs/project/me/termul`. For a picker that leniency is a
    /// convenience; here it would admit a home-directory session — and
    /// everything it touched — into one project's memory.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        let Some(candidate) = normalize_candidate(path) else {
            return false;
        };
        self.project
            .lookup_forms()
            .iter()
            .any(|form| candidate == *form || candidate.starts_with(form))
    }

    /// Classify a transcript's recorded working directory.
    ///
    /// A missing, blank or outside cwd yields [`SessionScope::Unscoped`]. That
    /// is not the same as "belongs to the currently open project": ownership has
    /// to come from the transcript's own record or from the vendor folder that
    /// encodes the path, never from which project the UI happens to be showing.
    #[must_use]
    pub fn classify_cwd(&self, cwd: Option<&str>) -> SessionScope {
        match cwd.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) if self.contains(Path::new(value)) => SessionScope::Scoped,
            _ => SessionScope::Unscoped,
        }
    }

    /// Reject a path that is outside the fence, with the shared error code.
    pub fn require_contains(&self, path: &Path) -> MemoryIndexResult<()> {
        if self.contains(path) {
            return Ok(());
        }
        Err(MemoryIndexError::new(
            ERR_OUT_OF_SCOPE,
            format!(
                "{} is outside project {}",
                path.display(),
                self.project.canonical().display()
            ),
        ))
    }
}

fn normalize_candidate(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim_end_matches('/');
    Some(if trimmed.is_empty() {
        PathBuf::from(path)
    } else {
        PathBuf::from(trimmed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence() -> ProjectFence {
        ProjectFence::single(Path::new("/Users/qs/project/me/termul")).unwrap()
    }

    /// AC8. A list with more than one root is a rejection, not a widening.
    #[test]
    fn more_than_one_project_root_is_refused() {
        let error = ProjectFence::from_roots(&[
            PathBuf::from("/Users/qs/project/me/termul"),
            PathBuf::from("/Users/qs/project/other"),
        ])
        .unwrap_err();
        assert_eq!(error.code, ERR_OUT_OF_SCOPE);
        assert!(error.detail.contains("got 2"), "{}", error.detail);
    }

    #[test]
    fn an_empty_root_list_is_refused() {
        assert_eq!(
            ProjectFence::from_roots(&[]).unwrap_err().code,
            ERR_PROJECT_ROOT_INVALID
        );
    }

    #[test]
    fn exactly_one_root_builds_a_fence() {
        let fence =
            ProjectFence::from_roots(&[PathBuf::from("/Users/qs/project/me/termul")]).unwrap();
        assert_eq!(fence.display_label(), "termul");
    }

    #[test]
    fn the_project_root_and_its_descendants_are_inside() {
        let fence = fence();
        assert!(fence.contains(Path::new("/Users/qs/project/me/termul")));
        assert!(fence.contains(Path::new("/Users/qs/project/me/termul/")));
        assert!(fence.contains(Path::new("/Users/qs/project/me/termul/src-tauri/src")));
    }

    /// The leniency this module exists to remove. A session whose cwd is an
    /// ancestor of the project is not a session of the project — it could have
    /// been working in any sibling.
    #[test]
    fn an_ancestor_of_the_project_is_outside() {
        let fence = fence();
        for ancestor in ["/Users/qs", "/Users/qs/project", "/Users/qs/project/me", "/"] {
            assert!(
                !fence.contains(Path::new(ancestor)),
                "{ancestor} must not count as inside the project"
            );
        }
    }

    /// A sibling whose path shares a string prefix must not be admitted by a
    /// naive `starts_with` on the rendered string.
    #[test]
    fn a_prefix_sharing_sibling_is_outside() {
        let fence = fence();
        assert!(!fence.contains(Path::new("/Users/qs/project/me/termul-fork")));
        assert!(!fence.contains(Path::new("/Users/qs/project/me/termulator/src")));
    }

    #[test]
    fn relative_and_parent_paths_are_outside() {
        let fence = fence();
        assert!(!fence.contains(Path::new("src-tauri")));
        assert!(!fence.contains(Path::new("/Users/qs/project/me/termul/../other")));
    }

    /// Unproven ownership is `Unscoped`, which the store excludes by default.
    #[test]
    fn cwd_classification_never_guesses() {
        let fence = fence();
        assert_eq!(
            fence.classify_cwd(Some("/Users/qs/project/me/termul/src")),
            SessionScope::Scoped
        );
        for unproven in [None, Some(""), Some("   "), Some("/Users/qs"), Some("/tmp/x")] {
            assert_eq!(
                fence.classify_cwd(unproven),
                SessionScope::Unscoped,
                "{unproven:?} must not be claimed by this project"
            );
        }
    }

    #[test]
    fn require_contains_reports_the_shared_error_code() {
        let error = fence()
            .require_contains(Path::new("/etc/passwd"))
            .unwrap_err();
        assert_eq!(error.code, ERR_OUT_OF_SCOPE);
    }

    /// The fence must not be reachable from the fleet-wide helper. This is a
    /// source-level guard because the failure it prevents is a future edit, not
    /// a runtime state.
    #[test]
    fn this_module_does_not_reference_the_fleet_wide_scope_helper() {
        let source = include_str!("scope.rs");
        // Comments are where the helper is *explained*; the guard is about
        // whether it is *called*. Strip them, or this test fails on its own
        // documentation.
        let code = source
            .split("#[cfg(test)]")
            .next()
            .expect("non-test source")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>();
        assert!(
            !code.contains("allowed_scope_roots_from_registry"),
            "the per-project fence must not be built from the fleet-wide registry helper"
        );
    }
}
