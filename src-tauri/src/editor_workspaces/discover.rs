use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::normalize::{candidate_id, display_name, normalize_project_path, EditorWorkspaceKind};
use super::{vscode, zed, EditorWorkspaceCandidate, EditorWorkspaceSource};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorWorkspaceList {
    pub candidates: Vec<EditorWorkspaceCandidate>,
}

pub fn discover_editor_workspaces() -> EditorWorkspaceList {
    let mut collected = vscode::discover();
    collected.extend(zed::discover());
    let list = dedupe(collected);
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for candidate in &list.candidates {
        *counts.entry(candidate.editor.as_str()).or_default() += 1;
    }
    for (editor, count) in counts {
        log::info!(
            target: "se_manager::editor_workspaces",
            "operation=discover editor={editor} count={count} stable_code=OK"
        );
        tracing::info!(
            target = "se_manager::editor_workspaces",
            operation = "discover",
            editor,
            count,
            stable_code = "OK",
            "editor workspace discovery"
        );
    }
    list
}

pub fn collect_folder(
    editor: EditorWorkspaceKind,
    source: EditorWorkspaceSource,
    path: &Path,
    explicit_name: Option<&str>,
) -> Vec<EditorWorkspaceCandidate> {
    let resolved = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => path.to_path_buf(),
    };
    if !resolved.is_dir() {
        return Vec::new();
    }
    vec![EditorWorkspaceCandidate {
        id: candidate_id(editor, &resolved),
        editor,
        name: display_name(&resolved, explicit_name),
        path: resolved.to_string_lossy().into_owned(),
        source,
    }]
}

pub fn dedupe(candidates: Vec<EditorWorkspaceCandidate>) -> EditorWorkspaceList {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for candidate in candidates {
        let key = (
            candidate.editor,
            normalize_project_path(&PathBuf::from(&candidate.path)),
        );
        if seen.insert(key) {
            unique.push(candidate);
        }
    }
    unique.sort_by(|left, right| {
        left.editor
            .as_str()
            .cmp(right.editor.as_str())
            .then(left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then(left.path.cmp(&right.path))
    });
    EditorWorkspaceList { candidates: unique }
}
