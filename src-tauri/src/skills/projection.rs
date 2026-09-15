use crate::skills::digest::sha256_hex;
use crate::skills::provider_config::ProjectionFallbackPolicy;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionMode {
    Symlink,
    Copy,
}

pub fn project(
    source: &Path,
    target: &Path,
    policy: ProjectionFallbackPolicy,
) -> Result<ProjectionMode, String> {
    if !source.is_file() {
        return Err("canonical source is unavailable".to_string());
    }
    if target.exists() {
        return Err("UNMANAGED_COLLISION".to_string());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    #[cfg(unix)]
    {
        match std::os::unix::fs::symlink(source, target) {
            Ok(()) => Ok(ProjectionMode::Symlink),
            Err(error) => match policy {
                ProjectionFallbackPolicy::Copy => {
                    copy(source, target).map(|_| ProjectionMode::Copy)
                }
                ProjectionFallbackPolicy::Deny => Err(format!("PROJECTION_UNSUPPORTED: {error}")),
                ProjectionFallbackPolicy::Ask => {
                    Err("PROJECTION_FALLBACK_CONFIRMATION_REQUIRED".to_string())
                }
            },
        }
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(source, target) {
            Ok(()) => Ok(ProjectionMode::Symlink),
            Err(error) => match policy {
                ProjectionFallbackPolicy::Copy => {
                    copy(source, target).map(|_| ProjectionMode::Copy)
                }
                ProjectionFallbackPolicy::Deny => Err(format!("PROJECTION_UNSUPPORTED: {error}")),
                ProjectionFallbackPolicy::Ask => {
                    Err("PROJECTION_FALLBACK_CONFIRMATION_REQUIRED".to_string())
                }
            },
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = policy;
        Err("PROJECTION_UNSUPPORTED".to_string())
    }
}

pub fn copy(source: &Path, target: &Path) -> Result<(), String> {
    fs::copy(source, target)
        .map(|_| ())
        .map_err(|error| format!("copy projection: {error}"))
}

pub fn projection_digest(path: &Path) -> Result<String, String> {
    Ok(sha256_hex(
        &fs::read(path).map_err(|error| error.to_string())?,
    ))
}

pub fn canonical_target(root: &Path, name: &str) -> PathBuf {
    root.join("canonical").join(name).join("SKILL.md")
}

pub fn mode_name(mode: ProjectionMode) -> &'static str {
    match mode {
        ProjectionMode::Symlink => "symlink",
        ProjectionMode::Copy => "copy",
    }
}
