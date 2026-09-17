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
pub const ERR_REMOTE_SOURCE_INVALID: &str = "REMOTE_SOURCE_INVALID";
pub const ERR_REMOTE_SOURCE_UNSUPPORTED: &str = "REMOTE_SOURCE_UNSUPPORTED";
pub const ERR_REMOTE_SOURCE_UNAVAILABLE: &str = "REMOTE_SOURCE_UNAVAILABLE";
pub const ERR_REMOTE_INTEGRITY_MISMATCH: &str = "REMOTE_INTEGRITY_MISMATCH";
pub const ERR_REMOTE_CONFIRMATION_REQUIRED: &str = "REMOTE_CONFIRMATION_REQUIRED";
pub const ERR_SKILL_OPERATION_NOT_FOUND: &str = "SKILL_OPERATION_NOT_FOUND";
pub const ERR_SKILL_OPERATION_CANCELLED: &str = "SKILL_OPERATION_CANCELLED";
pub const ERR_SKILL_OPERATION_BUSY: &str = "SKILL_OPERATION_BUSY";
pub const ERR_SKILL_OPERATION_JOIN_FAILED: &str = "SKILL_OPERATION_JOIN_FAILED";
pub const ERR_REMOTE_MODE_UNSUPPORTED: &str = "REMOTE_MODE_UNSUPPORTED";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SkillSource {
    Local {
        source_path: String,
    },
    Github {
        repository_or_url: String,
        #[serde(default)]
        reference: Option<String>,
        #[serde(default)]
        subpath: Option<String>,
    },
    Npm {
        package: String,
        #[serde(default)]
        version_or_specifier: Option<String>,
        #[serde(default)]
        subpath: Option<String>,
    },
    Url {
        url: String,
        #[serde(default)]
        expected_sha256: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SkillScope {
    Global,
    Project { project_id: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SkillInstallMode {
    #[default]
    InstallAndProject,
    InstallOnly,
    ProjectionOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsPreviewRequest {
    pub source: SkillSource,
    pub scope: SkillScope,
    #[serde(default)]
    pub mode: SkillInstallMode,
    #[serde(default)]
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsInstallPlan {
    pub source: SkillSource,
    pub scope: SkillScope,
    pub mode: SkillInstallMode,
    pub provider_ids: Vec<String>,
    pub preview_id: String,
    pub name: String,
    pub actual_sha256: String,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub requires_digest_confirmation: bool,
    #[serde(default)]
    pub collision_confirm_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsInstallCommit {
    pub preview_id: String,
    #[serde(default)]
    pub confirm_digest: bool,
    #[serde(default)]
    pub confirm_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOperationStart {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOperationProgress {
    pub job_id: String,
    pub phase: String,
    #[serde(default)]
    pub completed_units: Option<u64>,
    #[serde(default)]
    pub total_units: Option<u64>,
    #[serde(default)]
    pub bytes_received: Option<u64>,
    #[serde(default)]
    pub bytes_total: Option<u64>,
    #[serde(default)]
    pub stable_code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsOperationStatus {
    pub job_id: String,
    pub phase: String,
    #[serde(default)]
    pub progress: Option<SkillsOperationProgress>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillSourceMetadata {
    #[serde(default)]
    pub source_type: Option<String>,
    #[serde(default)]
    pub normalized_locator: Option<String>,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub resolved_version: Option<String>,
    #[serde(default)]
    pub resolved_commit: Option<String>,
    pub actual_sha256: String,
}

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
