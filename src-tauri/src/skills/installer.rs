use crate::skills::api_types::{
    ERR_INSTALL_DIGEST_MISMATCH, ERR_MANAGED_SKILL_PROTECTED, ERR_UNMANAGED_COLLISION,
};
use crate::skills::app_data::CanonicalSkillsRoot;
use crate::skills::digest::sha256_hex;
use crate::skills::manifest::SkillManifest;
use crate::skills::ownership;
use std::fs;
use std::path::Path;

pub fn collision_token(name: &str, existing: &str, incoming: &str) -> String {
    sha256_hex(format!("{name}:{existing}:{incoming}").as_bytes())
}

pub fn install_file(
    root: &CanonicalSkillsRoot,
    name: &str,
    source: &Path,
    confirm_token: Option<&str>,
) -> Result<SkillManifest, String> {
    if crate::skills::validate_skill_name(name).is_err() {
        return Err("invalid skill name".to_string());
    }
    let target_dir = root.canonical_dir().join(name);
    let target = target_dir.join("SKILL.md");
    if ownership::is_managed_skill(&target_dir) || ownership::is_managed_skill(&target) {
        return Err(ERR_MANAGED_SKILL_PROTECTED.to_string());
    }
    let raw = fs::read(source).map_err(|error| format!("read source: {error}"))?;
    let digest = sha256_hex(&raw);
    if target.exists() {
        let existing = sha256_hex(&fs::read(&target).map_err(|error| error.to_string())?);
        if existing != digest {
            let token = collision_token(name, &existing, &digest);
            if confirm_token != Some(token.as_str()) {
                return Err(format!("{ERR_UNMANAGED_COLLISION}: confirmToken={token}"));
            }
        } else {
            return Ok(SkillManifest {
                name: name.to_string(),
                digest,
                canonical_path: target,
                projections: Vec::new(),
            });
        }
    }
    let rehashed = fs::read(source).map_err(|error| format!("rehash source: {error}"))?;
    if sha256_hex(&rehashed) != digest {
        return Err(ERR_INSTALL_DIGEST_MISMATCH.to_string());
    }
    fs::create_dir_all(&target_dir)
        .map_err(|error| format!("create canonical directory: {error}"))?;
    let backup = if target.exists() {
        let path = target.with_extension("md.bak");
        fs::copy(&target, &path).map_err(|error| format!("backup canonical skill: {error}"))?;
        Some(path)
    } else {
        None
    };
    let temporary = target.with_extension(format!("md.tmp-{}", std::process::id()));
    if let Err(error) = fs::write(&temporary, &rehashed) {
        if let Some(backup_path) = backup {
            let _ = fs::remove_file(backup_path);
        }
        return Err(format!("write canonical temp: {error}"));
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        if let Some(backup_path) = &backup {
            let _ = fs::copy(backup_path, &target);
            let _ = fs::remove_file(backup_path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(format!("commit canonical skill: {error}"));
    }
    if let Some(backup_path) = backup {
        let _ = fs::remove_file(backup_path);
    }
    Ok(SkillManifest {
        name: name.to_string(),
        digest,
        canonical_path: target,
        projections: Vec::new(),
    })
}
