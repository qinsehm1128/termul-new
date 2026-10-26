use serde::{Deserialize, Serialize};

use crate::QuickTerminalId;

pub const QUICK_TERMINAL_SCHEMA_VERSION: u32 = 2;

/// Where the shell starts. Mirrors the agent session execution target so the
/// launcher can offer the same three choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuickTerminalTarget {
    /// A fresh private folder allocated for this quick terminal.
    Workspace,
    ProjectRoot {
        #[serde(rename = "projectId")]
        project_id: String,
        #[serde(rename = "projectRoot")]
        project_root: String,
    },
    Worktree {
        #[serde(rename = "projectId")]
        project_id: String,
        #[serde(rename = "worktreePath")]
        worktree_path: String,
        #[serde(rename = "worktreeBranch")]
        worktree_branch: String,
    },
}

impl QuickTerminalTarget {
    pub fn project_id(&self) -> Option<&str> {
        match self {
            Self::Workspace => None,
            Self::ProjectRoot { project_id, .. } | Self::Worktree { project_id, .. } => {
                Some(project_id)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuickTerminalOrigin {
    Created,
    /// Moved out of a legacy terminal-backed agent Conversation (same id).
    MigratedConversation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuickTerminalRecord {
    pub schema_version: u32,
    pub id: QuickTerminalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub target: QuickTerminalTarget,
    /// Absolute directory the shell starts in.
    pub cwd: String,
    pub created_at_utc: String,
    pub updated_at_utc: String,
    /// Primary PTY alias, synchronized with the first owned terminal ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_id: Option<String>,
    /// All PTYs owned by this entry. Defaults permit reading schema v1.
    #[serde(default)]
    pub terminal_ids: Vec<String>,
    pub origin: QuickTerminalOrigin,
}

impl QuickTerminalRecord {
    pub(crate) fn upgrade(&mut self) {
        if self.schema_version == 1 && self.terminal_ids.is_empty() {
            self.terminal_ids.extend(self.terminal_id.clone());
        }
        let mut seen = std::collections::HashSet::new();
        self.terminal_ids
            .retain(|id| !id.is_empty() && seen.insert(id.clone()));
        self.schema_version = QUICK_TERMINAL_SCHEMA_VERSION;
        self.sync_primary();
    }

    pub(crate) fn sync_primary(&mut self) {
        self.terminal_id = self.terminal_ids.first().cloned();
    }
}
