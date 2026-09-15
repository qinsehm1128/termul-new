use crate::skills::catalog::{SkillRecord, SkillSource, SkillsCatalog};
use crate::skills::digest::sha256_hex;
use crate::skills::ownership;
use crate::skills::provider_config::{expand_path, ProviderConfigSnapshot};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const SKIP_SEGMENTS: &[&str] = &["node_modules", "vendor", "cache", "build", ".git"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredProject {
    pub id: String,
    pub root: PathBuf,
}

pub fn path_is_within(parent: &Path, child: &Path) -> bool {
    let parent = match canonicalize_if_exists(parent) {
        Some(path) => path,
        None => normalize(parent),
    };
    let child = match canonicalize_if_exists(child) {
        Some(path) => path,
        None => normalize(child),
    };
    child.starts_with(&parent)
}

fn canonicalize_if_exists(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        fs::canonicalize(path).ok()
    } else {
        None
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                let _ = out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn scan(
    config: &ProviderConfigSnapshot,
    home: &Path,
    config_root: &Path,
    projects: &[RegisteredProject],
) -> SkillsCatalog {
    let mut records: BTreeMap<(String, String, Option<String>), SkillRecord> = BTreeMap::new();
    let mut diagnostics = config.diagnostics.clone();
    for provider in config.providers.iter().filter(|provider| provider.enabled) {
        for template in &provider.global_roots {
            match expand_path(template, home, config_root, None) {
                Ok(root) => scan_root(
                    &mut records,
                    &mut diagnostics,
                    provider.id.as_str(),
                    "global",
                    None,
                    None,
                    &root,
                ),
                Err(error) => diagnostics.push(format!("{} global root: {error}", provider.id)),
            }
        }
        for project in projects {
            for template in &provider.project_roots {
                match expand_path(template, home, config_root, Some(&project.root)) {
                    Ok(root) => {
                        if !path_is_within(&project.root, &root) {
                            diagnostics.push(format!(
                                "PROJECT_OUTSIDE_BOUNDARY: {} project root {} escapes {}",
                                provider.id,
                                root.display(),
                                project.id
                            ));
                            continue;
                        }
                        scan_root(
                            &mut records,
                            &mut diagnostics,
                            &provider.id,
                            "project",
                            Some(&project.id),
                            Some(&project.root),
                            &root,
                        );
                    }
                    Err(error) => {
                        diagnostics.push(format!("{} project root: {error}", provider.id))
                    }
                }
            }
        }
    }
    let mut skills: Vec<_> = records.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name).then(a.scope.cmp(&b.scope)));
    SkillsCatalog {
        revision: 1,
        skills,
        diagnostics,
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::provider_config::ProviderConfigSnapshot;
    use std::fs;

    #[test]
    fn scans_registered_projects_and_marks_digest_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let global = home.join(".agents/skills/shared");
        let alternate = home.join(".alt/skills/shared");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&alternate).unwrap();
        fs::write(
            global.join("SKILL.md"),
            "---\nname: shared\ndescription: one\n---\nbody\n",
        )
        .unwrap();
        fs::write(
            alternate.join("SKILL.md"),
            "---\nname: shared\ndescription: alternate\n---\nother\n",
        )
        .unwrap();
        let project = temp.path().join("project");
        let project_skill = project.join(".agents/skills/shared");
        fs::create_dir_all(&project_skill).unwrap();
        fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: shared\ndescription: two\n---\nother\n",
        )
        .unwrap();
        let outside = temp.path().join("outside/.agents/skills/hidden");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("SKILL.md"), "---\nname: hidden\n---\nno\n").unwrap();
        let config = ProviderConfigSnapshot::from_layers(&[(
            "test",
            r#"
[providers.agents]
global_roots = ["{home}/.agents/skills"]
project_roots = ["{project}/.agents/skills"]
[providers.alt]
global_roots = ["{home}/.alt/skills"]
project_roots = ["{project}/.agents/skills"]
"#,
        )]);
        let catalog = scan(
            &config,
            &home,
            temp.path(),
            &[RegisteredProject {
                id: "p1".into(),
                root: project,
            }],
        );
        assert_eq!(catalog.skills.len(), 2);
        let shared = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "shared" && skill.scope == "global")
            .unwrap();
        assert!(shared.conflict);
        let project_shared = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "shared" && skill.scope == "project")
            .unwrap();
        assert!(!project_shared.conflict);
        assert!(!catalog.skills.iter().any(|skill| skill.name == "hidden"));
    }

    #[cfg(unix)]
    #[test]
    fn project_symlink_escape_is_not_scanned() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(project.join(".agents")).unwrap();
        let outside = temp.path().join("outside/skills/escaped");
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            outside.join("SKILL.md"),
            "---\nname: escaped\ndescription: no\n---\nbody\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("outside/skills"),
            project.join(".agents/skills"),
        )
        .unwrap();
        let config = ProviderConfigSnapshot::from_layers(&[(
            "test",
            r#"
[providers.agents]
global_roots = ["{home}/.agents/skills"]
project_roots = ["{project}/.agents/skills"]
"#,
        )]);
        let catalog = scan(
            &config,
            &home,
            temp.path(),
            &[RegisteredProject {
                id: "p1".into(),
                root: project,
            }],
        );
        assert!(!catalog.skills.iter().any(|skill| skill.name == "escaped"));
        assert!(catalog
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("PROJECT_OUTSIDE_BOUNDARY")));
    }
}

