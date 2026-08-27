//! Validate client-supplied scope paths. Scan roots always come from the host
//! home; scopePaths select which vendor project folders are walked.

use std::path::{Component, Path, PathBuf};

use super::types::CliSessionScanIssue;

pub const SCOPE_PATHS_MAX: usize = 64;

pub fn normalize_scope_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    Some(path)
}

pub fn filter_scope_paths(
    raw: &[String],
    allowed_roots: Option<&[PathBuf]>,
) -> (Vec<PathBuf>, Vec<CliSessionScanIssue>) {
    let mut accepted = Vec::new();
    let mut issues = Vec::new();
    for (index, value) in raw.iter().take(SCOPE_PATHS_MAX).enumerate() {
        let Some(path) = normalize_scope_path(value) else {
            issues.push(CliSessionScanIssue {
                agent_id: "unknown".to_string(),
                path: value.clone(),
                message: "scope path must be absolute and must not contain '..'".to_string(),
            });
            continue;
        };
        if !is_within_allowed_roots(&path, allowed_roots) {
            issues.push(CliSessionScanIssue {
                agent_id: "unknown".to_string(),
                path: path.display().to_string(),
                message: "scope path is outside registered project roots".to_string(),
            });
            continue;
        }
        if !accepted.iter().any(|existing| existing == &path) {
            accepted.push(path);
        }
        let _ = index;
    }
    if raw.len() > SCOPE_PATHS_MAX {
        issues.push(CliSessionScanIssue {
            agent_id: "unknown".to_string(),
            path: String::new(),
            message: format!("scopePaths capped at {SCOPE_PATHS_MAX}"),
        });
    }
    (accepted, issues)
}

#[cfg(test)]
pub fn is_cwd_in_scope(cwd: Option<&str>, scope_paths: &[PathBuf]) -> bool {
    if scope_paths.is_empty() {
        return true;
    }
    let Some(cwd) = cwd.filter(|value| !value.is_empty()) else {
        return false;
    };
    let cwd_path = PathBuf::from(cwd);
    scope_paths
        .iter()
        .any(|root| cwd_path == *root || cwd_path.starts_with(root) || root.starts_with(&cwd_path))
}

fn is_within_allowed_roots(path: &Path, allowed_roots: Option<&[PathBuf]>) -> bool {
    match allowed_roots {
        None => true,
        Some(roots) => {
            !roots.is_empty()
                && roots
                    .iter()
                    .any(|root| path == root || path.starts_with(root) || root.starts_with(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_and_parent_scope_paths() {
        let (accepted, issues) =
            filter_scope_paths(&["../etc".to_string(), "rel".to_string()], None);
        assert!(accepted.is_empty());
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn accepts_absolute_paths_when_no_boundary() {
        let path = if cfg!(windows) {
            r"C:\repo".to_string()
        } else {
            "/repo".to_string()
        };
        let (accepted, issues) = filter_scope_paths(&[path.clone()], None);
        assert_eq!(accepted.len(), 1);
        assert!(issues.is_empty());
        assert!(is_cwd_in_scope(Some(&format!("{path}/pkg")), &accepted));
    }

    #[test]
    fn discards_paths_outside_registered_roots() {
        let root = PathBuf::from("/allowed");
        let (accepted, issues) =
            filter_scope_paths(&["/other".to_string()], Some(std::slice::from_ref(&root)));
        assert!(accepted.is_empty());
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn empty_allowed_roots_reject_all_scope_paths() {
        let empty: &[PathBuf] = &[];
        let (accepted, issues) = filter_scope_paths(&["/repo".to_string()], Some(empty));
        assert!(accepted.is_empty());
        assert_eq!(issues.len(), 1);
    }
}
