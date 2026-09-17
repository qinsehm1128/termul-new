//! Host-owned source validation and staging for remote Skills.
//!
//! Remote content is downloaded only by the Rust host into a temporary directory.
//! The renderer never supplies a command and no package lifecycle is executed.

use crate::skills::api_types::{
    SkillSource, SkillSourceMetadata, SkillsError, ERR_REMOTE_SOURCE_INVALID,
    ERR_REMOTE_SOURCE_UNAVAILABLE, ERR_REMOTE_SOURCE_UNSUPPORTED,
};
use crate::skills::digest::sha256_hex;
use futures_util::StreamExt;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use url::{Host, Url};

const MAX_REMOTE_BYTES: u64 = 64 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatedSkillSource {
    Local {
        path: String,
    },
    Github {
        repository: String,
        reference: Option<String>,
        subpath: Option<String>,
    },
    Npm {
        package: String,
        specifier: Option<String>,
        subpath: Option<String>,
    },
    Url {
        url: String,
        expected_sha256: Option<String>,
    },
}

pub struct StagingDir(PathBuf);

impl StagingDir {
    fn new() -> Result<Self, SkillsError> {
        let path = std::env::temp_dir().join(format!("termul-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path)
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct StagedSkill {
    pub root: StagingDir,
    pub skill_md: PathBuf,
    pub name: String,
    pub content: Vec<u8>,
    pub metadata: SkillSourceMetadata,
}

fn valid_package_name(package: &str) -> bool {
    let (scope, name) = package.strip_prefix('@').map_or((None, package), |value| {
        value
            .split_once('/')
            .map_or((Some(""), value), |(scope, name)| (Some(scope), name))
    });
    !package.is_empty()
        && package.len() <= 214
        && !package.starts_with('-')
        && !package.ends_with('-')
        && !package.contains("..")
        && scope.is_none_or(|scope| {
            !scope.is_empty()
                && scope
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        && !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
}

fn valid_npm_specifier(specifier: Option<&str>) -> bool {
    specifier.is_none_or(|value| {
        let version = value.strip_prefix('@').unwrap_or(value);
        !version.is_empty()
            && !version.starts_with('-')
            && value.len() <= 128
            && version.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || matches!(c, '.' | '-' | '+' | '^' | '~' | '<' | '>' | '=' | '*' | '|')
            })
    })
}

fn valid_git_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("..")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn ipv4_is_blocked(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
}

fn host_is_blocked(host: Host<&str>) -> bool {
    match host {
        Host::Domain(host) => host.eq_ignore_ascii_case("localhost") || host.ends_with(".local"),
        Host::Ipv4(ip) => ipv4_is_blocked(ip),
        Host::Ipv6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(ipv4_is_blocked)
        }
    }
}

fn validate_https(value: &str) -> Result<String, SkillsError> {
    let parsed = Url::parse(value)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_INVALID, error.to_string()))?;
    let Some(host) = parsed.host() else {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    };
    if parsed.scheme() != "https" || host_is_blocked(host) {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
    }
    Ok(parsed.to_string())
}

pub fn validate_source(source: &SkillSource) -> Result<ValidatedSkillSource, SkillsError> {
    match source {
        SkillSource::Local { source_path } => {
            if !Path::new(source_path).is_absolute() {
                return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
            }
            Ok(ValidatedSkillSource::Local {
                path: source_path.clone(),
            })
        }
        SkillSource::Github {
            repository_or_url,
            reference,
            subpath,
        } => {
            let repository = if repository_or_url.starts_with("https://") {
                let normalized = validate_https(repository_or_url)?;
                let parsed = Url::parse(&normalized)
                    .map_err(|_| SkillsError::coded(ERR_REMOTE_SOURCE_INVALID))?;
                if !matches!(
                    parsed.host_str(),
                    Some("github.com" | "codeload.github.com")
                ) {
                    return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
                }
                normalized
            } else {
                let valid = repository_or_url.split('/').count() == 2
                    && repository_or_url.split('/').all(|part| {
                        !part.is_empty()
                            && part
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                    });
                if !valid {
                    return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
                }
                repository_or_url.clone()
            };
            if reference
                .as_deref()
                .is_some_and(|reference| !valid_git_reference(reference))
                || subpath
                    .as_deref()
                    .is_some_and(|path| !safe_relative_path(path))
            {
                return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
            }
            Ok(ValidatedSkillSource::Github {
                repository,
                reference: reference.clone(),
                subpath: subpath.clone(),
            })
        }
        SkillSource::Npm {
            package,
            version_or_specifier,
            subpath,
        } => {
            if !valid_package_name(package)
                || !valid_npm_specifier(version_or_specifier.as_deref())
                || subpath
                    .as_deref()
                    .is_some_and(|path| !safe_relative_path(path))
            {
                return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
            }
            Ok(ValidatedSkillSource::Npm {
                package: package.clone(),
                specifier: version_or_specifier.clone(),
                subpath: subpath.clone(),
            })
        }
        SkillSource::Url {
            url,
            expected_sha256,
        } => {
            let normalized = validate_https(url)?;
            if expected_sha256.as_deref().is_some_and(|digest| {
                digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit())
            }) {
                return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
            }
            Ok(ValidatedSkillSource::Url {
                url: normalized,
                expected_sha256: expected_sha256.clone(),
            })
        }
    }
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, SkillsError> {
    let client = reqwest::Client::builder()
        .user_agent("Termul-SkillsHub/1")
        .redirect(reqwest::redirect::Policy::none())
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    if response.status().is_redirection() {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    }
    if !response.status().is_success() {
        return Err(SkillsError::new(
            ERR_REMOTE_SOURCE_UNAVAILABLE,
            format!("HTTP {}", response.status()),
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_REMOTE_BYTES)
    {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    }
    let mut output = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
        if output.len() as u64 + chunk.len() as u64 > MAX_REMOTE_BYTES {
            return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn copy_skill_file(source: &Path, destination: &Path) -> Result<(), SkillsError> {
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_REMOTE_BYTES {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    }
    let content = std::fs::read(source)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    std::fs::write(destination, content)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))
}

fn find_skill_md(root: &Path, subpath: Option<&str>) -> Result<PathBuf, SkillsError> {
    let start = if let Some(subpath) = subpath {
        if !safe_relative_path(subpath) {
            return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
        }
        root.join(subpath)
    } else {
        root.to_path_buf()
    };
    if start.is_file() && start.file_name().is_some_and(|name| name == "SKILL.md") {
        return Ok(start);
    }
    let mut candidates = Vec::new();
    let mut stack = vec![start];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "SKILL.md") {
                candidates.push(path);
            }
            if candidates.len() > 1 && subpath.is_some() {
                return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
            }
        }
    }
    if candidates.len() != 1 {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_INVALID));
    }
    Ok(candidates.remove(0))
}