fn scan_root(
    records: &mut BTreeMap<(String, String, Option<String>), SkillRecord>,
    diagnostics: &mut Vec<String>,
    provider: &str,
    scope: &str,
    project_id: Option<&str>,
    project_root: Option<&Path>,
    root: &Path,
) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            diagnostics.push(format!(
                "{}: cannot read {}: {error}",
                provider,
                root.display()
            ));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            let Ok(canonical) = fs::canonicalize(&path) else {
                continue;
            };
            if let Some(project_root) = project_root {
                if !path_is_within(project_root, &canonical) {
                    diagnostics.push(format!(
                        "PROJECT_OUTSIDE_BOUNDARY: {} skill {} escapes project",
                        provider,
                        path.display()
                    ));
                    continue;
                }
            }
        }
        if !path.is_dir() {
            continue;
        }
        let folder_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if SKIP_SEGMENTS.contains(&folder_name) {
            continue;
        }
        let skill_path = path.join("SKILL.md");
        if let Ok(skill_meta) = fs::symlink_metadata(&skill_path) {
            if skill_meta.file_type().is_symlink() {
                let Ok(canonical) = fs::canonicalize(&skill_path) else {
                    continue;
                };
                let boundary = project_root.unwrap_or(root);
                if !path_is_within(boundary, &canonical) {
                    diagnostics.push(format!(
                        "PROJECT_OUTSIDE_BOUNDARY: {} skill file {} escapes root",
                        provider,
                        skill_path.display()
                    ));
                    continue;
                }
            }
        }
        let raw = match fs::read(&skill_path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let text = match String::from_utf8(raw.clone()) {
            Ok(text) => text,
            Err(error) => {
                diagnostics.push(format!(
                    "{}: invalid UTF-8 {}: {error}",
                    provider,
                    skill_path.display()
                ));
                continue;
            }
        };
        let (frontmatter, _) = match crate::skills::frontmatter::parse(&text) {
            Ok(value) => value,
            Err(error) => {
                diagnostics.push(format!(
                    "INVALID_FRONTMATTER: {}: invalid frontmatter {}: {error}",
                    provider,
                    skill_path.display()
                ));
                continue;
            }
        };
        let name = frontmatter
            .get("name")
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| folder_name.to_string());
        if crate::skills::validate_skill_name(&name).is_err() {
            diagnostics.push(format!(
                "{}: invalid skill name in {}",
                provider,
                skill_path.display()
            ));
            continue;
        }
        let description = frontmatter.get("description").cloned().unwrap_or_default();
        let metadata = serde_json::to_vec(&frontmatter).unwrap_or_default();
        let digest = package_digest(&path);
        let source = SkillSource {
            provider: provider.to_string(),
            scope: scope.to_string(),
            project_id: project_id.map(str::to_string),
            skill_md_path: skill_path.to_string_lossy().to_string(),
            digest: digest.clone(),
            metadata_digest: sha256_hex(&metadata),
        };
        let key = (
            name.clone(),
            scope.to_string(),
            project_id.map(str::to_string),
        );
        if let Some(record) = records.get_mut(&key) {
            record.conflict |= record.digest != source.digest;
            if record.conflict {
                record.status = "conflict".to_string();
            }
            record.managed |= ownership::is_managed_skill(&path);
            record.sources.push(source);
        } else {
            records.insert(
                key,
                SkillRecord {
                    name,
                    description,
                    scope: scope.to_string(),
                    project_id: project_id.map(str::to_string),
                    digest,
                    metadata_digest: source.metadata_digest.clone(),
                    status: "available".to_string(),
                    conflict: false,
                    drift: false,
                    managed: ownership::is_managed_skill(&path),
                    sources: vec![source],
                },
            );
        }
    }
}

fn package_digest(dir: &Path) -> String {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_package_files(dir, dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut payload = Vec::new();
    for (relative, bytes) in files {
        payload.extend_from_slice(relative.as_bytes());
        payload.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        payload.extend_from_slice(&bytes);
    }
    sha256_hex(&payload)
}

fn collect_package_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let name = entry.file_name();
            if SKIP_SEGMENTS.contains(&name.to_str().unwrap_or_default()) {
                continue;
            }
            collect_package_files(root, &path, out);
        } else if meta.is_file() {
            if let Ok(bytes) = fs::read(&path) {
                if let Ok(relative) = path.strip_prefix(root) {
                    out.push((relative.to_string_lossy().into_owned(), bytes));
                }
            }
        }
    }
}
