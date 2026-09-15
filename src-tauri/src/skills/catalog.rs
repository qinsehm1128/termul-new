use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillSource {
    pub provider: String,
    pub scope: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub skill_md_path: String,
    pub digest: String,
    pub metadata_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub scope: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub digest: String,
    pub metadata_digest: String,
    pub status: String,
    #[serde(default)]
    pub conflict: bool,
    #[serde(default)]
    pub drift: bool,
    #[serde(default)]
    pub managed: bool,
    pub sources: Vec<SkillSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SkillsCatalog {
    pub revision: u64,
    pub skills: Vec<SkillRecord>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}
