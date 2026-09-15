use std::fs;
use std::path::Path;

pub const TERMUL_OWNER: &str = "termul";

pub fn is_managed_path(path: &Path) -> bool {
    path.join(".se-managed-skill").is_file() || path.join("managed-skills.json").is_file()
}

pub fn is_managed_skill(path: &Path) -> bool {
    let dir = if path.ends_with("SKILL.md") {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    if is_managed_path(dir) || dir.parent().is_some_and(is_managed_path) {
        return true;
    }
    let skill_md = if path.ends_with("SKILL.md") {
        path.to_path_buf()
    } else {
        dir.join("SKILL.md")
    };
    if let Ok(body) = fs::read_to_string(&skill_md) {
        let markers = [
            crate::brand::canonical().skill_marker,
            crate::brand::LEGACY.skill_marker,
        ];
        if markers.iter().any(|marker| body.contains(marker)) {
            return true;
        }
    }
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name == crate::brand::canonical().skill_name || name == crate::brand::LEGACY.skill_name
}

pub fn reject_unmanaged_collision(path: &Path) -> Result<(), String> {
    if path.exists() && !is_managed_path(path) && !is_managed_skill(path) {
        return Err("UNMANAGED_COLLISION".to_string());
    }
    Ok(())
}

pub fn reject_managed_protected(path: &Path) -> Result<(), String> {
    if is_managed_skill(path) {
        return Err("MANAGED_SKILL_PROTECTED".to_string());
    }
    Ok(())
}
