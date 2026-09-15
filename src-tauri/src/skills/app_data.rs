//! Paths owned by the Skills Hub.
//!
//! The hub deliberately derives its storage from the host's existing state
//! resolver instead of inventing a provider-specific home directory.

use std::path::{Path, PathBuf};

/// Directory containing canonical skills, manifests, and catalog state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalSkillsRoot(PathBuf);

impl CanonicalSkillsRoot {
    /// Resolve the root without creating it. Callers performing a write should
    /// create the directory only after all validation has succeeded.
    pub fn resolve(base: impl Into<PathBuf>) -> Result<Self, String> {
        let base = base.into();
        if !base.is_absolute() {
            return Err(format!(
                "skills state root must be absolute: {}",
                base.display()
            ));
        }
        Ok(Self(base.join("skills-hub")))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn canonical_dir(&self) -> PathBuf {
        self.0.join("canonical")
    }

    pub fn manifests_dir(&self) -> PathBuf {
        self.0.join("manifests")
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.0.join("catalog.json")
    }
}

/// Resolve the host state root used by the current runtime.
///
/// Desktop callers pass Tauri's `app_data_dir`; standalone callers use the
/// existing server state root. Keeping the base explicit also makes tests and
/// shared-live hosts deterministic.
pub fn resolve_from_base(base: impl Into<PathBuf>) -> Result<CanonicalSkillsRoot, String> {
    CanonicalSkillsRoot::resolve(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_skills_hub_without_touching_disk() {
        let root = CanonicalSkillsRoot::resolve(PathBuf::from("/tmp/se-state")).unwrap();
        assert_eq!(root.path(), Path::new("/tmp/se-state/skills-hub"));
        assert!(!root.path().exists() || root.path().is_dir());
    }

    #[test]
    fn rejects_relative_state_roots() {
        let error = CanonicalSkillsRoot::resolve(PathBuf::from("state")).unwrap_err();
        assert!(error.contains("absolute"));
    }
}
