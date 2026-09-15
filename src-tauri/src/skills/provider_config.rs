//! Layered, auditable provider configuration for Skills scanning.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "agent-providers.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectionFallbackPolicy {
    #[default]
    Ask,
    Copy,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub global_roots: Vec<String>,
    #[serde(default)]
    pub project_roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderFile {
    #[serde(default)]
    pub fallback_policy: ProjectionFallbackPolicy,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigSnapshot {
    pub fallback_policy: ProjectionFallbackPolicy,
    pub providers: Vec<ProviderConfig>,
    pub diagnostics: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl ProviderConfigSnapshot {
    pub fn from_layers(layers: &[(&str, &str)]) -> Self {
        let mut merged = ProviderFile::default();
        let mut diagnostics = Vec::new();
        for (label, raw) in layers {
            let allow_global = !label.starts_with("project:");
            match toml::from_str::<ProviderFile>(raw) {
                Ok(file) => {
                    if allow_global {
                        merged.fallback_policy = file.fallback_policy;
                    }
                    for (id, mut provider) in file.providers {
                        provider.id = id.clone();
                        if !allow_global {
                            provider.global_roots.clear();
                        }
                        if let Some(existing) = merged.providers.get_mut(&id) {
                            existing.enabled = provider.enabled;
                            if allow_global && !provider.global_roots.is_empty() {
                                existing.global_roots = provider.global_roots;
                            }
                            if !provider.project_roots.is_empty() {
                                existing.project_roots = provider.project_roots;
                            }
                        } else {
                            merged.providers.insert(id, provider);
                        }
                    }
                }
                Err(error) => diagnostics.push(format!(
                    "INVALID_PROVIDER_CONFIG: {label}: invalid provider config: {error}"
                )),
            }
        }
        Self {
            fallback_policy: merged.fallback_policy,
            providers: merged.providers.into_values().collect(),
            diagnostics,
        }
    }

    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    pub fn to_file(&self) -> ProviderFile {
        let mut providers = BTreeMap::new();
        for provider in &self.providers {
            providers.insert(provider.id.clone(), provider.clone());
        }
        ProviderFile {
            fallback_policy: self.fallback_policy,
            providers,
        }
    }

    pub fn has_invalid_config(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("INVALID_PROVIDER_CONFIG"))
    }
}

/// Expand configured path tokens without walking arbitrary directories.
pub fn expand_path(
    template: &str,
    home: &Path,
    config: &Path,
    project: Option<&Path>,
) -> Result<PathBuf, String> {
    let mut value = template.replace("{home}", &home.to_string_lossy());
    value = value.replace("{config}", &config.to_string_lossy());
    if value.contains("{project}") {
        let project =
            project.ok_or_else(|| "{project} requires a registered project".to_string())?;
        value = value.replace("{project}", &project.to_string_lossy());
    }
    while let Some(start) = value.find("{env:") {
        let end = value[start..]
            .find('}')
            .ok_or_else(|| "unterminated environment token".to_string())?
            + start;
        let key = &value[start + 5..end];
        if key.is_empty() {
            return Err("environment token must name a variable".to_string());
        }
        let replacement =
            env::var_os(key).ok_or_else(|| format!("environment variable {key} is not set"))?;
        value.replace_range(start..=end, &replacement.to_string_lossy());
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!(
            "expanded provider root is not absolute: {}",
            path.display()
        ));
    }
    Ok(path)
}

pub fn config_file_name() -> &'static str {
    CONFIG_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_override_existing_and_allow_custom_provider() {
        let snapshot = ProviderConfigSnapshot::from_layers(&[
            (
                "default",
                r#"fallback_policy = "ask"
[providers.claude]
global_roots = ["{home}/.claude/skills"]
project_roots = ["{project}/.claude/skills"]
"#,
            ),
            (
                "user",
                r#"fallback_policy = "copy"
[providers.claude]
enabled = false
[providers.internal]
global_roots = ["{config}/internal"]
"#,
            ),
        ]);
        assert_eq!(snapshot.fallback_policy, ProjectionFallbackPolicy::Copy);
        assert!(!snapshot.provider("claude").unwrap().enabled);
        assert!(snapshot.provider("internal").is_some());
        assert!(snapshot.diagnostics.is_empty());
    }

    #[test]
    fn invalid_layer_is_diagnostic_and_does_not_discard_last_good() {
        let snapshot = ProviderConfigSnapshot::from_layers(&[
            (
                "default",
                "[providers.ok]\nglobal_roots = [\"{home}/skills\"]",
            ),
            ("user", "not toml"),
        ]);
        assert!(snapshot.provider("ok").is_some());
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.has_invalid_config());
    }

    #[test]
    fn project_layer_cannot_expand_global_roots() {
        let snapshot = ProviderConfigSnapshot::from_layers(&[
            (
                "default",
                r#"
[providers.agents]
global_roots = ["{home}/.agents/skills"]
project_roots = ["{project}/.agents/skills"]
"#,
            ),
            (
                "project:p1",
                r#"
fallback_policy = "copy"
[providers.agents]
global_roots = ["{home}/.escaped/skills"]
project_roots = ["{project}/.custom/skills"]
[providers.extra]
global_roots = ["{home}/.extra/skills"]
project_roots = ["{project}/.extra/skills"]
"#,
            ),
        ]);
        assert_eq!(snapshot.fallback_policy, ProjectionFallbackPolicy::Ask);
        assert_eq!(
            snapshot.provider("agents").unwrap().global_roots,
            vec!["{home}/.agents/skills".to_string()]
        );
        assert_eq!(
            snapshot.provider("agents").unwrap().project_roots,
            vec!["{project}/.custom/skills".to_string()]
        );
        let extra = snapshot.provider("extra").unwrap();
        assert!(extra.global_roots.is_empty());
        assert_eq!(
            extra.project_roots,
            vec!["{project}/.extra/skills".to_string()]
        );
    }

    #[test]
    fn project_token_requires_registered_project() {
        let result = expand_path(
            "{project}/.agents/skills",
            Path::new("/home/u"),
            Path::new("/cfg"),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn environment_tokens_expand_to_absolute_paths() {
        std::env::set_var("SE_SKILLS_TEST_ROOT", "/tmp/skills");
        let result = expand_path(
            "{env:SE_SKILLS_TEST_ROOT}/x",
            Path::new("/home/u"),
            Path::new("/cfg"),
            None,
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/tmp/skills/x"));
    }
}
