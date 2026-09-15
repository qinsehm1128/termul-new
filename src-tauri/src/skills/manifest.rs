use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillManifest {
    pub name: String,
    pub digest: String,
    pub canonical_path: PathBuf,
    #[serde(default)]
    pub projections: Vec<ProjectionManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionManifest {
    pub provider: String,
    pub target_path: PathBuf,
    pub mode: String,
    pub source_digest: String,
    pub target_digest: String,
    #[serde(default)]
    pub fallback_reason: Option<String>,
}
