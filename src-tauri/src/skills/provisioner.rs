use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::conversation::{DirectoryPermissions, DurableFileSystem};

pub const SCHEDULED_TASK_SKILL_NAME: &str = "termul-scheduled-tasks";
pub const SCHEDULED_TASK_SKILL_TEMPLATE_VERSION: u32 = 2;
const MANAGED_MARKER: &str = "<!-- managed-by-termul:termul-scheduled-tasks -->";

const SKILL_TEMPLATE: &str = r#"---
name: termul-scheduled-tasks
description: Draft safe Termul-level AI schedules for review from any Conversation.
metadata:
  managedByTermul: true
  templateVersion: 2
---
<!-- managed-by-termul:termul-scheduled-tasks -->

# Termul Scheduled Tasks

Use Termul's `scheduled_task_*` tools when a user asks for recurring, delayed,
or one-time autonomous work.

1. Clarify the timezone, schedule, ACP, execution directory, expected result,
   and minimum permissions. A project association is optional; never require
   one before drafting a task.
2. Call `scheduled_task_preview` and show the next execution times. Never
   calculate timezone or DST behavior yourself.
3. Create or update a draft. A draft does not run until the user confirms the
   exact revision and hash in Termul.
4. Never place secrets, credentials, environment variables, or hidden model
   reasoning in a task prompt. Explain observable decisions through normal
   messages, plans, and tool calls.
5. Prefer narrow permissions and an explicit failure result. Do not add
   automatic retries for work that can have side effects.
6. `scheduled_task_pause` affects future occurrences only. It does not cancel
   a Conversation that has already started.

Termul stores full execution traces in the isolated child Conversation. The task
run ledger contains only status, hashes, timestamps, usage, and Conversation
links.
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillProvider {
    Claude,
    Qwen,
    Cline,
    CrossTool,
}

impl SkillProvider {
    fn relative_root(self) -> &'static str {
        match self {
            Self::Claude => ".claude/skills",
            Self::Qwen => ".qwen/skills",
            Self::Cline => ".cline/skills",
            Self::CrossTool => ".agents/skills",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedSkillManifestV1 {
    pub schema_version: u32,
    pub managed_by_termul: bool,
    pub skill_name: String,
    pub template_version: u32,
    pub sha256: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedSkill {
    pub primary_path: PathBuf,
    pub mirrored_paths: Vec<PathBuf>,
    pub sha256: String,
}

#[derive(Debug)]
pub enum SkillProvisionError {
    InvalidWorkspace(String),
    UnmanagedCollision(PathBuf),
    Io(std::io::Error),
    Durable(crate::conversation::DurableFsError),
    Json(serde_json::Error),
}

impl std::fmt::Display for SkillProvisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidWorkspace(detail) => {
                write!(formatter, "invalid skill workspace: {detail}")
            }
            Self::UnmanagedCollision(path) => write!(
                formatter,
                "refusing to overwrite an unmanaged skill at '{}'",
                path.display()
            ),
            Self::Io(error) => write!(formatter, "skill provisioning io error: {error}"),
            Self::Durable(error) => write!(
                formatter,
                "skill provisioning durable write failed: {error}"
            ),
            Self::Json(error) => write!(formatter, "skill manifest serialization failed: {error}"),
        }
    }
}

impl std::error::Error for SkillProvisionError {}

impl From<std::io::Error> for SkillProvisionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<crate::conversation::DurableFsError> for SkillProvisionError {
    fn from(value: crate::conversation::DurableFsError) -> Self {
        Self::Durable(value)
    }
}

impl From<serde_json::Error> for SkillProvisionError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Default)]
pub struct ConversationSkillProvisioner {
    durable_fs: DurableFileSystem,
}

