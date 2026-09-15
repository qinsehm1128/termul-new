use crate::skills::catalog::SkillsCatalog;
use std::fs;
use std::path::Path;

pub fn load(path: &Path) -> Result<Option<SkillsCatalog>, String> {
    match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|error| format!("read catalog {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read catalog {}: {error}", path.display())),
    }
}

pub fn save_atomic(path: &Path, catalog: &SkillsCatalog) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "catalog path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("create catalog directory: {error}"))?;
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let body = serde_json::to_vec_pretty(catalog)
        .map_err(|error| format!("serialize catalog: {error}"))?;
    fs::write(&temp, body).map_err(|error| format!("write catalog temp: {error}"))?;
    fs::rename(&temp, path).map_err(|error| format!("replace catalog: {error}"))
}