fn staged_skill(
    root: StagingDir,
    skill_md: PathBuf,
    source_type: &str,
    locator: String,
    expected: Option<String>,
) -> Result<StagedSkill, SkillsError> {
    let metadata = std::fs::symlink_metadata(&skill_md)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_REMOTE_BYTES {
        return Err(SkillsError::coded(ERR_REMOTE_SOURCE_UNSUPPORTED));
    }
    let content = std::fs::read(&skill_md)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    let (frontmatter, _) = crate::skills::parse_skill_md(
        std::str::from_utf8(&content).map_err(|_| SkillsError::coded(ERR_REMOTE_SOURCE_INVALID))?,
    )
    .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_INVALID, error))?;
    let name = frontmatter
        .get("name")
        .cloned()
        .or_else(|| {
            skill_md
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .ok_or_else(|| SkillsError::coded(ERR_REMOTE_SOURCE_INVALID))?;
    crate::skills::validate_skill_name(&name)
        .map_err(|_| SkillsError::coded(ERR_REMOTE_SOURCE_INVALID))?;
    let actual = sha256_hex(&content);
    if expected
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case(&actual))
    {
        return Err(SkillsError::coded(
            crate::skills::api_types::ERR_REMOTE_INTEGRITY_MISMATCH,
        ));
    }
    Ok(StagedSkill {
        root,
        skill_md,
        name,
        content,
        metadata: SkillSourceMetadata {
            source_type: Some(source_type.to_string()),
            normalized_locator: Some(locator),
            actual_sha256: actual,
            ..Default::default()
        },
    })
}

