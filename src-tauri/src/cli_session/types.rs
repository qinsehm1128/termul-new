use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_LIMIT_PER_AGENT: usize = 80;
pub const WALK_LIMIT_PER_AGENT: usize = 1000;
pub const SESSION_ID_MAX_LEN: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliSessionAgentId {
    ClaudeCode,
    Codex,
    GeminiCli,
    Cursor,
    Opencode,
    Pi,
}

impl CliSessionAgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::GeminiCli => "gemini-cli",
            Self::Cursor => "cursor",
            Self::Opencode => "opencode",
            Self::Pi => "pi",
        }
    }

    pub fn all() -> [Self; 6] {
        [
            Self::ClaudeCode,
            Self::Codex,
            Self::GeminiCli,
            Self::Cursor,
            Self::Opencode,
            Self::Pi,
        ]
    }
}

impl std::fmt::Display for CliSessionAgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredCliSession {
    pub schema_version: u32,
    pub id: String,
    pub agent_id: CliSessionAgentId,
    pub session_id: String,
    pub cwd: Option<String>,
    pub title: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: u32,
    pub file_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_file_path: Option<String>,
    pub resumable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionScanIssue {
    pub agent_id: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionListArgs {
    #[serde(default)]
    pub scope_paths: Option<Vec<String>>,
    #[serde(default)]
    pub agents: Option<Vec<CliSessionAgentId>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionListResult {
    pub sessions: Vec<DiscoveredCliSession>,
    pub issues: Vec<CliSessionScanIssue>,
    pub scanned_at: String,
}

pub fn normalize_session_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > SESSION_ID_MAX_LEN
        || trimmed.starts_with('-')
        || trimmed
            .chars()
            .any(|ch| ch <= '\u{001f}' || ch == '\u{007f}')
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub const RESOLVE_BATCH_MAX: usize = 16;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionResolveFile {
    pub agent_id: CliSessionAgentId,
    pub file_path: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionResolveArgs {
    #[serde(default)]
    pub files: Vec<CliSessionResolveFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CliSessionResolveResult {
    pub sessions: Vec<DiscoveredCliSession>,
    pub issues: Vec<CliSessionScanIssue>,
}

pub fn session_list_id(agent: CliSessionAgentId, file_path: &str) -> String {
    format!("{}:{file_path}", agent.as_str())
}

#[cfg(test)]
pub fn first_cwd_wins(current: &mut Option<String>, next: Option<String>) {
    if current.is_some() {
        return;
    }
    if let Some(value) = next.filter(|cwd| !cwd.trim().is_empty()) {
        *current = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_session_ids() {
        assert!(normalize_session_id("").is_none());
        assert!(normalize_session_id("-x").is_none());
        assert!(normalize_session_id("a\nb").is_none());
        assert_eq!(normalize_session_id("abc-1").as_deref(), Some("abc-1"));
    }

    #[test]
    fn keeps_first_cwd() {
        let mut cwd = None;
        first_cwd_wins(&mut cwd, Some("/a".into()));
        first_cwd_wins(&mut cwd, Some("/b".into()));
        assert_eq!(cwd.as_deref(), Some("/a"));
    }
}
