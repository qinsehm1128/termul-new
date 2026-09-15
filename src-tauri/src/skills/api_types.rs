use crate::skills::catalog::SkillsCatalog;
use crate::skills::provider_config::ProjectionFallbackPolicy;
use serde::{Deserialize, Serialize};

pub const ERR_SKILL_NOT_FOUND: &str = "SKILL_NOT_FOUND";
pub const ERR_PROJECT_NOT_REGISTERED: &str = "PROJECT_NOT_REGISTERED";
pub const ERR_PROJECT_OUTSIDE_BOUNDARY: &str = "PROJECT_OUTSIDE_BOUNDARY";
pub const ERR_INVALID_PROVIDER_CONFIG: &str = "INVALID_PROVIDER_CONFIG";
pub const ERR_INVALID_FRONTMATTER: &str = "INVALID_FRONTMATTER";
pub const ERR_UNMANAGED_COLLISION: &str = "UNMANAGED_COLLISION";
pub const ERR_MANAGED_SKILL_PROTECTED: &str = "MANAGED_SKILL_PROTECTED";
pub const ERR_PROJECTION_UNSUPPORTED: &str = "PROJECTION_UNSUPPORTED";
pub const ERR_PROJECTION_FALLBACK_CONFIRMATION_REQUIRED: &str =
    "PROJECTION_FALLBACK_CONFIRMATION_REQUIRED";
pub const ERR_PROJECTION_DRIFT: &str = "PROJECTION_DRIFT";
pub const ERR_INSTALL_DIGEST_MISMATCH: &str = "INSTALL_DIGEST_MISMATCH";
pub const ERR_CANONICAL_ROOT_UNAVAILABLE: &str = "CANONICAL_ROOT_UNAVAILABLE";
pub const ERR_WATCHER_BACKPRESSURE: &str = "WATCHER_BACKPRESSURE";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillsCatalogRequest {
    pub project_id: Option<String>,
    pub project_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsInstallRequest {
    pub name: String,
    pub source_path: String,
    #[serde(default)]
    pub scope: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub confirm_token: Option<String>,
    pub fallback: Option<ProjectionFallbackPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsProjectionRequest {
    pub name: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub confirm_fallback: Option<bool>,
    pub fallback: Option<ProjectionFallbackPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsRepairRequest {
    pub name: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsStatus {
    pub catalog: SkillsCatalog,
    pub fallback_policy: ProjectionFallbackPolicy,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub watched_roots: Vec<String>,
    #[serde(default)]
    pub last_scan_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsHubEvent {
    pub kind: String,
    pub revision: u64,
    pub project_id: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

pub const EVENT_CATALOG_CHANGED: &str = "skills_catalog_changed";
pub const EVENT_CONFLICT_DETECTED: &str = "skills_conflict_detected";
pub const EVENT_LINK_DRIFT: &str = "skills_link_drift_detected";
pub const EVENT_SYNC_STALE: &str = "skills_sync_stale";

#[derive(Debug, Clone)]
pub struct SkillsError {
    pub code: String,
    pub message: String,
}

impl SkillsError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn coded(code: &'static str) -> Self {
        Self {
            code: code.to_string(),
            message: code.to_string(),
        }
    }

    pub fn into_string(self) -> String {
        if self.message == self.code || self.message.starts_with(&format!("{}:", self.code)) {
            self.message
        } else {
            format!("{}: {}", self.code, self.message)
        }
    }
}

impl std::fmt::Display for SkillsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message == self.code {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}: {}", self.code, self.message)
        }
    }
}