impl ConversationSkillProvisioner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn provision(
        &self,
        workspace_cwd: &Path,
        agent_config_id: &str,
    ) -> Result<ProvisionedSkill, SkillProvisionError> {
        if !workspace_cwd.is_absolute() {
            return Err(SkillProvisionError::InvalidWorkspace(
                "workspace path must be absolute".to_string(),
            ));
        }
        if !workspace_cwd.is_dir() {
            return Err(SkillProvisionError::InvalidWorkspace(
                "workspace directory does not exist".to_string(),
            ));
        }

        let provider = provider_for_agent(agent_config_id);
        let primary = skill_path(workspace_cwd, SkillProvider::CrossTool);
        let mut paths = vec![primary.clone()];
        let native = skill_path(workspace_cwd, provider);
        if native != primary {
            paths.push(native);
        }

        let sha256 = sha256(SKILL_TEMPLATE.as_bytes());
        for path in &paths {
            self.write_managed_skill(path)?;
        }
        let manifest = ManagedSkillManifestV1 {
            schema_version: 1,
            managed_by_termul: true,
            skill_name: SCHEDULED_TASK_SKILL_NAME.to_string(),
            template_version: SCHEDULED_TASK_SKILL_TEMPLATE_VERSION,
            sha256: sha256.clone(),
            paths: paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        };
        let manifest_dir = workspace_cwd.join(".termul");
        self.durable_fs
            .create_dir_durable(&manifest_dir, DirectoryPermissions::PrivateOwnerOnly)?;
        self.durable_fs.replace_bytes(
            &manifest_dir.join("managed-skills.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        log::info!(
            "[scheduled-task-skill] boundary=provisioned provider={:?} version={} mirror_count={}",
            provider,
            SCHEDULED_TASK_SKILL_TEMPLATE_VERSION,
            paths.len()
        );
        Ok(ProvisionedSkill {
            primary_path: primary,
            mirrored_paths: paths,
            sha256,
        })
    }

    fn write_managed_skill(&self, path: &Path) -> Result<(), SkillProvisionError> {
        if let Ok(existing) = fs::read_to_string(path) {
            if !existing.contains(MANAGED_MARKER) {
                return Err(SkillProvisionError::UnmanagedCollision(path.to_path_buf()));
            }
            if existing == SKILL_TEMPLATE {
                return Ok(());
            }
        }
        let parent = path.parent().ok_or_else(|| {
            SkillProvisionError::InvalidWorkspace("skill path has no parent".to_string())
        })?;
        self.durable_fs
            .create_dir_durable(parent, DirectoryPermissions::PrivateOwnerOnly)?;
        self.durable_fs
            .replace_bytes(path, SKILL_TEMPLATE.as_bytes())?;
        Ok(())
    }
}

#[must_use]
pub fn provider_for_agent(agent_config_id: &str) -> SkillProvider {
    let id = agent_config_id.to_ascii_lowercase();
    if id.contains("claude") {
        SkillProvider::Claude
    } else if id.contains("qwen") {
        SkillProvider::Qwen
    } else if id.contains("cline") {
        SkillProvider::Cline
    } else {
        SkillProvider::CrossTool
    }
}

fn skill_path(workspace_cwd: &Path, provider: SkillProvider) -> PathBuf {
    workspace_cwd
        .join(provider.relative_root())
        .join(SCHEDULED_TASK_SKILL_NAME)
        .join("SKILL.md")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn workspace(label: &str) -> PathBuf {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("termul-skill-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn provider_mapping_matches_supported_native_roots() {
        assert_eq!(provider_for_agent("claude-acp"), SkillProvider::Claude);
        assert_eq!(provider_for_agent("qwen-code"), SkillProvider::Qwen);
        assert_eq!(provider_for_agent("cline"), SkillProvider::Cline);
        assert_eq!(provider_for_agent("codex-acp"), SkillProvider::CrossTool);
        assert_eq!(provider_for_agent("gemini"), SkillProvider::CrossTool);
    }

    #[test]
    fn provisions_cross_tool_and_native_mirror_inside_managed_workspace() {
        let root = workspace("mirror");
        let receipt = ConversationSkillProvisioner::new()
            .provision(&root, "claude-acp")
            .unwrap();
        assert!(receipt.primary_path.is_file());
        assert!(root
            .join(".claude/skills/termul-scheduled-tasks/SKILL.md")
            .is_file());
        assert!(root.join(".termul/managed-skills.json").is_file());
        assert!(receipt
            .mirrored_paths
            .iter()
            .all(|path| path.starts_with(&root)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_to_replace_an_unmanaged_skill() {
        let root = workspace("collision");
        let path = root.join(".agents/skills/termul-scheduled-tasks/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "user owned").unwrap();
        assert!(matches!(
            ConversationSkillProvisioner::new().provision(&root, "codex"),
            Err(SkillProvisionError::UnmanagedCollision(found)) if found == path
        ));
        let _ = fs::remove_dir_all(root);
    }
}
