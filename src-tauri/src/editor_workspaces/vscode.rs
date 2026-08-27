use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::code_workspace;
use super::normalize::{decode_location, EditorWorkspaceKind};
use super::{discover, EditorWorkspaceCandidate, EditorWorkspaceSource};

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StorageFile {
    #[serde(default)]
    backup_workspaces: Option<BackupWorkspaces>,
    #[serde(default)]
    profile_associations: Option<ProfileAssociations>,
}

#[derive(Debug, Deserialize, Default)]
struct BackupWorkspaces {
    #[serde(default)]
    folders: Vec<BackupFolder>,
    #[serde(default)]
    workspaces: Vec<BackupWorkspace>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct BackupFolder {
    folder_uri: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct BackupWorkspace {
    #[serde(
        default,
        rename = "configURIPath",
        alias = "configURI",
        alias = "configUriPath"
    )]
    config_uri_path: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct ProfileAssociations {
    #[serde(default)]
    workspaces: serde_json::Map<String, serde_json::Value>,
}

pub fn product_roots() -> Vec<(String, EditorWorkspaceKind)> {
    vec![
        ("Code".into(), EditorWorkspaceKind::Vscode),
        ("Code - Insiders".into(), EditorWorkspaceKind::Vscode),
        ("VSCodium".into(), EditorWorkspaceKind::Vscode),
        ("Cursor".into(), EditorWorkspaceKind::Cursor),
        ("Windsurf".into(), EditorWorkspaceKind::Windsurf),
        ("Trae".into(), EditorWorkspaceKind::Trae),
        ("TRAE SOLO CN".into(), EditorWorkspaceKind::Trae),
    ]
}

pub fn discover() -> Vec<EditorWorkspaceCandidate> {
    discover_from_roots(&product_roots(), &application_support_dirs())
}

pub fn discover_from_roots(
    products: &[(String, EditorWorkspaceKind)],
    support_roots: &[PathBuf],
) -> Vec<EditorWorkspaceCandidate> {
    let mut collected = Vec::new();
    for root in support_roots {
        for (product, editor) in products {
            let product_root = root.join(product);
            collected.extend(discover_product(*editor, &product_root));
        }
    }
    discover::dedupe(collected).candidates
}

fn application_support_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = super::normalize::user_home() {
        dirs.push(home.join("Library").join("Application Support"));
        dirs.push(home.join(".config"));
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            dirs.push(PathBuf::from(xdg));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata));
    }
    dirs
}

fn discover_product(
    editor: EditorWorkspaceKind,
    product_root: &Path,
) -> Vec<EditorWorkspaceCandidate> {
    let mut collected = Vec::new();
    for storage in [
        product_root.join("storage.json"),
        product_root
            .join("User")
            .join("globalStorage")
            .join("storage.json"),
    ] {
        collected.extend(discover_storage_file(editor, &storage));
    }
    collected
}

fn discover_storage_file(
    editor: EditorWorkspaceKind,
    storage: &Path,
) -> Vec<EditorWorkspaceCandidate> {
    let Ok(contents) = std::fs::read_to_string(storage) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<StorageFile>(&contents) else {
        return Vec::new();
    };
    let mut collected = Vec::new();
    if let Some(backup) = parsed.backup_workspaces {
        for folder in backup.folders {
            if let Some(uri) = folder.folder_uri.as_deref() {
                if let Some(path) = decode_location(uri) {
                    collected.extend(discover::collect_folder(
                        editor,
                        EditorWorkspaceSource::Recent,
                        &path,
                        None,
                    ));
                }
            }
        }
        for workspace in backup.workspaces {
            if let Some(uri) = workspace.config_uri_path.as_deref() {
                collected.extend(expand_workspace_file(editor, uri));
            }
        }
    }
    if let Some(associations) = parsed.profile_associations {
        for key in associations.workspaces.keys() {
            if key.ends_with(".code-workspace") {
                collected.extend(expand_workspace_file(editor, key));
            } else if let Some(path) = decode_location(key) {
                collected.extend(discover::collect_folder(
                    editor,
                    EditorWorkspaceSource::Recent,
                    &path,
                    None,
                ));
            }
        }
    }
    collected
}

fn expand_workspace_file(editor: EditorWorkspaceKind, uri: &str) -> Vec<EditorWorkspaceCandidate> {
    let Some(path) = decode_location(uri) else {
        return Vec::new();
    };
    match code_workspace::parse_code_workspace_file(&path) {
        Ok(list) => list
            .candidates
            .into_iter()
            .map(|mut candidate| {
                candidate.editor = editor;
                candidate.id = super::normalize::candidate_id(editor, Path::new(&candidate.path));
                candidate
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}
