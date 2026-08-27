use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorWorkspaceKind {
    Vscode,
    Cursor,
    Windsurf,
    Trae,
    Zed,
}

impl EditorWorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vscode => "vscode",
            Self::Cursor => "cursor",
            Self::Windsurf => "windsurf",
            Self::Trae => "trae",
            Self::Zed => "zed",
        }
    }
}

pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn expand_tilde(path: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = user_home() {
            return home.join(rest);
        }
    }
    if raw == "~" {
        if let Some(home) = user_home() {
            return home;
        }
    }
    path.to_path_buf()
}

pub fn decode_location(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(url) = Url::parse(trimmed) {
        if url.scheme() == "file" {
            return url.to_file_path().ok().map(|path| expand_tilde(&path));
        }
    }
    Some(expand_tilde(Path::new(trimmed)))
}

pub fn normalize_project_path(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed == "/" {
        return "/".to_string();
    }
    let mut normalized = trimmed.trim_end_matches('/').to_string();
    #[cfg(windows)]
    {
        normalized = normalized.to_ascii_lowercase();
    }
    #[cfg(not(windows))]
    {
        normalized = normalized.to_lowercase();
    }
    normalized
}

pub fn candidate_id(editor: EditorWorkspaceKind, path: &Path) -> String {
    format!("{}:{}", editor.as_str(), normalize_project_path(path))
}

pub fn display_name(path: &Path, explicit: Option<&str>) -> String {
    if let Some(name) = explicit.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Untitled")
        .to_string()
}
