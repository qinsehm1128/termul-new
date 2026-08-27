use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::discover::EditorWorkspaceList;
use super::normalize::{decode_location, EditorWorkspaceKind};
use super::{discover, EditorWorkspaceSource};

#[derive(Debug, Deserialize)]
struct CodeWorkspaceFile {
    #[serde(default)]
    folders: Vec<CodeWorkspaceFolder>,
}

#[derive(Debug, Deserialize)]
struct CodeWorkspaceFolder {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

pub fn parse_code_workspace_file(path: &Path) -> Result<EditorWorkspaceList, String> {
    if !path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("code-workspace"))
    {
        return Err("not a .code-workspace file".to_string());
    }
    let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let parsed: CodeWorkspaceFile =
        serde_json::from_str(&contents).map_err(|error| error.to_string())?;
    let base = path.parent().unwrap_or(path);
    Ok(EditorWorkspaceList {
        candidates: folders_from_workspace(EditorWorkspaceKind::Vscode, base, &parsed.folders),
    })
}

fn folders_from_workspace(
    editor: EditorWorkspaceKind,
    base: &Path,
    folders: &[CodeWorkspaceFolder],
) -> Vec<super::EditorWorkspaceCandidate> {
    let mut collected = Vec::new();
    for folder in folders {
        let resolved = resolve_folder_path(base, &folder.path);
        collected.extend(discover::collect_folder(
            editor,
            EditorWorkspaceSource::WorkspaceFile,
            &resolved,
            folder.name.as_deref(),
        ));
    }
    collected
}

fn resolve_folder_path(base: &Path, raw: &str) -> PathBuf {
    if let Some(decoded) = decode_location(raw) {
        if decoded.is_absolute() {
            return decoded;
        }
        return base.join(decoded);
    }
    base.join(raw)
}