fn stage_bytes(
    bytes: Vec<u8>,
    filename: &str,
    source_type: &str,
    locator: String,
    expected: Option<String>,
    subpath: Option<&str>,
) -> Result<StagedSkill, SkillsError> {
    let root = StagingDir::new()?;
    let input = root.path().join(filename);
    std::fs::write(&input, bytes)
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
    let skill_md = if filename == "SKILL.md" {
        input.clone()
    } else {
        let extracted = root.path().join("package");
        std::fs::create_dir_all(&extracted)
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
        crate::acp::archive::extract_archive(&input, &extracted)
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_INVALID, error))?;
        find_skill_md(&extracted, subpath)?
    };
    staged_skill(root, skill_md, source_type, locator, expected)
}

pub async fn stage_source(source: &SkillSource) -> Result<StagedSkill, SkillsError> {
    let validated = validate_source(source)?;
    match validated {
        ValidatedSkillSource::Local { path } => tokio::task::spawn_blocking(move || {
            let root = StagingDir::new()?;
            let skill_md = root.path().join("SKILL.md");
            copy_skill_file(Path::new(&path), &skill_md)?;
            staged_skill(root, skill_md, "local", path, None)
        })
        .await
        .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?,
        ValidatedSkillSource::Url {
            url,
            expected_sha256,
        } => {
            let is_raw = url.ends_with("/SKILL.md") || url.ends_with("/skill.md");
            let filename = if is_raw { "SKILL.md" } else { "skill.tar.gz" };
            let bytes = fetch_bytes(&url).await?;
            tokio::task::spawn_blocking(move || {
                stage_bytes(bytes, filename, "url", url, expected_sha256, None)
            })
            .await
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?
        }
        ValidatedSkillSource::Github {
            repository,
            reference,
            subpath,
        } => {
            let (url, resolved) = if repository.starts_with("https://") {
                (repository.clone(), reference.clone())
            } else {
                let reference = reference.clone().unwrap_or_else(|| "HEAD".to_string());
                (
                    format!(
                        "https://codeload.github.com/{repository}/tar.gz/refs/heads/{reference}"
                    ),
                    Some(reference),
                )
            };
            let bytes = fetch_bytes(&url).await?;
            let subpath = subpath.clone();
            let mut staged = tokio::task::spawn_blocking(move || {
                stage_bytes(
                    bytes,
                    "github.tar.gz",
                    "github",
                    repository,
                    None,
                    subpath.as_deref(),
                )
            })
            .await
            .map_err(|error| {
                SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string())
            })??;
            staged.metadata.reference = resolved;
            Ok(staged)
        }
        ValidatedSkillSource::Npm {
            package,
            specifier,
            subpath,
        } => {
            let root = StagingDir::new()?;
            let destination = root.path().join("npm");
            std::fs::create_dir_all(&destination).map_err(|error| {
                SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string())
            })?;
            let package_spec = format!("{}{}", package, specifier.as_deref().unwrap_or(""));
            let result = tokio::time::timeout(
                FETCH_TIMEOUT,
                Command::new("npm")
                    .args(["pack", "--ignore-scripts", "--pack-destination"])
                    .arg(&destination)
                    .arg("--")
                    .arg(&package_spec)
                    .current_dir(&destination)
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .map_err(|_| SkillsError::coded(ERR_REMOTE_SOURCE_UNAVAILABLE))?
            .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string()))?;
            if !result.status.success() {
                return Err(SkillsError::new(
                    ERR_REMOTE_SOURCE_UNAVAILABLE,
                    "npm pack failed",
                ));
            }
            let archive = std::fs::read_dir(&destination)
                .map_err(|error| {
                    SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string())
                })?
                .flatten()
                .map(|entry| entry.path())
                .find(|path| path.extension().is_some_and(|ext| ext == "tgz"))
                .ok_or_else(|| SkillsError::coded(ERR_REMOTE_SOURCE_INVALID))?;
            let extracted = root.path().join("package");
            std::fs::create_dir_all(&extracted).map_err(|error| {
                SkillsError::new(ERR_REMOTE_SOURCE_UNAVAILABLE, error.to_string())
            })?;
            crate::acp::archive::extract_archive(&archive, &extracted)
                .map_err(|error| SkillsError::new(ERR_REMOTE_SOURCE_INVALID, error))?;
            let skill_md = find_skill_md(&extracted, subpath.as_deref())?;
            staged_skill(root, skill_md, "npm", package_spec, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_github_shorthand_and_https_url() {
        assert!(validate_source(&SkillSource::Github {
            repository_or_url: "owner/repo".into(),
            reference: None,
            subpath: None
        })
        .is_ok());
        assert!(validate_source(&SkillSource::Url {
            url: "https://example.com/skill.tar.gz".into(),
            expected_sha256: None
        })
        .is_ok());
    }

    #[test]
    fn rejects_unsafe_npm_locators() {
        assert!(validate_source(&SkillSource::Npm {
            package: "lodash".into(),
            version_or_specifier: Some("@https://127.0.0.1/pkg.tgz".into()),
            subpath: None,
        })
        .is_err());
        assert!(validate_source(&SkillSource::Npm {
            package: "--ignore-scripts=false".into(),
            version_or_specifier: None,
            subpath: None,
        })
        .is_err());
    }

    #[test]
    fn rejects_mapped_and_link_local_urls() {
        for url in [
            "https://[::ffff:127.0.0.1]/skill.md",
            "https://[fe80::1]/skill.md",
        ] {
            assert!(validate_source(&SkillSource::Url {
                url: url.into(),
                expected_sha256: None,
            })
            .is_err());
        }
    }

    #[test]
    fn rejects_unsafe_urls_and_paths() {
        assert!(validate_source(&SkillSource::Url {
            url: "http://example.com/skill.md".into(),
            expected_sha256: None
        })
        .is_err());
        assert!(validate_source(&SkillSource::Url {
            url: "file:///tmp/skill.md".into(),
            expected_sha256: None
        })
        .is_err());
        assert!(validate_source(&SkillSource::Url {
            url: "https://127.0.0.1/skill.md".into(),
            expected_sha256: None
        })
        .is_err());
        assert!(validate_source(&SkillSource::Github {
            repository_or_url: "owner/repo".into(),
            reference: None,
            subpath: Some("../SKILL.md".into())
        })
        .is_err());
    }

    #[test]
    fn rejects_github_hosts_with_credentials() {
        assert!(validate_source(&SkillSource::Github {
            repository_or_url: "https://evil.example/owner/repo".into(),
            reference: None,
            subpath: None
        })
        .is_err());
        assert!(validate_source(&SkillSource::Url {
            url: "https://user:pass@example.com/skill.md".into(),
            expected_sha256: None
        })
        .is_err());
    }

    #[tokio::test]
    async fn stages_local_skill_and_computes_digest() {
        let root = tempfile::tempdir().unwrap();
        let skill_dir = root.path().join("demo-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let source = skill_dir.join("SKILL.md");
        std::fs::write(
            &source,
            "---\nname: demo-skill\ndescription: test\n---\n# Demo\n",
        )
        .unwrap();
        let staged = stage_source(&SkillSource::Local {
            source_path: source.to_string_lossy().into_owned(),
        })
        .await
        .unwrap();
        assert_eq!(staged.name, "demo-skill");
        assert_eq!(staged.metadata.actual_sha256, sha256_hex(&staged.content));
        assert!(staged.skill_md.starts_with(staged.root.path()));
    }

    #[test]
    fn validates_expected_digest() {
        assert!(validate_source(&SkillSource::Url {
            url: "https://example.com/skill.md".into(),
            expected_sha256: Some("a".repeat(64))
        })
        .is_ok());
        assert!(validate_source(&SkillSource::Url {
            url: "https://example.com/skill.md".into(),
            expected_sha256: Some("bad".into())
        })
        .is_err());
    }
}
